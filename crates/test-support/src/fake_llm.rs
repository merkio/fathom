use std::sync::{Arc, Mutex};

use application::ports::{
    LlmChunk, LlmError, LlmProvider, LlmRequest, LlmResponse, ModelInfo, Usage,
};
use async_trait::async_trait;
use domain::{ModelId, ProviderId};
use futures::stream::{self, BoxStream, StreamExt};

use crate::lock;

/// Scripted LLM provider: replies with fixed text, split into chunks when streaming.
#[derive(Clone)]
pub struct FakeLlm {
    id: ProviderId,
    chunks: Vec<String>,
    usage: Option<Usage>,
    models: Vec<String>,
    error: Option<LlmError>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

impl FakeLlm {
    pub fn new(id: &str, chunks: &[&str]) -> Self {
        Self {
            id: ProviderId::new(id),
            chunks: chunks.iter().map(|c| (*c).to_owned()).collect(),
            usage: None,
            models: vec!["fake-model".to_owned()],
            error: None,
            requests: Arc::default(),
        }
    }

    /// Token usage reported with every reply (as a `Usage` chunk when streaming).
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_models(mut self, models: &[&str]) -> Self {
        self.models = models.iter().map(|m| (*m).to_owned()).collect();
        self
    }

    pub fn failing(mut self, error: LlmError) -> Self {
        self.error = Some(error);
        self
    }

    /// Requests received so far, in order. Lets tests assert what would leave the machine.
    pub fn requests(&self) -> Vec<LlmRequest> {
        lock(&self.requests).clone()
    }
}

#[async_trait]
impl LlmProvider for FakeLlm {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        Ok(self
            .models
            .iter()
            .map(|m| ModelInfo {
                id: ModelId::new(m.clone()),
                display_name: m.clone(),
            })
            .collect())
    }

    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
        lock(&self.requests).push(req);
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        Ok(LlmResponse {
            text: self.chunks.concat(),
            usage: self.usage,
        })
    }

    fn stream(&self, req: LlmRequest) -> BoxStream<'static, Result<LlmChunk, LlmError>> {
        lock(&self.requests).push(req);
        if let Some(e) = &self.error {
            return stream::once(futures::future::ready(Err(e.clone()))).boxed();
        }
        let items: Vec<Result<LlmChunk, LlmError>> = self
            .chunks
            .iter()
            .map(|c| Ok(LlmChunk::Text(c.clone())))
            .chain(self.usage.map(|u| Ok(LlmChunk::Usage(u))))
            .chain([Ok(LlmChunk::Done)])
            .collect();
        stream::iter(items).boxed()
    }
}

#[cfg(test)]
mod tests {
    use application::registry::LlmRegistry;
    use domain::ChatMessage;
    use futures::executor::block_on;

    use super::*;

    fn request() -> LlmRequest {
        LlmRequest {
            model: ModelId::new("fake-model"),
            system: None,
            messages: vec![ChatMessage::user("hi")],
            max_tokens: None,
            temperature: None,
        }
    }

    #[test]
    fn stream_yields_text_then_done() {
        let llm = FakeLlm::new("fake", &["SELECT ", "1"]);
        let chunks: Vec<_> = block_on(llm.stream(request()).collect());
        assert_eq!(
            chunks,
            vec![
                Ok(LlmChunk::Text("SELECT ".into())),
                Ok(LlmChunk::Text("1".into())),
                Ok(LlmChunk::Done)
            ]
        );
        assert_eq!(llm.requests().len(), 1);
    }

    #[test]
    fn complete_concatenates_and_errors_are_scriptable() {
        let llm = FakeLlm::new("fake", &["a", "b"]);
        assert_eq!(
            block_on(llm.complete(request())).map(|r| r.text),
            Ok("ab".to_string())
        );
        let bad = FakeLlm::new("fake", &[]).failing(LlmError::RateLimited);
        assert_eq!(
            block_on(bad.complete(request())).err(),
            Some(LlmError::RateLimited)
        );
    }

    #[test]
    fn registry_resolves_provider_by_id() {
        let mut registry = LlmRegistry::new();
        registry.register(std::sync::Arc::new(FakeLlm::new("fake", &[])));
        assert!(registry.get(&ProviderId::new("fake")).is_some());
        assert_eq!(registry.ids(), vec![ProviderId::new("fake")]);
    }
}
