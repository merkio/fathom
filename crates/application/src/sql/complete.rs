//! Schema-aware, offline, synchronous SQL completion.
//!
//! The text before the cursor decides what is suggested: tables after `FROM`/`JOIN`, columns of
//! the tables in scope after `SELECT`/`WHERE`/`ON`, a table's columns after `alias.`, foreign-key
//! join conditions after `ON`, and keywords or functions otherwise. Incomplete SQL is the normal
//! input, so nothing here assumes the statement is valid.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use domain::{Dialect, SchemaTree, Table, TableKind};

use super::lexer::{KeywordSet, Token, TokenKind, identifier_text, tokenize};
use super::scope::{TableRef, resolve, table_refs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionKind {
    Keyword,
    Function,
    Schema,
    Table,
    View,
    Column,
    /// A ready-made join condition such as `o.customer_id = c.id`.
    Join,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionSource {
    /// Computed on this machine from the cached schema.
    Local,
    /// Produced by an LLM (inline suggestions, M7b).
    Llm,
}

/// One completion. Editors replace `range` with `text`; both completion sources share this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Byte range of the input to replace.
    pub range: Range<usize>,
    pub text: String,
    pub kind: SuggestionKind,
    pub source: SuggestionSource,
    /// Extra information for display, such as a column's type or its table.
    pub detail: Option<String>,
}

pub struct CompletionRequest<'a> {
    pub sql: &'a str,
    /// Byte offset of the cursor.
    pub cursor: usize,
    pub schema: &'a SchemaTree,
    pub dialect: &'a Dialect,
    /// Recently executed statements, newest first; names used there rank higher.
    pub recent_queries: &'a [String],
}

const MAX_RESULTS: usize = 100;

/// Keywords after which a table is expected.
const TABLE_KEYWORDS: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE", "TABLE"];
/// Keywords after which an expression (column, function, value) is expected.
const EXPRESSION_KEYWORDS: &[&str] = &[
    "SELECT",
    "WHERE",
    "ON",
    "AND",
    "OR",
    "NOT",
    "BY",
    "SET",
    "HAVING",
    "DISTINCT",
    "WHEN",
    "THEN",
    "ELSE",
    "CASE",
    "IN",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "IS",
    "USING",
    "ALL",
    "ANY",
    "EXISTS",
    "RETURNING",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    Table { after_join: bool },
    Expression { after_on: bool },
    Keyword,
    Nothing,
}

struct Candidate {
    suggestion: Suggestion,
    rank: u8,
    boost: u32,
    sort_key: String,
}

