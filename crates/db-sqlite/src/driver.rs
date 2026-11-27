use application::ports::{DatabaseDriver, DbError, DbSession};
use async_trait::async_trait;
use domain::{ConnectionProfile, Dialect, DriverKind, LimitStyle};
use rusqlite::{Connection, OpenFlags};
use secrecy::SecretString;

use crate::session::SqliteSession;

pub(crate) const SQLITE_DIALECT: Dialect = Dialect {
    name: "sqlite",
    identifier_quote: '"',
    // SQLite compares identifiers case-insensitively, so mixed case needs no quoting.
    quote_mixed_case: false,
    limit_style: LimitStyle::Limit,
    keywords: &[
        "SELECT",
        "FROM",
        "WHERE",
        "GROUP BY",
        "HAVING",
        "ORDER BY",
        "LIMIT",
        "OFFSET",
        "JOIN",
        "INNER JOIN",
        "LEFT JOIN",
        "CROSS JOIN",
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
        "GLOB",
        "IS NULL",
        "IS NOT NULL",
        "DISTINCT",
        "UNION",
        "UNION ALL",
        "INTERSECT",
        "EXCEPT",
        "WITH",
        "RETURNING",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "INSERT INTO",
        "VALUES",
        "UPDATE",
        "SET",
        "DELETE FROM",
        "CREATE TABLE",
        "ALTER TABLE",
        "DROP TABLE",
        "CREATE INDEX",
        "EXPLAIN",
        "PRAGMA",
    ],
    functions: &[
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
        "TOTAL",
        "COALESCE",
        "IFNULL",
        "NULLIF",
        "LENGTH",
        "LOWER",
        "UPPER",
        "SUBSTR",
        "TRIM",
        "REPLACE",
        "ROUND",
        "ABS",
        "DATE",
        "TIME",
        "DATETIME",
        "STRFTIME",
        "GROUP_CONCAT",
        "JSON_EXTRACT",
        "RANDOM",
        "CAST",
    ],
};

#[derive(Debug, Default, Clone, Copy)]
pub struct SqliteDriver;

impl SqliteDriver {
    pub fn new() -> Self {
        Self
    }
}

fn option_is(profile: &ConnectionProfile, key: &str, value: &str) -> bool {
    profile
        .options
        .get(key)
        .is_some_and(|v| v.eq_ignore_ascii_case(value))
}

#[async_trait]
impl DatabaseDriver for SqliteDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::new("sqlite")
    }

    fn dialect(&self) -> Dialect {
        SQLITE_DIALECT
    }

    async fn connect(
        &self,
        profile: &ConnectionProfile,
        _secret: Option<SecretString>,
    ) -> Result<Box<dyn DbSession>, DbError> {
        let Some(path) = profile.database.clone().filter(|p| !p.trim().is_empty()) else {
            return Err(DbError::Connection(
                "No database file: enter the path of the SQLite file in the Database field".into(),
            ));
        };
        let mut flags = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
        flags |= if option_is(profile, "mode", "ro") {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        if option_is(profile, "create", "true") {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        // Opening touches the disk, so keep it off the async threads.
        let opened = tokio::task::spawn_blocking(move || {
            let conn = Connection::open_with_flags(&path, flags)
                .map_err(|e| DbError::Connection(format!("{path}: {e}")))?;
            // Opening is lazy: read the schema header so a file that is not a database fails now.
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|e| DbError::Connection(format!("{path}: {e}")))?;
            Ok::<_, DbError>(conn)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))??;
        Ok(Box::new(SqliteSession::new(opened)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_dialect() {
        let d = SqliteDriver::new();
        assert_eq!(d.kind().as_str(), "sqlite");
        assert_eq!(d.dialect().name, "sqlite");
        assert!(!d.dialect().quote_mixed_case);
    }
}
