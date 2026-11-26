use async_trait::async_trait;
use domain::{ChatMessage, ModelId, ProviderId};
use futures::stream::BoxStream;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LlmError {
    #[error("authentication failed")]
    Authentication,
    #[error("rate limited")]
    RateLimited,
    #[error("network error: {0}")]
    Network(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("malformed response: {0}")]
    Malformed(String),
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: ModelId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmRequest {
    pub model: ModelId,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmResponse {
    pub text: String,
    pub usage: Option<Usage>,
}

/// Normalized streaming chunk. Provider quirks stay inside adapters.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmChunk {
    Text(String),
    Usage(Usage),
    Done,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError>;
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    fn stream(&self, req: LlmRequest) -> BoxStream<'static, Result<LlmChunk, LlmError>>;
}