pub fn complete(req: &CompletionRequest<'_>) -> Vec<Suggestion> {
    let sql = req.sql;
    let dialect = req.dialect;
    let mut cursor = req.cursor.min(sql.len());
    while !sql.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let tokens = tokenize(sql, dialect);

    // The token being typed, if any.
    let mut token_start = cursor;
    let mut prefix_start = cursor;
    let mut in_quote = false;
    let mut closing_quote_present = false;
    for t in &tokens {
        let r = &t.range;
        let touches = r.start < cursor && cursor <= r.end;
        let inside = r.start < cursor && (cursor < r.end || (cursor == r.end && !t.terminated));
        match t.kind {
            TokenKind::Word if touches => {
                token_start = r.start;
                prefix_start = r.start;
            }
            TokenKind::Number if touches => return Vec::new(),
            TokenKind::String | TokenKind::Comment if inside => return Vec::new(),
            TokenKind::QuotedIdent if inside => {
                token_start = r.start;
                prefix_start = r.start + dialect.identifier_quote.len_utf8();
                in_quote = true;
                closing_quote_present = t.terminated;
            }
            _ => {}
        }
    }
    let prefix = &sql[prefix_start..cursor.max(prefix_start)];

    // Significant tokens of the statement the cursor is in.
    let stmt_start = tokens
        .iter()
        .filter(|t| t.range.end <= token_start && t.text(sql) == ";" && t.kind == TokenKind::Punct)
        .map(|t| t.range.end)
        .next_back()
        .unwrap_or(0);
    let stmt_end = tokens
        .iter()
        .find(|t| t.range.start >= cursor && t.text(sql) == ";" && t.kind == TokenKind::Punct)
        .map_or(sql.len(), |t| t.range.start);
    let statement: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.is_significant() && t.range.start >= stmt_start && t.range.end <= stmt_end)
        .collect();
    let before: Vec<&Token> = statement
        .iter()
        .copied()
        .filter(|t| t.range.end <= token_start)
        .collect();

    let keywords = KeywordSet::new(dialect);
    let refs = table_refs(sql, &statement, dialect);
    let history = name_counts(req.recent_queries, dialect);
    let ctx = Ctx {
        dialect,
        schema: req.schema,
        prefix,
        history: &history,
    };
    let range = prefix_start..cursor.max(prefix_start);

    let mut out: Vec<Candidate> = Vec::new();
    let quote_style = if in_quote {
        QuoteStyle::InsideQuotes {
            closing_present: closing_quote_present,
        }
    } else {
        QuoteStyle::Bare
    };

    // `qualifier.` completion.
    let n = before.len();
    if n >= 2 && before[n - 1].text(sql) == "." && is_ident_token(before[n - 2], sql, &keywords) {
        let first = identifier_text(before[n - 2].text(sql), before[n - 2], dialect);
        let second = (n >= 4
            && before[n - 3].text(sql) == "."
            && is_ident_token(before[n - 4], sql, &keywords))
        .then(|| identifier_text(before[n - 4].text(sql), before[n - 4], dialect));
        qualified(
            &ctx,
            &refs,
            &first,
            second.as_deref(),
            &range,
            quote_style,
            &mut out,
        );
        return finish(out);
    }

    let position = position_of(&before, sql, &keywords, &statement);
    match position {
        Position::Nothing => return Vec::new(),
        Position::Table { after_join } => {
            let related = if after_join {
                related_tables(req.schema, &refs)
            } else {
                HashSet::new()
            };
            tables(&ctx, &related, &range, quote_style, &mut out);
        }
        Position::Expression { after_on } => {
            if after_on {
                join_conditions(&ctx, &refs, cursor, &range, &mut out);
            }
            columns(&ctx, &refs, &range, quote_style, &mut out);
            words(
                &ctx,
                dialect.functions,
                SuggestionKind::Function,
                2,
                true,
                &range,
                &mut out,
            );
            if !prefix.is_empty() {
                words(
                    &ctx,
                    dialect.keywords,
                    SuggestionKind::Keyword,
                    3,
                    false,
                    &range,
                    &mut out,
                );
            }
        }
        Position::Keyword => {
            words(
                &ctx,
                dialect.keywords,
                SuggestionKind::Keyword,
                0,
                false,
                &range,
                &mut out,
            );
        }
    }
    finish(out)
}

struct Ctx<'a> {
    dialect: &'a Dialect,
    schema: &'a SchemaTree,
    prefix: &'a str,
    history: &'a HashMap<String, u32>,
}

#[derive(Clone, Copy)]
enum QuoteStyle {
    Bare,
    InsideQuotes { closing_present: bool },
}

fn is_ident_token(t: &Token, sql: &str, keywords: &KeywordSet) -> bool {
    match t.kind {
        TokenKind::QuotedIdent => true,
        TokenKind::Word => !keywords.contains(t.text(sql)),
        _ => false,
    }
}

fn matches_prefix(name: &str, prefix: &str) -> bool {
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// Text to insert for an identifier, quoting it only when needed.
fn identifier(ctx: &Ctx<'_>, name: &str, style: QuoteStyle) -> String {
    let q = ctx.dialect.identifier_quote;
    match style {
        QuoteStyle::InsideQuotes { closing_present } => {
            let escaped = name.replace(q, &format!("{q}{q}"));
            if closing_present {
                escaped
            } else {
                format!("{escaped}{q}")
            }
        }
        QuoteStyle::Bare => {
            let plain = name
                .chars()
                .next()
                .is_some_and(|c| c == '_' || c.is_alphabetic())
                && name
                    .chars()
                    .all(|c| c == '_' || c == '$' || c.is_alphanumeric());
            let case_matters = ctx.dialect.quote_mixed_case && name.chars().any(char::is_uppercase);
            if plain && !case_matters {
                name.to_owned()
            } else {
                ctx.dialect.quote_identifier(name)
            }
        }
    }
}

fn boost(ctx: &Ctx<'_>, name: &str) -> u32 {
    ctx.history.get(&name.to_lowercase()).copied().unwrap_or(0)
}

// Private builder: its parameters are exactly the fields it fills.
#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Candidate>,
    range: &Range<usize>,
    text: String,
    kind: SuggestionKind,
    detail: Option<String>,
    rank: u8,
    boost: u32,
    sort_key: &str,
) {
    out.push(Candidate {
        suggestion: Suggestion {
            range: range.clone(),
            text,
            kind,
            source: SuggestionSource::Local,
            detail,
        },
        rank,
        boost,
        sort_key: sort_key.to_lowercase(),
    });
}

