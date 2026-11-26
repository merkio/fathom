//! Syntax highlighting classes. The UI only maps these to colours.

use std::ops::Range;

use domain::Dialect;

use super::lexer::{KeywordSet, TokenKind, tokenize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxClass {
    Keyword,
    Function,
    Identifier,
    String,
    Number,
    Comment,
    Punctuation,
    Whitespace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub class: SyntaxClass,
}

/// Classifies every byte of `sql`; spans are contiguous and cover the whole text.
pub fn highlight(sql: &str, dialect: &Dialect) -> Vec<Span> {
    let keywords = KeywordSet::new(dialect);
    let tokens = tokenize(sql, dialect);
    let mut spans = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let class = match t.kind {
            TokenKind::Whitespace => SyntaxClass::Whitespace,
            TokenKind::Comment => SyntaxClass::Comment,
            TokenKind::String => SyntaxClass::String,
            TokenKind::Number => SyntaxClass::Number,
            TokenKind::QuotedIdent => SyntaxClass::Identifier,
            TokenKind::Punct => SyntaxClass::Punctuation,
            TokenKind::Word if keywords.contains(t.text(sql)) => SyntaxClass::Keyword,
            TokenKind::Word => {
                let followed_by_paren = tokens[i + 1..]
                    .iter()
                    .find(|n| n.kind != TokenKind::Whitespace)
                    .is_some_and(|n| n.text(sql) == "(");
                if followed_by_paren {
                    SyntaxClass::Function
                } else {
                    SyntaxClass::Identifier
                }
            }
        };
        spans.push(Span {
            range: t.range.clone(),
            class,
        });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(sql: &str) -> Vec<(&str, SyntaxClass)> {
        highlight(sql, &Dialect::ANSI)
            .into_iter()
            .filter(|s| s.class != SyntaxClass::Whitespace)
            .map(|s| (&sql[s.range], s.class))
            .collect()
    }

    #[test]
    fn classifies_common_tokens() {
        use SyntaxClass::*;
        assert_eq!(
            classes("select count(*), 'x', 1 from t -- c"),
            vec![
                ("select", Keyword),
                ("count", Function),
                ("(", Punctuation),
                ("*", Punctuation),
                (")", Punctuation),
                (",", Punctuation),
                ("'x'", String),
                (",", Punctuation),
                ("1", Number),
                ("from", Keyword),
                ("t", Identifier),
                ("-- c", Comment),
            ]
        );
    }

    #[test]
    fn spans_cover_the_text_without_gaps() {
        let sql = "select  \"A b\" from t where x = 'é'";
        let spans = highlight(sql, &Dialect::ANSI);
        let mut at = 0;
        for s in &spans {
            assert_eq!(s.range.start, at);
            at = s.range.end;
        }
        assert_eq!(at, sql.len());
    }
}
