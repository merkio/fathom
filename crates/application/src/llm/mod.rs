//! LLM support: safety classification of generated SQL, schema context, prompts and parsing.
//! Pure functions and data; the providers themselves are adapters behind the `LlmProvider` port.

pub mod context;
pub mod prompts;
pub mod reply;
pub mod safety;

pub use context::{SchemaContext, SchemaContextBuilder};
pub use prompts::{Prompt, PromptBuilder, PromptContext};
pub use reply::{Reply, parse_reply};
pub use safety::{SafetyReport, SqlSafetyGuard, StatementKind};