fn table_kind(t: &Table) -> SuggestionKind {
    match t.kind {
        TableKind::Table => SuggestionKind::Table,
        TableKind::View => SuggestionKind::View,
    }
}

fn tables(
    ctx: &Ctx<'_>,
    related: &HashSet<String>,
    range: &Range<usize>,
    style: QuoteStyle,
    out: &mut Vec<Candidate>,
) {
    // Qualify a table with its schema only when the bare name would be ambiguous.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, t) in ctx.schema.tables() {
        *counts.entry(t.name.to_lowercase()).or_default() += 1;
    }
    for (schema, t) in ctx.schema.tables() {
        if !matches_prefix(&t.name, ctx.prefix) {
            continue;
        }
        let ambiguous = counts.get(&t.name.to_lowercase()).copied().unwrap_or(0) > 1;
        let text = if ambiguous {
            format!(
                "{}.{}",
                identifier(ctx, schema, style),
                identifier(ctx, &t.name, style)
            )
        } else {
            identifier(ctx, &t.name, style)
        };
        let rank = if related.contains(&t.name.to_lowercase()) {
            0
        } else {
            1
        };
        push(
            out,
            range,
            text,
            table_kind(t),
            Some(schema.to_owned()),
            rank,
            boost(ctx, &t.name),
            &t.name,
        );
    }
    // Schema names help when the prefix is typed (`pub|` -> `public.`).
    let schema_count = ctx
        .schema
        .catalogs
        .iter()
        .map(|c| c.schemas.len())
        .sum::<usize>();
    if schema_count > 1 && !ctx.prefix.is_empty() {
        for s in ctx.schema.catalogs.iter().flat_map(|c| &c.schemas) {
            if matches_prefix(&s.name, ctx.prefix) {
                let text = format!("{}.", identifier(ctx, &s.name, style));
                push(
                    out,
                    range,
                    text,
                    SuggestionKind::Schema,
                    None,
                    2,
                    0,
                    &s.name,
                );
            }
        }
    }
}

fn columns(
    ctx: &Ctx<'_>,
    refs: &[TableRef],
    range: &Range<usize>,
    style: QuoteStyle,
    out: &mut Vec<Candidate>,
) {
    let resolved: Vec<(&TableRef, &Table)> = refs
        .iter()
        .filter_map(|r| resolve(ctx.schema, r).map(|(_, t)| (r, t)))
        .collect();
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    for (_, t) in &resolved {
        for c in &t.columns {
            *occurrences.entry(c.name.to_lowercase()).or_default() += 1;
        }
    }
    for (r, t) in &resolved {
        for c in &t.columns {
            if !matches_prefix(&c.name, ctx.prefix) {
                continue;
            }
            let ambiguous = occurrences
                .get(&c.name.to_lowercase())
                .copied()
                .unwrap_or(0)
                > 1;
            let name = identifier(ctx, &c.name, style);
            let text = if ambiguous && matches!(style, QuoteStyle::Bare) {
                format!("{}.{name}", identifier(ctx, r.qualifier(), style))
            } else {
                name
            };
            let detail = format!("{} ({})", c.data_type.0, t.name);
            push(
                out,
                range,
                text,
                SuggestionKind::Column,
                Some(detail),
                1,
                boost(ctx, &c.name),
                &c.name,
            );
        }
    }
}

