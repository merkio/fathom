//! LLM inline suggestions ("ghost text"): opt-in, off by default.
//!
//! The setting is read on every call, inside this use case, so nothing can reach a provider
//! while suggestions are off no matter what the caller does. What is sent: the dialect, the
//! schema pruned to the tables the editor mentions, and the text around the cursor (limited).
//! Never row data.

use std::ops::Range;
use std::sync::Arc;

use domain::{Dialect, ModelId, ProviderId, SchemaTree};
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::UseCaseError;
use crate::llm::{
    PromptBuilder, PromptContext, SchemaContextBuilder, SqlSafetyGuard, StatementKind,
};
use crate::ports::{LlmChunk, LlmError, LlmRequest, SettingsRepository, StorageError};
use crate::registry::LlmRegistry;
use crate::sql::{Suggestion, SuggestionKind, SuggestionSource};

const ENABLED_KEY: &str = "ai.inline.enabled";
const PROVIDER_KEY: &str = "ai.inline.provider";
const MODEL_KEY: &str = "ai.inline.model";

/// Characters of editor text sent before and after the cursor.
const MAX_PREFIX_CHARS: usize = 2000;
const MAX_SUFFIX_CHARS: usize = 500;
/// A completion is a few words, not an essay.
const MAX_TOKENS: u32 = 64;
/// Smaller than the assistant's budget: inline suggestions must be fast.
const SCHEMA_TOKEN_BUDGET: usize = 800;

/// The user's inline-suggestion settings. `Default` is off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineSettings {
    pub enabled: bool,
    /// A small, fast model is recommended (a local Ollama model).
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
}

pub struct InlineSettingsStore {
    settings: Arc<dyn SettingsRepository>,
}

impl InlineSettingsStore {
    pub fn new(settings: Arc<dyn SettingsRepository>) -> Self {
        Self { settings }
    }

    pub async fn load(&self) -> Result<InlineSettings, StorageError> {
        let text = |v: Option<String>| v.filter(|s| !s.is_empty());
        Ok(InlineSettings {
            enabled: self.settings.get(ENABLED_KEY).await?.as_deref() == Some("true"),
            provider: text(self.settings.get(PROVIDER_KEY).await?).map(ProviderId::new),
            model: text(self.settings.get(MODEL_KEY).await?).map(ModelId::new),
        })
    }

    pub async fn save(&self, s: &InlineSettings) -> Result<(), StorageError> {
        self.settings
            .set(ENABLED_KEY, if s.enabled { "true" } else { "false" })
            .await?;
        self.settings
            .set(
                PROVIDER_KEY,
                s.provider.as_ref().map_or("", ProviderId::as_str),
            )
            .await?;
        self.settings
            .set(MODEL_KEY, s.model.as_ref().map_or("", ModelId::as_str))
            .await?;
        Ok(())
    }
}

pub struct InlineRequest<'a> {
    pub dialect: &'a Dialect,
    pub schema: &'a SchemaTree,
    /// The whole editor text and the cursor (byte offset) in it.
    pub text: &'a str,
    pub cursor: usize,
    /// Per-connection override: `Some(true)` or `Some(false)` wins over the global setting.
    pub connection_override: Option<bool>,
}

/// A suggestion with the safety label of the statement it would complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineCompletion {
    pub suggestion: Suggestion,
    /// Labelling only: suggestions never run, so this just informs the UI.
    pub safety: Option<StatementKind>,
}

pub struct SuggestInline {
    providers: Arc<LlmRegistry>,
    settings: InlineSettingsStore,
    prompts: PromptBuilder,
    context: SchemaContextBuilder,
    guard: SqlSafetyGuard,
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The last `max` characters of `text`.
fn tail(text: &str, max: usize) -> &str {
    let count = text.chars().count();
    if count <= max {
        return text;
    }
    let skip = count - max;
    text.char_indices()
        .nth(skip)
        .map_or(text, |(i, _)| &text[i..])
}

/// The first `max` characters of `text`.
fn head(text: &str, max: usize) -> &str {
    text.char_indices()
        .nth(max)
        .map_or(text, |(i, _)| &text[..i])
}

/// Models often wrap the answer in a fence or repeat the line they were given; undo that.
fn clean(raw: &str, prefix: &str) -> String {
    let mut text = raw.trim_start_matches(['\r', '\n']);
    if let Some(rest) = text.strip_prefix("```") {
        // Drop an opening fence line (``` or ```sql) and a closing fence.
        text = rest.split_once('\n').map_or("", |(_, body)| body);
    }
    let text = text.split("```").next().unwrap_or_default();
    // Repeating the current line: keep only what is new.
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    let current_line = &prefix[line_start..];
    let text = if current_line.trim().len() >= 3 {
        text.strip_prefix(current_line).unwrap_or(text)
    } else {
        text
    };
    let text = text.trim_end();
    if text.trim().is_empty() {
        String::new()
    } else {
        text.to_owned()
    }
}

impl SuggestInline {
    pub fn new(providers: Arc<LlmRegistry>, settings: Arc<dyn SettingsRepository>) -> Self {
        Self {
            providers,
            settings: InlineSettingsStore::new(settings),
            prompts: PromptBuilder,
            context: SchemaContextBuilder::new(SCHEMA_TOKEN_BUDGET),
            guard: SqlSafetyGuard,
        }
    }

