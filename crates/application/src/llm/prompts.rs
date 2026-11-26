//! Versioned, dialect-aware prompt templates.
//!
//! Everything sent to a provider is built here, so "what leaves the machine" is one reviewable
//! place. Prompts contain the dialect, the pruned schema (names, types, keys; never row data)
//! and the user's task. Sample rows appear only when the caller passes them explicitly, which
//! the UI does only when the user opted in for that request.

use domain::{ChatMessage, Dialect, LimitStyle, QueryResult, Role, Value};

use super::context::SchemaContext;

/// A prompt ready to be sent: the template that produced it, the system text and the messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// Template name and version, for example `generate_sql/v1`. Bump the version on any
    /// change to a template's wording.
    pub template: &'static str,
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

pub struct PromptContext<'a> {
    pub dialect: &'a Dialect,
    pub schema: &'a SchemaContext,
    /// Present only if the user chose to share sample rows with this request.
    pub sample_rows: Option<&'a QueryResult>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PromptBuilder;

const MAX_SAMPLE_ROWS: usize = 5;
const MAX_CELL_CHARS: usize = 80;

fn limit_hint(dialect: &Dialect) -> &'static str {
    match dialect.limit_style {
        LimitStyle::Limit => "LIMIT n",
        LimitStyle::FetchFirst => "FETCH FIRST n ROWS ONLY",
        LimitStyle::Top => "SELECT TOP n",
    }
}

fn cell(value: &Value) -> String {
    let text = match value {
        Value::Bytes(b) => format!("<{} bytes>", b.len()),
        other => other.display(),
    };
    if text.chars().count() > MAX_CELL_CHARS {
        let cut: String = text.chars().take(MAX_CELL_CHARS).collect();
        format!("{cut}...")
    } else {
        text
    }
}

fn sample_block(rows: &QueryResult) -> String {
    let header: Vec<&str> = rows.columns.iter().map(|c| c.name.as_str()).collect();
    let mut out = format!(
        "\nSample rows (the user chose to share these with this request):\n{}\n",
        header.join(" | ")
    );
    for row in rows.rows.iter().take(MAX_SAMPLE_ROWS) {
        out.push_str(&row.iter().map(cell).collect::<Vec<_>>().join(" | "));
        out.push('\n');
    }
    out
}

fn system(ctx: &PromptContext<'_>, task: &str) -> String {
    let q = ctx.dialect.identifier_quote;
    let schema = if ctx.schema.text.is_empty() {
        "(no tables)"
    } else {
        ctx.schema.text.as_str()
    };
    let mut s = format!(
        "You are a database assistant inside a SQL client.\n\
         Database: {name}. Quote identifiers with {q}. Row limits use {limit}.\n\n\
         Schema (names and types only, no data):\n{schema}\n",
        name = ctx.dialect.name,
        limit = limit_hint(ctx.dialect),
    );
    if let Some(rows) = ctx.sample_rows {
        s.push_str(&sample_block(rows));
    }
    s.push('\n');
    s.push_str(task);
    s
}

const SQL_FORMAT: &str = "Reply with exactly one SQL statement in a single ```sql fenced block, \
followed by a short plain-language explanation. Use only tables and columns that appear in the \
schema. If the request cannot be answered with this schema, say so instead of guessing.";

fn fenced(sql: &str) -> String {
    format!("```sql\n{}\n```", sql.trim())
}

impl PromptBuilder {
    pub fn generate_sql(
        &self,
        ctx: &PromptContext<'_>,
        question: &str,
        current_sql: Option<&str>,
    ) -> Prompt {
        let mut user = format!("Write a query for: {}", question.trim());
        if let Some(sql) = current_sql.filter(|s| !s.trim().is_empty()) {
            user.push_str(&format!(
                "\n\nThe editor currently contains:\n{}",
                fenced(sql)
            ));
        }
        Prompt {
            template: "generate_sql/v1",
            system: system(
                ctx,
                &format!(
                    "Task: translate the user's request into a query for this database. Prefer read-only queries unless the user clearly asks to change data.\n{SQL_FORMAT}"
                ),
            ),
            messages: vec![ChatMessage::user(user)],
        }
    }

    pub fn explain_sql(&self, ctx: &PromptContext<'_>, sql: &str) -> Prompt {
        Prompt {
            template: "explain_sql/v1",
            system: system(
                ctx,
                "Task: explain what the user's SQL does, step by step, in plain language. Mention anything surprising or risky. Do not rewrite the query.",
            ),
            messages: vec![ChatMessage::user(format!(
                "Explain this query:\n{}",
                fenced(sql)
            ))],
        }
    }

    pub fn optimize_sql(&self, ctx: &PromptContext<'_>, sql: &str) -> Prompt {
        Prompt {
            template: "optimize_sql/v1",
            system: system(
                ctx,
                &format!(
                    "Task: improve the performance and clarity of the user's query without changing its result. Use the keys shown in the schema when suggesting indexes or joins.\n{SQL_FORMAT} List the changes you made."
                ),
            ),
            messages: vec![ChatMessage::user(format!(
                "Optimize this query:\n{}",
                fenced(sql)
            ))],
        }
    }

    pub fn fix_sql(&self, ctx: &PromptContext<'_>, sql: &str, error: &str) -> Prompt {
        Prompt {
            template: "fix_sql/v1",
            system: system(
                ctx,
                &format!(
                    "Task: fix the user's query so it runs, given the database error. Change as little as possible.\n{SQL_FORMAT} Say what was wrong."
                ),
            ),
            messages: vec![ChatMessage::user(format!(
                "This query:\n{}\n\nfailed with this error:\n{}",
                fenced(sql),
                error.trim()
            ))],
        }
    }

    /// Multi-turn chat about the schema. `history` is the conversation so far (user and
    /// assistant turns); the new `question` is appended as the final user message.
    pub fn chat(&self, ctx: &PromptContext<'_>, history: &[ChatMessage], question: &str) -> Prompt {
        let mut messages: Vec<ChatMessage> = history
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        messages.push(ChatMessage::user(question.trim()));
        Prompt {
            template: "chat/v1",
            system: system(
                ctx,
                "Task: answer questions about this database and help the user work with it. When you show SQL, put it in a ```sql fenced block. Be concise.",
            ),
            messages,
        }
    }
}
