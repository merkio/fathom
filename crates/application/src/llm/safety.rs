//! `SqlSafetyGuard`: classifies SQL (read / write / DDL / admin) before anything runs.
//!
//! LLM-generated SQL is never executed automatically; this classification drives the badge the
//! user sees, and per-connection read-only mode blocks everything that is not a plain read.
//!
//! Limitation: this is a syntactic check. A `SELECT` that calls a function with side effects
//! (for example `pg_terminate_backend`) is still classified as a read. Enforcing read-only at
//! the connection level (M8) is the complete answer; this guard is the first line.

use domain::Dialect;
use sqlparser::ast::{Query, SetExpr, Statement};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;

/// Ordered by risk: later variants are more dangerous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatementKind {
    /// Only reads data (`SELECT`, `SHOW`, `EXPLAIN` without `ANALYZE`, ...).
    Read,
    /// Changes data (`INSERT`, `UPDATE`, `DELETE`, `MERGE`, `COPY`, `SELECT ... INTO`, ...).
    Write,
    /// Changes structure (`CREATE`, `ALTER`, `DROP`, `TRUNCATE`, ...).
    Ddl,
    /// Permissions, session and transaction control, procedure calls.
    Admin,
    /// Could not be parsed or recognised; treated as unsafe.
    Unknown,
}