    pub fn settings(&self) -> &InlineSettingsStore {
        &self.settings
    }

    /// Starts a suggestion. The stream yields the completion as it grows; dropping it cancels the
    /// request. Fails with `Blocked`, **without contacting any provider**, when suggestions are
    /// off for this connection.
    pub async fn start(
        &self,
        req: &InlineRequest<'_>,
    ) -> Result<BoxStream<'static, Result<InlineCompletion, LlmError>>, UseCaseError> {
        let settings = self.settings.load().await?;
        let enabled = req.connection_override.unwrap_or(settings.enabled);
        if !enabled {
            return Err(UseCaseError::Blocked("Inline suggestions are off".into()));
        }
        let (Some(provider_id), Some(model)) = (settings.provider, settings.model) else {
            return Err(UseCaseError::Blocked(
                "Choose a provider and model for inline suggestions".into(),
            ));
        };
        let provider = self
            .providers
            .get(&provider_id)
            .ok_or_else(|| UseCaseError::UnknownProvider(provider_id.clone()))?;

        let cursor = floor_boundary(req.text, req.cursor);
        let before = &req.text[..cursor];
        let after = &req.text[cursor..];
        let prefix = tail(before, MAX_PREFIX_CHARS).to_owned();
        let suffix = head(after, MAX_SUFFIX_CHARS).to_owned();

        let schema = self
            .context
            .build(req.schema, &format!("{prefix} {suffix}"));
        let ctx = PromptContext {
            dialect: req.dialect,
            schema: &schema,
            sample_rows: None,
        };
        let prompt = self.prompts.inline_sql(&ctx, &prefix, &suffix);

        let stream = provider.stream(LlmRequest {
            model,
            system: Some(prompt.system),
            messages: prompt.messages,
            max_tokens: Some(MAX_TOKENS),
            temperature: Some(0.0),
        });

        let dialect = *req.dialect;
        let guard = self.guard;
        let range: Range<usize> = cursor..cursor;
        let mut raw = String::new();
        Ok(stream
            .filter_map(move |chunk| {
                let item = match chunk {
                    Ok(LlmChunk::Text(t)) => {
                        raw.push_str(&t);
                        let text = clean(&raw, &prefix);
                        (!text.is_empty()).then(|| {
                            let statement = format!("{prefix}{text}{suffix}");
                            let safety = guard.classify(&statement, &dialect).overall();
                            Ok(InlineCompletion {
                                suggestion: Suggestion {
                                    range: range.clone(),
                                    text,
                                    kind: SuggestionKind::Inline,
                                    source: SuggestionSource::Llm,
                                    detail: None,
                                },
                                safety,
                            })
                        })
                    }
                    Ok(LlmChunk::Usage(_) | LlmChunk::Done) => None,
                    Err(e) => Some(Err(e)),
                };
                futures::future::ready(item)
            })
            .boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_fences_and_echoed_lines() {
        assert_eq!(clean("customers", "SELECT * FROM "), "customers");
        assert_eq!(clean("```sql\nSELECT 1\n```", ""), "SELECT 1");
        assert_eq!(clean("```\nid, name\n```\nbecause", "SELECT "), "id, name");
        assert_eq!(clean("\nWHERE id = 1", "SELECT * FROM t"), "WHERE id = 1");
        // The model repeated the line it was given.
        assert_eq!(
            clean("SELECT * FROM customers", "SELECT * FROM "),
            "customers"
        );
        assert_eq!(clean("   \n", ""), "");
        // A leading space is meaningful and kept.
        assert_eq!(clean(" JOIN orders", "FROM customers"), " JOIN orders");
    }

    #[test]
    fn tail_and_head_respect_char_boundaries() {
        assert_eq!(tail("héllo wörld", 5), "wörld");
        assert_eq!(head("héllo wörld", 5), "héllo");
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(head("abc", 10), "abc");
    }

    #[test]
    fn floor_boundary_never_splits_a_character() {
        assert_eq!(floor_boundary("é", 1), 0);
        assert_eq!(floor_boundary("abc", 99), 3);
    }
}
