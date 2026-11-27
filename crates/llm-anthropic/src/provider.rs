use std::sync::Arc;
use std::time::Duration;

use application::ports::{
    LlmChunk, LlmError, LlmProvider, LlmRequest, LlmResponse, ModelInfo, SecretStore, Usage,
};
use application::streaming::{error_for_status, llm_stream};
use async_trait::async_trait;
use domain::{ModelId, ProviderId, Role, SecretRef};
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use reqwest::header::HeaderValue;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::decoder::AnthropicDecoder;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
/// The Messages API requires `max_tokens`.
const DEFAULT_MAX_TOKENS: u32 = 1024;

#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub base_url: String,
    /// Where the API key lives in the `SecretStore`.
    pub api_key: SecretRef,
}

impl AnthropicConfig {
    pub fn new(api_key: SecretRef) -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

pub struct AnthropicProvider {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    config: AnthropicConfig,
    secrets: Arc<dyn SecretStore>,
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: Option<String>,
}

#[derive(Deserialize)]
struct ModelsBody {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    display_name: Option<String>,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig, secrets: Arc<dyn SecretStore>) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .user_agent("fathom")
            .connect_timeout(Duration::from_secs(10))
            // Time allowed between chunks of a streamed reply, not for the whole reply.
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Network(e.without_url().to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                config,
                secrets,
            }),
        })
    }
}

impl Inner {
    async fn key(&self) -> Result<SecretString, LlmError> {
        match self.secrets.get(&self.config.api_key).await {
            Ok(Some(key)) => Ok(key),
            // No key configured: report it as an authentication problem.
            Ok(None) => Err(LlmError::Authentication),
            Err(e) => Err(LlmError::Provider(format!("secret store: {e}"))),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url.trim_end_matches('/'))
    }

    fn redact(&self, text: &str, key: &SecretString) -> String {
        let secret = key.expose_secret();
        if secret.is_empty() {
            text.to_owned()
        } else {
            text.replace(secret, "[redacted]")
        }
    }

    fn headers(&self, key: &SecretString) -> Result<reqwest::header::HeaderMap, LlmError> {
        let mut value =
            HeaderValue::from_str(key.expose_secret()).map_err(|_| LlmError::Authentication)?;
        value.set_sensitive(true);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", value);
        headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
        Ok(headers)
    }

    async fn check(
        &self,
        response: reqwest::Response,
        key: &SecretString,
    ) -> Result<reqwest::Response, LlmError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ErrorBody>(&body)
            .ok()
            .and_then(|b| b.error)
            .and_then(|e| e.message)
            .unwrap_or(body);
        Err(error_for_status(
            status.as_u16(),
            Some(&self.redact(&message, key)),
        ))
    }

    fn network(&self, e: reqwest::Error, key: &SecretString) -> LlmError {
        LlmError::Network(self.redact(&e.without_url().to_string(), key))
    }

    async fn open(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmChunk, LlmError>>, LlmError> {
        let key = self.key().await?;

        // System messages go in the top-level `system` field, not in `messages`.
        let mut system: Vec<&str> = req.system.iter().map(String::as_str).collect();
        system.extend(
            req.messages
                .iter()
                .filter(|m| m.role == Role::System)
                .map(|m| m.content.as_str()),
        );
        let body = Body {
            model: req.model.as_str(),
            max_tokens: req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            stream: true,
            system: (!system.is_empty()).then(|| system.join("\n\n")),
            messages: req
                .messages
                .iter()
                .filter(|m| m.role != Role::System)
                .map(|m| Message {
                    role: if m.role == Role::Assistant {
                        "assistant"
                    } else {
                        "user"
                    },
                    content: &m.content,
                })
                .collect(),
            temperature: req.temperature,
        };

        let response = self
            .http
            .post(self.url("/v1/messages"))
            .headers(self.headers(&key)?)
            .json(&body)
            .send()
            .await
            .map_err(|e| self.network(e, &key))?;
        let response = self.check(response, &key).await?;

        let bytes = response
            .bytes_stream()
            .map(move |r| r.map_err(|e| LlmError::Network(e.without_url().to_string())));
        Ok(llm_stream(bytes, AnthropicDecoder::new()))
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("anthropic")
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let key = self.inner.key().await?;
        let response = self
            .inner
            .http
            .get(self.inner.url("/v1/models?limit=1000"))
            .headers(self.inner.headers(&key)?)
            .send()
            .await
            .map_err(|e| self.inner.network(e, &key))?;
        let response = self.inner.check(response, &key).await?;
        let body: ModelsBody = response
            .json()
            .await
            .map_err(|e| LlmError::Malformed(format!("model list: {}", e.without_url())))?;
        Ok(body
            .data
            .into_iter()
            .map(|m| ModelInfo {
                display_name: m.display_name.unwrap_or_else(|| m.id.clone()),
                id: ModelId::new(m.id),
            })
            .collect())
    }

    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut stream = self.stream(req);
        let mut text = String::new();
        let mut usage: Option<Usage> = None;
        while let Some(chunk) = stream.next().await {
            match chunk? {
                LlmChunk::Text(t) => text.push_str(&t),
                LlmChunk::Usage(u) => usage = Some(u),
                LlmChunk::Done => break,
            }
        }
        Ok(LlmResponse { text, usage })
    }

    fn stream(&self, req: LlmRequest) -> BoxStream<'static, Result<LlmChunk, LlmError>> {
        let inner = Arc::clone(&self.inner);
        stream::once(async move { inner.open(req).await })
            .flat_map(|opened| match opened {
                Ok(s) => s,
                Err(e) => stream::once(futures::future::ready(Err(e))).boxed(),
            })
            .boxed()
    }
}
