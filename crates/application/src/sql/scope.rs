//! Finds the tables a statement refers to (`FROM`, `JOIN`, `UPDATE`, `INTO`) and resolves them
//! against the schema.

use domain::{Dialect, SchemaTree, Table};

use super::lexer::{KeywordSet, Token, TokenKind, identifier_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
    /// Introduced by `JOIN` (as opposed to `FROM`, a comma list, `UPDATE` or `INTO`).
    pub via_join: bool,
    /// Byte offset where the reference starts.
    pub pos: usize,
}

impl TableRef {
    /// How columns of this table are qualified in the statement.
    pub fn qualifier(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

/// Table references among the significant tokens of one statement.
pub fn table_refs(sql: &str, tokens: &[&Token], dialect: &Dialect) -> Vec<TableRef> {
    let keywords = KeywordSet::new(dialect);
    let text = |i: usize| tokens.get(i).map(|t| t.text(sql));
    let is_ident = |i: usize| {
        tokens.get(i).is_some_and(|t| match t.kind {
            TokenKind::QuotedIdent => true,
            TokenKind::Word => !keywords.contains(t.text(sql)),
            _ => false,
        })
    };
    let name_at = |i: usize| identifier_text(tokens[i].text(sql), tokens[i], dialect);
    let is_word = |i: usize, w: &str| {
        tokens
            .get(i)
            .is_some_and(|t| t.kind == TokenKind::Word && t.text(sql).eq_ignore_ascii_case(w))
    };

    // Parses `[schema.]name [AS] [alias]` at `i`; returns the ref and the next index.
    let parse_ref = |mut i: usize, via_join: bool| -> Option<(TableRef, usize)> {
        if !is_ident(i) {
            return None;
        }
        let pos = tokens[i].range.start;
        let mut schema = None;
        let mut name = name_at(i);
        i += 1;
        if text(i) == Some(".") && is_ident(i + 1) {
            schema = Some(name);
            name = name_at(i + 1);
            i += 2;
        }
        let mut alias = None;
        if is_word(i, "AS") {
            i += 1;
            if is_ident(i) {
                alias = Some(name_at(i));
                i += 1;
            }
        } else if is_ident(i) {
            alias = Some(name_at(i));
            i += 1;
        }
        Some((
            TableRef {
                schema,
                name,
                alias,
                via_join,
                pos,
            },
            i,
        ))
    };

    let mut refs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let upper = (tokens[i].kind == TokenKind::Word).then(|| tokens[i].text(sql).to_uppercase());
        let (is_from, via_join) = match upper.as_deref() {
            Some("FROM") => (true, false),
            Some("JOIN") => (false, true),
            Some("UPDATE" | "INTO") => (false, false),
            _ => {
                i += 1;
                continue;
            }
        };
        i += 1;
        let Some((first, mut next)) = parse_ref(i, via_join) else {
            continue;
        };
        refs.push(first);
        if is_from {
            while text(next) == Some(",") {
                let Some((r, n)) = parse_ref(next + 1, false) else {
                    break;
                };
                refs.push(r);
                next = n;
            }
        }
        i = next;
    }
    refs
}

/// Looks a reference up in the schema (names compare case-insensitively).
pub fn resolve<'a>(tree: &'a SchemaTree, r: &TableRef) -> Option<(&'a str, &'a Table)> {
    tree.tables().find(|(schema, t)| {
        t.name.eq_ignore_ascii_case(&r.name)
            && r.schema
                .as_ref()
                .is_none_or(|s| schema.eq_ignore_ascii_case(s))
    })
}
