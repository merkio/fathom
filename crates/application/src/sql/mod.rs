//! SQL text helpers shared by autocomplete and the editor: a tolerant lexer, scope detection,
//! completion and highlighting. Everything here is pure and synchronous (no I/O).

pub mod changes;
pub mod complete;
pub mod highlight;
pub mod lexer;
pub mod scope;

pub use complete::{CompletionRequest, Suggestion, SuggestionKind, SuggestionSource, complete};
pub use highlight::{Span, SyntaxClass, highlight};
pub use lexer::{KeywordSet, Token, TokenKind, tokenize};

use domain::Dialect;

/// Whether the SQL contains a statement that changes the schema (so cached metadata is stale).
pub fn changes_schema(sql: &str, dialect: &Dialect) -> bool {
    const DDL: &[&str] = &["CREATE", "ALTER", "DROP", "TRUNCATE", "RENAME"];
    let tokens = tokenize(sql, dialect);
    let mut at_statement_start = true;
    for t in tokens.iter().filter(|t| t.is_significant()) {
        let text = t.text(sql);
        if text == ";" {
            at_statement_start = true;
        } else if at_statement_start {
            at_statement_start = false;
            if t.kind == TokenKind::Word && DDL.iter().any(|k| text.eq_ignore_ascii_case(k)) {
                return true;
            }
        }
    }
    false
}
