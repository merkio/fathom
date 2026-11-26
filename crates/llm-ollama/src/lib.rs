//! Ollama adapter: implements `LlmProvider` over `/api/chat` with NDJSON streaming and
//! `/api/tags` for model listing. Ollama needs no API key. It usually runs on this machine, so
//! with the default base URL nothing leaves it.

mod decoder;
mod provider;

pub use provider::{OllamaConfig, OllamaProvider};
