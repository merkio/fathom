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
use serde_json::{Map, Value};

use crate::decoder::OpenAiDecoder;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Which JSON field carries the output limit. Newer OpenAI models require
/// `max_completion_tokens`; most compatible servers only know `max_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxTokensParam {
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Provider id shown to the user; use a distinct one per compatible server.
    pub id: ProviderId,
    /// Base URL including the version path, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Where the API key lives in the `SecretStore`. `None` for servers without authentication.
    pub api_key: Option<SecretRef>,
    pub max_tokens_param: MaxTokensParam,
}

impl OpenAiConfig {
    pub fn new(api_key: SecretRef) -> Self {
        Self {
            id: ProviderId::new("openai"),
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key: Some(api_key),
            max_tokens_param: MaxTokensParam::MaxTokens,
        }
    }

    /// An OpenAI-compatible server. Pass `None` when it needs no key.
    pub fn compatible(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<SecretRef>,
    ) -> Self {
        Self {
            id: ProviderId::new(id),
            base_url: base_url.into(),
            api_key,
            max_tokens_param: MaxTokensParam::MaxTokens,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_max_tokens_param(mut self, param: MaxTokensParam) -> Self {
        self.max_tokens_param = param;
        self
    }
}

pub struct OpenAiProvider {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    config: OpenAiConfig,
    secrets: Arc<dyn SecretStore>,
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
}

impl OpenAiProvider {
    pub fn new(config: OpenAiConfig, secrets: Arc<dyn SecretStore>) -> Result<Self, LlmError> {
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
    /// `Ok(None)` for servers configured without a key.
    async fn key(&self) -> Result<Option<SecretString>, LlmError> {
        let Some(reference) = &self.config.api_key else {
            return Ok(None);
        };
        match self.secrets.get(reference).await {
            Ok(Some(key)) => Ok(Some(key)),
            Ok(None) => Err(LlmError::Authentication),
            Err(e) => Err(LlmError::Provider(format!("secret store: {e}"))),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url.trim_end_matches('/'))
    }

    fn redact(&self, text: &str, key: Option<&SecretString>) -> String {
        match key.map(ExposeSecret::expose_secret) {
            Some(secret) if !secret.is_empty() => text.replace(secret, "[redacted]"),
            _ => text.to_owned(),
        }
    }

    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
        key: Option<&SecretString>,
    ) -> Result<reqwest::RequestBuilder, LlmError> {
        let Some(key) = key else { return Ok(request) };
        let mut value = HeaderValue::from_str(&format!("Bearer {}", key.expose_secret()))
            .map_err(|_| LlmError::Authentication)?;
        value.set_sensitive(true);
        Ok(request.header(reqwest::header::AUTHORIZATION, value))
    }

    async fn check(
        &self,
        response: reqwest::Response,
        key: Option<&SecretString>,
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

    fn network(&self, e: reqwest::Error, key: Option<&SecretString>) -> LlmError {
        LlmError::Network(self.redact(&e.without_url().to_string(), key))
    }

    fn body(&self, req: &LlmRequest) -> Value {
        let mut messages: Vec<Message<'_>> = Vec::new();
        if let Some(system) = &req.system {
            messages.push(Message {
                role: "system",
                content: system,
            });
        }
        for m in &req.messages {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            messages.push(Message {
                role,
                content: &m.content,
            });
        }
        let mut body = Map::new();
        body.insert("model".into(), req.model.as_str().into());
        body.insert("stream".into(), true.into());
        body.insert(
            "stream_options".into(),
            serde_json::json!({"include_usage": true}),
        );
        body.insert(
            "messages".into(),
            serde_json::to_value(messages).unwrap_or(Value::Null),
        );
        if let Some(max) = req.max_tokens {
            let field = match self.config.max_tokens_param {
                MaxTokensParam::MaxTokens => "max_tokens",
                MaxTokensParam::MaxCompletionTokens => "max_completion_tokens",
            };
            body.insert(field.into(), max.into());
        }
        if let Some(t) = req.temperature {
            body.insert("temperature".into(), t.into());
        }
        Value::Object(body)
    }

    async fn open(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmChunk, LlmError>>, LlmError> {
        let key = self.key().await?;
        let request = self
            .http
            .post(self.url("/chat/completions"))
            .json(&self.body(&req));
        let response = self
            .authorize(request, key.as_ref())?
            .send()
            .await
            .map_err(|e| self.network(e, key.as_ref()))?;
        let response = self.check(response, key.as_ref()).await?;
        let bytes = response
            .bytes_stream()
            .map(|r| r.map_err(|e| LlmError::Network(e.without_url().to_string())));
        Ok(llm_stream(bytes, OpenAiDecoder::new()))
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        self.inner.config.id.clone()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let key = self.inner.key().await?;
        let request = self.inner.http.get(self.inner.url("/models"));
        let response = self
            .inner
            .authorize(request, key.as_ref())?
            .send()
            .await
            .map_err(|e| self.inner.network(e, key.as_ref()))?;
        let response = self.inner.check(response, key.as_ref()).await?;
        let body: ModelsBody = response
            .json()
            .await
            .map_err(|e| LlmError::Malformed(format!("model list: {}", e.without_url())))?;
        Ok(body
            .data
            .into_iter()
            .map(|m| ModelInfo {
                display_name: m.id.clone(),
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
