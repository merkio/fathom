//! Anthropic (Claude) adapter: implements `LlmProvider` over the Messages API with SSE streaming.
//!
//! The API key is read from the `SecretStore` on every request and never stored, logged or
//! included in error messages.

mod decoder;
mod provider;

pub use provider::{AnthropicConfig, AnthropicProvider};
