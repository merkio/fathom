//! A forgiving SQL lexer. Unlike a parser's tokenizer it never fails: unterminated strings,
//! quoted identifiers and comments simply run to the end of the input, which is exactly what
//! half-typed SQL looks like. Token ranges are byte offsets into the input.

use std::collections::HashSet;
use std::ops::Range;

use domain::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Whitespace,
    Comment,
    /// Unquoted identifier or keyword.
    Word,
    QuotedIdent,
    String,
    Number,
    Punct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: Range<usize>,
    /// False for a string, quoted identifier or block comment missing its closing delimiter, and
    /// for line comments (which end at the line break, not at a delimiter).
    pub terminated: bool,
}

impl Token {
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.range.clone()]
    }

    pub fn is_significant(&self) -> bool {
        !matches!(self.kind, TokenKind::Whitespace | TokenKind::Comment)
    }
}

/// Words that structure statements, independent of any dialect's own keyword list.
const CORE_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "BY",
    "HAVING",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "ON",
    "USING",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IN",
    "EXISTS",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "NULL",
    "DISTINCT",
    "UNION",
    "ALL",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "RETURNING",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "ALTER",
    "DROP",
    "TABLE",
    "INDEX",
    "VIEW",
    "ASC",
    "DESC",
    "TRUE",
    "FALSE",
    "ANY",
    "TRUNCATE",
    "RENAME",
];

/// Uppercase single-word keywords for a dialect (its own list plus the core set).
pub struct KeywordSet(HashSet<String>);

impl KeywordSet {
    pub fn new(dialect: &Dialect) -> Self {
        let mut set: HashSet<String> = CORE_KEYWORDS.iter().map(|s| (*s).to_owned()).collect();
        for entry in dialect.keywords {
            set.extend(entry.split_whitespace().map(str::to_uppercase));
        }
        Self(set)
    }

    pub fn contains(&self, word: &str) -> bool {
        self.0.contains(&word.to_uppercase())
    }
}

fn is_word_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_word_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

pub fn tokenize(sql: &str, dialect: &Dialect) -> Vec<Token> {
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let byte_at = |i: usize| chars.get(i).map_or(sql.len(), |(b, _)| *b);
    let quote = dialect.identifier_quote;
    let mut tokens = Vec::new();
    let mut i = 0;

    // Scans a quoted run starting at `i` (the opening quote). Returns (next index, terminated).
    let scan_quoted = |start: usize, q: char| -> (usize, bool) {
        let mut j = start + 1;
        while j < chars.len() {
            if chars[j].1 == q {
                if chars.get(j + 1).is_some_and(|(_, c)| *c == q) {
                    j += 2;
                    continue;
                }
                return (j + 1, true);
            }
            j += 1;
        }
        (chars.len(), false)
    };

    while i < chars.len() {
        let (start_byte, c) = chars[i];
        let next = chars.get(i + 1).map(|(_, c)| *c);
        let (kind, end, terminated) = if c.is_whitespace() {
            let mut j = i;
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
            (TokenKind::Whitespace, j, true)
        } else if c == '-' && next == Some('-') {
            let mut j = i;
            while j < chars.len() && chars[j].1 != '\n' {
                j += 1;
            }
            (TokenKind::Comment, j, false)
        } else if c == '/' && next == Some('*') {
            let mut j = i + 2;
            let mut closed = false;
            while j < chars.len() {
                if chars[j].1 == '*' && chars.get(j + 1).is_some_and(|(_, c)| *c == '/') {
                    j += 2;
                    closed = true;
                    break;
                }
                j += 1;
            }
            (TokenKind::Comment, j, closed)
        } else if c == '\'' {
            let (j, t) = scan_quoted(i, '\'');
            (TokenKind::String, j, t)
        } else if c == quote {
            let (j, t) = scan_quoted(i, quote);
            (TokenKind::QuotedIdent, j, t)
        } else if c == '"' {
            // Not the identifier quote for this dialect (MySQL): a string literal.
            let (j, t) = scan_quoted(i, '"');
            (TokenKind::String, j, t)
        } else if is_word_start(c) {
            let mut j = i + 1;
            while j < chars.len() && is_word_char(chars[j].1) {
                j += 1;
            }
            (TokenKind::Word, j, true)
        } else if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].1.is_ascii_digit() || chars[j].1 == '.') {
                j += 1;
            }
            (TokenKind::Number, j, true)
        } else {
            (TokenKind::Punct, i + 1, true)
        };
        tokens.push(Token {
            kind,
            range: start_byte..byte_at(end),
            terminated,
        });
        i = end;
    }
    tokens
}

