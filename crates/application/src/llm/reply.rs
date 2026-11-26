//! Parsing of model replies: pulls the SQL out of a fenced block.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The SQL from the first SQL fenced block, if any.
    pub sql: Option<String>,
    /// Everything outside that block, trimmed.
    pub explanation: String,
    /// Set when the reply was not in the requested shape.
    pub warning: Option<String>,
}

const SQL_LANGUAGES: &[&str] = &[
    "",
    "sql",
    "postgres",
    "postgresql",
    "pgsql",
    "mysql",
    "mariadb",
    "plpgsql",
];

struct Block {
    language: String,
    body: String,
    terminated: bool,
    start: usize,
    end: usize,
}

fn find_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut offset = 0;
    let mut open: Option<(String, usize, usize)> = None; // (language, body start, fence start)
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            match open.take() {
                None => open = Some((rest.trim().to_lowercase(), offset + line.len(), offset)),
                Some((language, body_start, start)) => {
                    blocks.push(Block {
                        language,
                        body: text[body_start..offset].to_owned(),
                        terminated: true,
                        start,
                        end: offset + line.len(),
                    });
                }
            }
        }
        offset += line.len();
    }
    if let Some((language, body_start, start)) = open {
        blocks.push(Block {
            language,
            body: text[body_start..].to_owned(),
            terminated: false,
            start,
            end: text.len(),
        });
    }
    blocks
}

pub fn parse_reply(text: &str) -> Reply {
    let blocks = find_blocks(text);
    let sql_blocks: Vec<&Block> = blocks
        .iter()
        .filter(|b| SQL_LANGUAGES.contains(&b.language.as_str()))
        .collect();
    let Some(first) = sql_blocks.first() else {
        return Reply {
            sql: None,
            explanation: text.trim().to_owned(),
            warning: Some("The reply contained no SQL block; showing it as text.".into()),
        };
    };

    let sql = first.body.trim().to_owned();
    let before = text[..first.start].trim();
    let after = text[first.end..].trim();
    let explanation = [before, after]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut warnings = Vec::new();
    if !first.terminated {
        warnings.push("The reply ended inside the SQL block; it may be incomplete.".to_owned());
    }
    if sql_blocks.len() > 1 {
        warnings.push(format!(
            "The reply contained {} SQL blocks; using the first.",
            sql_blocks.len()
        ));
    }
    if sql.is_empty() {
        return Reply {
            sql: None,
            explanation,
            warning: Some("The SQL block was empty.".into()),
        };
    }
    Reply {
        sql: Some(sql),
        explanation,
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_sql_and_explanation() {
        let r = parse_reply("Here you go:\n```sql\nSELECT 1;\n```\nThis returns one.");
        assert_eq!(r.sql.as_deref(), Some("SELECT 1;"));
        assert_eq!(r.explanation, "Here you go:\n\nThis returns one.");
        assert_eq!(r.warning, None);
    }

    #[test]
    fn accepts_untagged_and_dialect_tagged_fences() {
        assert_eq!(
            parse_reply("```\nSELECT 1\n```").sql.as_deref(),
            Some("SELECT 1")
        );
        assert_eq!(
            parse_reply("```PostgreSQL\nSELECT 1\n```").sql.as_deref(),
            Some("SELECT 1")
        );
    }

    #[test]
    fn other_languages_are_not_sql() {
        let r = parse_reply("```python\nprint(1)\n```");
        assert_eq!(r.sql, None);
        assert!(r.warning.is_some());
    }

    #[test]
    fn plain_text_is_shown_with_a_warning() {
        let r = parse_reply("  I cannot answer that.  ");
        assert_eq!(
            (r.sql, r.explanation.as_str()),
            (None, "I cannot answer that.")
        );
        assert!(r.warning.is_some());
    }

    #[test]
    fn cut_off_reply_keeps_what_arrived_and_warns() {
        let r = parse_reply("Sure\n```sql\nSELECT a,\n       b FROM");
        assert_eq!(r.sql.as_deref(), Some("SELECT a,\n       b FROM"));
        assert!(
            r.warning
                .as_deref()
                .is_some_and(|w| w.contains("incomplete"))
        );
    }

    #[test]
    fn several_blocks_use_the_first_and_warn() {
        let r = parse_reply("```sql\nSELECT 1\n```\nor\n```sql\nSELECT 2\n```");
        assert_eq!(r.sql.as_deref(), Some("SELECT 1"));
        assert!(
            r.warning
                .as_deref()
                .is_some_and(|w| w.contains("2 SQL blocks"))
        );
    }

    #[test]
    fn empty_block_is_not_sql() {
        let r = parse_reply("```sql\n\n```");
        assert_eq!(r.sql, None);
        assert!(r.warning.is_some());
    }

    #[test]
    fn indented_fences_and_crlf_work() {
        let r = parse_reply("  ```sql\r\nSELECT 1\r\n  ```\r\n");
        assert_eq!(r.sql.as_deref(), Some("SELECT 1"));
    }
}
