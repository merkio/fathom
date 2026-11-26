//! Dialect capabilities. Each database adapter supplies its own `Dialect`; no SQL-specific
//! knowledge lives in the UI.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStyle {
    /// `... LIMIT n`
    Limit,
    /// `... FETCH FIRST n ROWS ONLY`
    FetchFirst,
    /// `SELECT TOP n ...`
    Top,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect {
    pub name: &'static str,
    pub identifier_quote: char,
    /// Identifiers containing uppercase letters must be quoted to keep their case (PostgreSQL
    /// folds unquoted names to lowercase; MySQL does not).
    pub quote_mixed_case: bool,
    pub limit_style: LimitStyle,
    pub keywords: &'static [&'static str],
    pub functions: &'static [&'static str],
}

impl Dialect {
    /// Quotes an identifier, doubling any embedded quote character.
    pub fn quote_identifier(&self, ident: &str) -> String {
        let q = self.identifier_quote;
        let escaped = ident.replace(q, &format!("{q}{q}"));
        format!("{q}{escaped}{q}")
    }

    /// A dialect with ANSI defaults, used by fakes and as a starting point for adapters.
    pub const ANSI: Dialect = Dialect {
        name: "ansi",
        identifier_quote: '"',
        quote_mixed_case: true,
        limit_style: LimitStyle::FetchFirst,
        keywords: &[
            "SELECT", "FROM", "WHERE", "JOIN", "ON", "GROUP", "ORDER", "BY",
        ],
        functions: &["COUNT", "SUM", "MIN", "MAX", "AVG"],
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_escapes_identifiers() {
        assert_eq!(Dialect::ANSI.quote_identifier("users"), "\"users\"");
        assert_eq!(Dialect::ANSI.quote_identifier("we\"ird"), "\"we\"\"ird\"");
        let mysql = Dialect {
            identifier_quote: '`',
            ..Dialect::ANSI
        };
        assert_eq!(mysql.quote_identifier("a`b"), "`a``b`");
    }
}