/// Text of an identifier token without its quotes, with doubled quotes collapsed.
pub fn identifier_text(text: &str, token: &Token, dialect: &Dialect) -> String {
    match token.kind {
        TokenKind::QuotedIdent => {
            let q = dialect.identifier_quote;
            let inner = text.strip_prefix(q).unwrap_or(text);
            let inner = if token.terminated {
                inner.strip_suffix(q).unwrap_or(inner)
            } else {
                inner
            };
            inner.replace(&format!("{q}{q}"), &q.to_string())
        }
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<(TokenKind, &str)> {
        tokenize(sql, &Dialect::ANSI)
            .into_iter()
            .filter(Token::is_significant)
            .map(|t| (t.kind, &sql[t.range]))
            .collect()
    }

    #[test]
    fn splits_words_numbers_and_punctuation() {
        assert_eq!(
            kinds("select a.b, 12.5 from t;"),
            vec![
                (TokenKind::Word, "select"),
                (TokenKind::Word, "a"),
                (TokenKind::Punct, "."),
                (TokenKind::Word, "b"),
                (TokenKind::Punct, ","),
                (TokenKind::Number, "12.5"),
                (TokenKind::Word, "from"),
                (TokenKind::Word, "t"),
                (TokenKind::Punct, ";"),
            ]
        );
    }

    #[test]
    fn unterminated_input_runs_to_the_end_without_failing() {
        let t = tokenize("select 'abc", &Dialect::ANSI);
        let last = t.last().expect("token");
        assert_eq!(
            (last.kind, last.terminated, last.range.clone()),
            (TokenKind::String, false, 7..11)
        );
        let t = tokenize("select \"Us", &Dialect::ANSI);
        assert_eq!(
            t.last().map(|t| (t.kind, t.terminated)),
            Some((TokenKind::QuotedIdent, false))
        );
        let t = tokenize("select /* hi", &Dialect::ANSI);
        assert_eq!(
            t.last().map(|t| (t.kind, t.terminated)),
            Some((TokenKind::Comment, false))
        );
    }

    #[test]
    fn doubled_quotes_stay_inside_the_token() {
        assert_eq!(kinds("'it''s'"), vec![(TokenKind::String, "'it''s'")]);
        assert_eq!(
            kinds("\"a\"\"b\""),
            vec![(TokenKind::QuotedIdent, "\"a\"\"b\"")]
        );
    }

    #[test]
    fn backtick_dialect_treats_double_quotes_as_strings() {
        let mysql = Dialect {
            identifier_quote: '`',
            ..Dialect::ANSI
        };
        let t = tokenize("`a b` \"s\"", &mysql);
        let sig: Vec<_> = t
            .iter()
            .filter(|t| t.is_significant())
            .map(|t| t.kind)
            .collect();
        assert_eq!(sig, vec![TokenKind::QuotedIdent, TokenKind::String]);
    }

    #[test]
    fn comments_are_skipped_and_unicode_ranges_are_byte_offsets() {
        let sql = "select 'é' -- note\nfrom t";
        let t = tokenize(sql, &Dialect::ANSI);
        let from = t.iter().find(|t| t.text(sql) == "from").expect("from");
        assert_eq!(&sql[from.range.clone()], "from");
        assert!(t.iter().any(|t| t.kind == TokenKind::Comment));
    }

    #[test]
    fn identifier_text_unquotes_and_unescapes() {
        let sql = "\"a\"\"b\"";
        let t = &tokenize(sql, &Dialect::ANSI)[0];
        assert_eq!(identifier_text(sql, t, &Dialect::ANSI), "a\"b");
    }

    #[test]
    fn keyword_set_includes_core_and_dialect_words() {
        let kw = KeywordSet::new(&Dialect::ANSI);
        assert!(kw.contains("select") && kw.contains("GROUP") && !kw.contains("customers"));
    }
}