impl StatementKind {
    pub fn label(self) -> &'static str {
        match self {
            StatementKind::Read => "read",
            StatementKind::Write => "write",
            StatementKind::Ddl => "ddl",
            StatementKind::Admin => "admin",
            StatementKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyReport {
    /// One entry per statement, in order.
    pub statements: Vec<StatementKind>,
    /// Why the SQL could not be fully parsed, if so.
    pub parse_error: Option<String>,
}

impl SafetyReport {
    /// The most dangerous kind present, or `None` for empty input.
    pub fn overall(&self) -> Option<StatementKind> {
        self.statements.iter().copied().max()
    }

    /// True only for non-empty input made entirely of plain reads that parsed cleanly.
    pub fn is_read_only(&self) -> bool {
        self.parse_error.is_none()
            && !self.statements.is_empty()
            && self.statements.iter().all(|k| *k == StatementKind::Read)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SqlSafetyGuard;

impl SqlSafetyGuard {
    pub fn classify(&self, sql: &str, dialect: &Dialect) -> SafetyReport {
        let parsed = match dialect.name {
            "postgres" => Parser::parse_sql(&PostgreSqlDialect {}, sql),
            "mysql" | "mariadb" => Parser::parse_sql(&MySqlDialect {}, sql),
            _ => Parser::parse_sql(&GenericDialect {}, sql),
        };
        match parsed {
            Ok(statements) => SafetyReport {
                statements: statements.iter().map(classify_statement).collect(),
                parse_error: None,
            },
            Err(e) => SafetyReport {
                statements: vec![StatementKind::Unknown],
                parse_error: Some(e.to_string()),
            },
        }
    }
}

fn classify_statement(statement: &Statement) -> StatementKind {
    match statement {
        Statement::Query(q) => {
            if query_writes(q) {
                StatementKind::Write
            } else {
                StatementKind::Read
            }
        }
        // `EXPLAIN ANALYZE` executes the statement; so can `EXPLAIN (ANALYZE ...)`.
        Statement::Explain {
            analyze,
            statement,
            options,
            ..
        } => {
            if *analyze || options.is_some() {
                classify_statement(statement).max(StatementKind::Read)
            } else {
                StatementKind::Read
            }
        }
        other => by_leading_keyword(&other.to_string()),
    }
}

fn by_leading_keyword(sql: &str) -> StatementKind {
    let word = sql
        .trim_start()
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    match word.as_str() {
        "SELECT" | "VALUES" | "TABLE" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "WITH" => {
            StatementKind::Read
        }
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "COPY" | "LOAD" | "UPSERT" => {
            StatementKind::Write
        }
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "RENAME" | "COMMENT" => StatementKind::Ddl,
        "" => StatementKind::Unknown,
        _ => StatementKind::Admin,
    }
}

/// Whether a query contains a data-modifying common table expression or `SELECT ... INTO`.
fn query_writes(q: &Query) -> bool {
    let in_ctes = q
        .with
        .as_ref()
        .is_some_and(|w| w.cte_tables.iter().any(|cte| query_writes(&cte.query)));
    in_ctes || set_expr_writes(&q.body)
}

fn set_expr_writes(e: &SetExpr) -> bool {
    match e {
        SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => true,
        SetExpr::Select(s) => s.into.is_some(),
        SetExpr::Query(q) => query_writes(q),
        SetExpr::SetOperation { left, right, .. } => {
            set_expr_writes(left) || set_expr_writes(right)
        }
        SetExpr::Values(_) | SetExpr::Table(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(dialect_name: &'static str, sql: &str) -> Vec<StatementKind> {
        let d = Dialect {
            name: dialect_name,
            ..Dialect::ANSI
        };
        SqlSafetyGuard.classify(sql, &d).statements
    }

    use StatementKind::*;

    #[test]
    fn reads() {
        for sql in [
            "SELECT 1",
            "select * from t where a in (select b from u)",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "VALUES (1), (2)",
            "SHOW TABLES",
            "EXPLAIN SELECT * FROM t",
            "SELECT 1 UNION SELECT 2",
        ] {
            assert_eq!(kinds("postgres", sql), [Read], "{sql}");
        }
    }

    #[test]
    fn writes() {
        for sql in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "SELECT * INTO backup FROM t",
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
            "WITH d AS (INSERT INTO t VALUES (1) RETURNING *) SELECT * FROM d",
        ] {
            assert_eq!(kinds("postgres", sql), [Write], "{sql}");
        }
    }

    #[test]
    fn ddl() {
        for sql in [
            "CREATE TABLE t (id int)",
            "ALTER TABLE t ADD COLUMN c int",
            "DROP TABLE t",
            "TRUNCATE t",
            "CREATE INDEX i ON t (a)",
        ] {
            assert_eq!(kinds("postgres", sql), [Ddl], "{sql}");
        }
    }

    #[test]
    fn explain_analyze_executes_the_statement() {
        assert_eq!(kinds("postgres", "EXPLAIN ANALYZE DELETE FROM t"), [Write]);
        assert_eq!(kinds("postgres", "EXPLAIN ANALYZE SELECT 1"), [Read]);
        assert_eq!(
            kinds("postgres", "EXPLAIN (ANALYZE) DELETE FROM t"),
            [Write]
        );
        assert_eq!(kinds("postgres", "EXPLAIN DELETE FROM t"), [Read]);
    }

    #[test]
    fn admin_statements() {
        for sql in [
            "GRANT SELECT ON t TO bob",
            "SET search_path = x",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
        ] {
            assert_eq!(kinds("postgres", sql), [Admin], "{sql}");
        }
    }

    #[test]
    fn multiple_statements_report_each_and_the_worst_wins() {
        let d = Dialect {
            name: "postgres",
            ..Dialect::ANSI
        };
        let r = SqlSafetyGuard.classify("SELECT 1; DROP TABLE t; SELECT 2", &d);
        assert_eq!(r.statements, [Read, Ddl, Read]);
        assert_eq!(r.overall(), Some(Ddl));
        assert!(!r.is_read_only());
    }

    #[test]
    fn a_semicolon_inside_a_string_does_not_split_statements() {
        assert_eq!(kinds("postgres", "SELECT 'a; DROP TABLE t'"), [Read]);
    }

    #[test]
    fn unparseable_sql_is_unknown_and_not_read_only() {
        let d = Dialect {
            name: "postgres",
            ..Dialect::ANSI
        };
        let r = SqlSafetyGuard.classify("SELEKT nonsense FROM", &d);
        assert_eq!(r.statements, [Unknown]);
        assert!(r.parse_error.is_some() && !r.is_read_only());
        assert!(
            !SqlSafetyGuard.classify("", &d).is_read_only(),
            "empty is not a read"
        );
    }

    #[test]
    fn read_only_requires_every_statement_to_be_a_read() {
        let d = Dialect {
            name: "postgres",
            ..Dialect::ANSI
        };
        assert!(
            SqlSafetyGuard
                .classify("SELECT 1; SELECT 2;", &d)
                .is_read_only()
        );
    }

    #[test]
    fn dialect_specific_syntax_parses_with_the_matching_dialect() {
        assert_eq!(kinds("mysql", "SELECT `a` FROM `t` LIMIT 5"), [Read]);
        assert_eq!(kinds("postgres", "SELECT a::int FROM t"), [Read]);
        assert_eq!(
            kinds(
                "mysql",
                "INSERT INTO `t` (a) VALUES (1) ON DUPLICATE KEY UPDATE a = 2"
            ),
            [Write]
        );
    }

    #[test]
    fn kinds_are_ordered_by_risk() {
        assert!(Read < Write && Write < Ddl && Ddl < Admin && Admin < Unknown);
        assert_eq!(Ddl.label(), "ddl");
    }
}
