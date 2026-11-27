use std::sync::Arc;
use std::time::Duration;

use application::ports::{
    LlmChunk, LlmError, LlmProvider, LlmRequest, LlmResponse, ModelInfo, Usage,
};
use application::streaming::{error_for_status, llm_stream};
use async_trait::async_trait;
use domain::{ModelId, ProviderId, Role};
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::decoder::OllamaDecoder;

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub base_url: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }
}

impl OllamaConfig {
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }
}

pub struct OllamaProvider {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    config: OllamaConfig,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
}

#[derive(Deserialize)]
struct TagsBody {
    models: Vec<TagEntry>,
}

#[derive(Deserialize)]
struct TagEntry {
    name: String,
}

impl OllamaProvider {
    pub fn new(config: OllamaConfig) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .user_agent("fathom")
            .connect_timeout(Duration::from_secs(10))
            // Local models can take a while to load before the first token.
            .read_timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| LlmError::Network(e.without_url().to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner { http, config }),
        })
    }
}

fn network(e: reqwest::Error) -> LlmError {
    LlmError::Network(e.without_url().to_string())
}

impl Inner {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url.trim_end_matches('/'))
    }

    async fn check(&self, response: reqwest::Response) -> Result<reqwest::Response, LlmError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ErrorBody>(&body)
            .ok()
            .and_then(|b| b.error)
            .unwrap_or(body);
        Err(error_for_status(status.as_u16(), Some(&message)))
    }

    fn body(req: &LlmRequest) -> Value {
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
        let mut options = Map::new();
        if let Some(t) = req.temperature {
            options.insert("temperature".into(), t.into());
        }
        if let Some(n) = req.max_tokens {
            options.insert("num_predict".into(), n.into());
        }
        let mut body = Map::new();
        body.insert("model".into(), req.model.as_str().into());
        body.insert("stream".into(), true.into());
        body.insert(
            "messages".into(),
            serde_json::to_value(messages).unwrap_or(Value::Null),
        );
        if !options.is_empty() {
            body.insert("options".into(), Value::Object(options));
        }
        Value::Object(body)
    }

    async fn open(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmChunk, LlmError>>, LlmError> {
        let response = self
            .http
            .post(self.url("/api/chat"))
            .json(&Self::body(&req))
            .send()
            .await
            .map_err(network)?;
        let response = self.check(response).await?;
        let bytes = response.bytes_stream().map(|r| r.map_err(network));
        Ok(llm_stream(bytes, OllamaDecoder::new()))
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("ollama")
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let response = self
            .inner
            .http
            .get(self.inner.url("/api/tags"))
            .send()
            .await
            .map_err(network)?;
        let response = self.inner.check(response).await?;
        let body: TagsBody = response
            .json()
            .await
            .map_err(|e| LlmError::Malformed(format!("model list: {}", e.without_url())))?;
        Ok(body
            .models
            .into_iter()
            .map(|m| ModelInfo {
                display_name: m.name.clone(),
                id: ModelId::new(m.name),
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