/// Foreign-key based join conditions for the table introduced by the last `JOIN` before the cursor.
fn join_conditions(
    ctx: &Ctx<'_>,
    refs: &[TableRef],
    cursor: usize,
    range: &Range<usize>,
    out: &mut Vec<Candidate>,
) {
    let Some(joined) = refs.iter().rfind(|r| r.via_join && r.pos < cursor) else {
        return;
    };
    let Some((_, joined_table)) = resolve(ctx.schema, joined) else {
        return;
    };
    for other in refs.iter().filter(|r| r.pos < joined.pos) {
        let Some((_, other_table)) = resolve(ctx.schema, other) else {
            continue;
        };
        let (jq, oq) = (joined.qualifier(), other.qualifier());
        let mut conditions: Vec<String> = Vec::new();
        for fk in &joined_table.foreign_keys {
            if fk.referenced_table.eq_ignore_ascii_case(&other_table.name) {
                conditions.push(pairs(jq, &fk.columns, oq, &fk.referenced_columns));
            }
        }
        for fk in &other_table.foreign_keys {
            if fk.referenced_table.eq_ignore_ascii_case(&joined_table.name) {
                conditions.push(pairs(oq, &fk.columns, jq, &fk.referenced_columns));
            }
        }
        for text in conditions {
            if matches_prefix(&text, ctx.prefix) {
                let detail = Some("foreign key".to_owned());
                push(
                    out,
                    range,
                    text.clone(),
                    SuggestionKind::Join,
                    detail,
                    0,
                    0,
                    &text,
                );
            }
        }
    }
}

