//! LLM assistance: generate, explain, optimize and fix SQL, and chat about the schema.
//!
//! Replies stream. Nothing here executes SQL: generated SQL is parsed, classified by
//! `SqlSafetyGuard` and handed to the UI, which puts it in the editor for the user to run.

use std::sync::Arc;

use domain::{ChatMessage, Dialect, ModelId, ProviderId, QueryResult, SchemaTree};
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::UseCaseError;
use crate::llm::{
    Prompt, PromptBuilder, PromptContext, SafetyReport, SchemaContext, SchemaContextBuilder,
    SqlSafetyGuard, parse_reply,
};
use crate::ports::{LlmChunk, LlmError, LlmRequest, Usage};
use crate::registry::LlmRegistry;

pub enum AssistTask<'a> {
    Generate {
        question: &'a str,
        current_sql: Option<&'a str>,
    },
    Explain {
        sql: &'a str,
    },
    Optimize {
        sql: &'a str,
    },
    Fix {
        sql: &'a str,
        error: &'a str,
    },
    Chat {
        history: &'a [ChatMessage],
        question: &'a str,
    },
}

pub struct AssistRequest<'a> {
    pub provider: ProviderId,
    pub model: ModelId,
    pub dialect: &'a Dialect,
    pub schema: &'a SchemaTree,
    pub task: AssistTask<'a>,
    /// Row data. `Some` only when the user opted in to share it with this request.
    pub sample_rows: Option<&'a QueryResult>,
    pub max_tokens: Option<u32>,
}

/// Exactly what a request will send, for the privacy preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPreview {
    pub provider: ProviderId,
    pub model: ModelId,
    pub template: &'static str,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub includes_sample_rows: bool,
    pub schema_tables_sent: usize,
    pub schema_tables_total: usize,
}

/// A finished reply, parsed and classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistReply {
    pub text: String,
    pub sql: Option<String>,
    pub explanation: String,
    /// Classification of `sql`; drives the read/write/DDL badge.
    pub safety: Option<SafetyReport>,
    pub warning: Option<String>,
    pub usage: Option<Usage>,
}

pub struct Assistant {
    providers: Arc<LlmRegistry>,
    prompts: PromptBuilder,
    context: SchemaContextBuilder,
    guard: SqlSafetyGuard,
}

const DEFAULT_MAX_TOKENS: u32 = 2048;

impl Assistant {
    pub fn new(providers: Arc<LlmRegistry>, schema_token_budget: usize) -> Self {
        Self {
            providers,
            prompts: PromptBuilder,
            context: SchemaContextBuilder::new(schema_token_budget),
            guard: SqlSafetyGuard,
        }
    }

    fn build(&self, req: &AssistRequest<'_>) -> (Prompt, SchemaContext) {
        // Relevance hints: the user's own words and SQL, never data.
        let hints = match &req.task {
            AssistTask::Generate {
                question,
                current_sql,
            } => {
                format!("{question} {}", current_sql.unwrap_or_default())
            }
            AssistTask::Explain { sql } | AssistTask::Optimize { sql } => (*sql).to_owned(),
            AssistTask::Fix { sql, error } => format!("{sql} {error}"),
            AssistTask::Chat { history, question } => {
                let past: Vec<&str> = history.iter().map(|m| m.content.as_str()).collect();
                format!("{question} {}", past.join(" "))
            }
        };
        let schema = self.context.build(req.schema, &hints);
        let ctx = PromptContext {
            dialect: req.dialect,
            schema: &schema,
            sample_rows: req.sample_rows,
        };
        let prompt = match &req.task {
            AssistTask::Generate {
                question,
                current_sql,
            } => self.prompts.generate_sql(&ctx, question, *current_sql),
            AssistTask::Explain { sql } => self.prompts.explain_sql(&ctx, sql),
            AssistTask::Optimize { sql } => self.prompts.optimize_sql(&ctx, sql),
            AssistTask::Fix { sql, error } => self.prompts.fix_sql(&ctx, sql, error),
            AssistTask::Chat { history, question } => self.prompts.chat(&ctx, history, question),
        };
        (prompt, schema)
    }

    /// What would be sent for this request. Pure: nothing is sent.
    pub fn preview(&self, req: &AssistRequest<'_>) -> PromptPreview {
        let (prompt, schema) = self.build(req);
        PromptPreview {
            provider: req.provider.clone(),
            model: req.model.clone(),
            template: prompt.template,
            system: prompt.system,
            messages: prompt.messages,
            includes_sample_rows: req.sample_rows.is_some(),
            schema_tables_sent: schema.included_tables,
            schema_tables_total: schema.total_tables,
        }
    }

    /// Starts the request and returns the streamed reply. Dropping the stream cancels it.
    pub fn stream(
        &self,
        req: &AssistRequest<'_>,
    ) -> Result<BoxStream<'static, Result<LlmChunk, LlmError>>, UseCaseError> {
        let provider = self
            .providers
            .get(&req.provider)
            .ok_or_else(|| UseCaseError::UnknownProvider(req.provider.clone()))?;
        let (prompt, _) = self.build(req);
        let temperature = match req.task {
            AssistTask::Explain { .. } | AssistTask::Chat { .. } => 0.3,
            _ => 0.0,
        };
        Ok(provider.stream(LlmRequest {
            model: req.model.clone(),
            system: Some(prompt.system),
            messages: prompt.messages,
            max_tokens: Some(req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
            temperature: Some(temperature),
        }))
    }

    /// Parses a (possibly partial) reply and classifies its SQL. Call it when the stream ends.
    pub fn finish(&self, text: &str, dialect: &Dialect, usage: Option<Usage>) -> AssistReply {
        let reply = parse_reply(text);
        let safety = reply
            .sql
            .as_deref()
            .map(|sql| self.guard.classify(sql, dialect));
        AssistReply {
            text: text.to_owned(),
            sql: reply.sql,
            explanation: reply.explanation,
            safety,
            warning: reply.warning,
            usage,
        }
    }

    /// Runs the request to completion. Convenience over `stream` + `finish`.
    pub async fn complete(&self, req: &AssistRequest<'_>) -> Result<AssistReply, UseCaseError> {
        let mut stream = self.stream(req)?;
        let mut text = String::new();
        let mut usage = None;
        while let Some(chunk) = stream.next().await {
            match chunk? {
                LlmChunk::Text(t) => text.push_str(&t),
                LlmChunk::Usage(u) => usage = Some(u),
                LlmChunk::Done => break,
            }
        }
        Ok(self.finish(&text, req.dialect, usage))
    }
}
