//! OpenAI adapter: implements `LlmProvider` over Chat Completions with SSE streaming. A
//! configurable base URL and provider id make it work with OpenAI-compatible servers (vLLM,
//! LM Studio, LiteLLM, ...), which may not need an API key.
//!
//! When a key is configured it is read from the `SecretStore` on every request and never
//! stored, logged or included in error messages.

mod decoder;
mod provider;

pub use provider::{MaxTokensParam, OpenAiConfig, OpenAiProvider};