fn pairs(left_q: &str, left: &[String], right_q: &str, right: &[String]) -> String {
    left.iter()
        .zip(right)
        .map(|(l, r)| format!("{left_q}.{l} = {right_q}.{r}"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Tables linked by a foreign key to a table already in the statement.
fn related_tables(tree: &SchemaTree, refs: &[TableRef]) -> HashSet<String> {
    let mut related = HashSet::new();
    for r in refs {
        let Some((_, t)) = resolve(tree, r) else {
            continue;
        };
        for fk in &t.foreign_keys {
            related.insert(fk.referenced_table.to_lowercase());
        }
        for (_, other) in tree.tables() {
            if other
                .foreign_keys
                .iter()
                .any(|fk| fk.referenced_table.eq_ignore_ascii_case(&t.name))
            {
                related.insert(other.name.to_lowercase());
            }
        }
    }
    related
}

fn qualified(
    ctx: &Ctx<'_>,
    refs: &[TableRef],
    first: &str,
    schema_qualifier: Option<&str>,
    range: &Range<usize>,
    style: QuoteStyle,
    out: &mut Vec<Candidate>,
) {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    // `schema.table.` -> that table's columns.
    let table: Option<&Table> = if let Some(schema) = schema_qualifier {
        ctx.schema
            .tables()
            .find(|(s, t)| eq(s, schema) && eq(&t.name, first))
            .map(|(_, t)| t)
    } else {
        // An alias or table name from the statement, then any table with that name.
        refs.iter()
            .find(|r| r.alias.as_deref().is_some_and(|a| eq(a, first)))
            .or_else(|| {
                refs.iter()
                    .find(|r| r.alias.is_none() && eq(&r.name, first))
            })
            .or_else(|| refs.iter().find(|r| eq(&r.name, first)))
            .and_then(|r| resolve(ctx.schema, r).map(|(_, t)| t))
            .or_else(|| ctx.schema.find_table(first).map(|(_, t)| t))
    };
    if let Some(t) = table {
        for c in &t.columns {
            if matches_prefix(&c.name, ctx.prefix) {
                let text = identifier(ctx, &c.name, style);
                let detail = Some(c.data_type.0.clone());
                push(
                    out,
                    range,
                    text,
                    SuggestionKind::Column,
                    detail,
                    0,
                    boost(ctx, &c.name),
                    &c.name,
                );
            }
        }
        return;
    }
    // `schema.` -> its tables.
    if schema_qualifier.is_none() {
        for s in ctx
            .schema
            .catalogs
            .iter()
            .flat_map(|c| &c.schemas)
            .filter(|s| eq(&s.name, first))
        {
            for t in &s.tables {
                if matches_prefix(&t.name, ctx.prefix) {
                    let text = identifier(ctx, &t.name, style);
                    push(
                        out,
                        range,
                        text,
                        table_kind(t),
                        None,
                        0,
                        boost(ctx, &t.name),
                        &t.name,
                    );
                }
            }
        }
    }
}

fn words(
    ctx: &Ctx<'_>,
    list: &[&str],
    kind: SuggestionKind,
    rank: u8,
    call: bool,
    range: &Range<usize>,
    out: &mut Vec<Candidate>,
) {
    let lower = ctx.prefix.chars().next().is_some_and(char::is_lowercase);
    for word in list {
        if !matches_prefix(word, ctx.prefix) {
            continue;
        }
        let mut text = if lower {
            word.to_lowercase()
        } else {
            word.to_uppercase()
        };
        if call {
            text.push('(');
        }
        push(out, range, text, kind, None, rank, 0, word);
    }
}

/// Decides what kind of thing is expected next from the tokens before the cursor.
fn position_of(
    before: &[&Token],
    sql: &str,
    keywords: &KeywordSet,
    statement: &[&Token],
) -> Position {
    let _ = statement;
    let Some(prev) = before.last() else {
        return Position::Keyword;
    };
    let prev_text = prev.text(sql);
    let upper = prev_text.to_uppercase();
    match prev.kind {
        TokenKind::Word if TABLE_KEYWORDS.contains(&upper.as_str()) => Position::Table {
            after_join: upper == "JOIN",
        },
        TokenKind::Word if upper == "AS" => Position::Nothing,
        TokenKind::Word if EXPRESSION_KEYWORDS.contains(&upper.as_str()) => Position::Expression {
            after_on: upper == "ON",
        },
        TokenKind::Word | TokenKind::QuotedIdent | TokenKind::String | TokenKind::Number => {
            let _ = keywords;
            Position::Keyword
        }
        TokenKind::Punct => match prev_text {
            "," => {
                if main_clause(before, sql).is_some_and(|c| c == "FROM") {
                    Position::Table { after_join: false }
                } else {
                    Position::Expression { after_on: false }
                }
            }
            ")" => Position::Keyword,
            "*" => {
                let before_star = before.len().checked_sub(2).map(|i| before[i]);
                let wildcard = before_star.is_none_or(|t| {
                    let tx = t.text(sql).to_uppercase();
                    matches!(tx.as_str(), "SELECT" | "DISTINCT" | "," | "." | "(")
                });
                if wildcard {
                    Position::Keyword
                } else {
                    Position::Expression { after_on: false }
                }
            }
            ";" => Position::Keyword,
            _ => Position::Expression { after_on: false },
        },
        TokenKind::Whitespace | TokenKind::Comment => Position::Keyword,
    }
}

/// The nearest clause keyword at the current parenthesis depth.
fn main_clause(before: &[&Token], sql: &str) -> Option<String> {
    const CLAUSES: &[&str] = &[
        "SELECT", "FROM", "JOIN", "ON", "WHERE", "GROUP", "ORDER", "HAVING", "SET", "UPDATE",
        "INTO", "VALUES",
    ];
    let mut depth = 0usize;
    for t in before.iter().rev() {
        match t.text(sql) {
            ")" => depth += 1,
            "(" => depth = depth.saturating_sub(1),
            w if depth == 0 && t.kind == TokenKind::Word => {
                let up = w.to_uppercase();
                if CLAUSES.contains(&up.as_str()) {
                    return Some(up);
                }
            }
            _ => {}
        }
    }
    None
}

/// How often each identifier appears in recent statements (lowercased).
fn name_counts(recent: &[String], dialect: &Dialect) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for query in recent {
        for t in tokenize(query, dialect) {
            if matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent) {
                let name = identifier_text(t.text(query), &t, dialect).to_lowercase();
                *counts.entry(name).or_default() += 1;
            }
        }
    }
    counts
}

fn finish(mut candidates: Vec<Candidate>) -> Vec<Suggestion> {
    candidates.sort_by(|a, b| {
        (a.rank, Reverse(a.boost), &a.sort_key).cmp(&(b.rank, Reverse(b.boost), &b.sort_key))
    });
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|c| seen.insert((c.suggestion.text.clone(), c.suggestion.kind)))
        .take(MAX_RESULTS)
        .map(|c| c.suggestion)
        .collect()
}
