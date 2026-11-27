use application::ports::{DatabaseDriver, DbError, DbSession};
use async_trait::async_trait;
use clickhouse::Client;
use domain::{ConnectionProfile, Dialect, DriverKind, LimitStyle};
use secrecy::{ExposeSecret, SecretString};

use crate::error::map_error;
use crate::session::ClickHouseSession;

pub(crate) const CLICKHOUSE_DIALECT: Dialect = Dialect {
    name: "clickhouse",
    identifier_quote: '`',
    // Identifiers are case-sensitive.
    quote_mixed_case: true,
    limit_style: LimitStyle::Limit,
    keywords: &[
        "SELECT",
        "FROM",
        "WHERE",
        "PREWHERE",
        "GROUP BY",
        "HAVING",
        "ORDER BY",
        "LIMIT",
        "OFFSET",
        "LIMIT BY",
        "WITH",
        "WITH TOTALS",
        "FINAL",
        "SAMPLE",
        "ARRAY JOIN",
        "LEFT ARRAY JOIN",
        "JOIN",
        "INNER JOIN",
        "LEFT JOIN",
        "RIGHT JOIN",
        "FULL JOIN",
        "CROSS JOIN",
        "ASOF JOIN",
        "ANY JOIN",
        "GLOBAL IN",
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
        "IS NULL",
        "IS NOT NULL",
        "DISTINCT",
        "UNION ALL",
        "UNION DISTINCT",
        "INTERSECT",
        "EXCEPT",
        "SETTINGS",
        "FORMAT",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "INSERT INTO",
        "VALUES",
        "ALTER TABLE",
        "UPDATE",
        "DELETE",
        "CREATE TABLE",
        "CREATE VIEW",
        "CREATE MATERIALIZED VIEW",
        "DROP TABLE",
        "TRUNCATE TABLE",
        "OPTIMIZE TABLE",
        "ENGINE",
        "PARTITION BY",
        "PRIMARY KEY",
        "TTL",
        "DESCRIBE",
        "EXPLAIN",
        "SHOW TABLES",
        "SHOW DATABASES",
    ],
    functions: &[
        "count",
        "sum",
        "avg",
        "min",
        "max",
        "uniq",
        "uniqExact",
        "uniqCombined",
        "any",
        "anyLast",
        "argMin",
        "argMax",
        "groupArray",
        "groupUniqArray",
        "quantile",
        "quantiles",
        "median",
        "topK",
        "if",
        "multiIf",
        "ifNull",
        "coalesce",
        "nullIf",
        "toString",
        "toInt32",
        "toInt64",
        "toUInt32",
        "toUInt64",
        "toFloat64",
        "toDate",
        "toDateTime",
        "toDateTime64",
        "toStartOfDay",
        "toStartOfHour",
        "toStartOfMonth",
        "toStartOfWeek",
        "toYYYYMM",
        "now",
        "today",
        "dateDiff",
        "dateAdd",
        "formatDateTime",
        "length",
        "lower",
        "upper",
        "substring",
        "concat",
        "replaceAll",
        "splitByChar",
        "position",
        "match",
        "arrayJoin",
        "arrayMap",
        "arrayFilter",
        "has",
        "empty",
        "notEmpty",
        "JSONExtractString",
        "JSONExtractInt",
        "JSONExtract",
        "round",
        "floor",
        "ceil",
        "abs",
        "cityHash64",
        "generateUUIDv4",
        "sleep",
    ],
};

#[derive(Debug, Default, Clone, Copy)]
pub struct ClickHouseDriver;

impl ClickHouseDriver {
    pub fn new() -> Self {
        Self
    }
}

/// HTTPS when the profile asks for TLS: `secure=true`, or a `sslmode` that requires it.
fn use_tls(profile: &ConnectionProfile) -> Result<bool, DbError> {
    let secure = profile
        .options
        .get("secure")
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"));
    let by_mode = match profile.options.get("sslmode").map(String::as_str) {
        None | Some("disable" | "prefer" | "allow") => false,
        Some("require" | "verify-ca" | "verify-full" | "verify-identity") => true,
        Some(other) => return Err(DbError::Unsupported(format!("sslmode `{other}`"))),
    };
    Ok(secure || by_mode)
}

fn url(profile: &ConnectionProfile, tls: bool) -> String {
    let port = profile.port.unwrap_or(if tls { 8443 } else { 8123 });
    let host = profile.host.trim();
    // An IPv6 literal needs brackets in a URL.
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    format!("{}://{host}:{port}", if tls { "https" } else { "http" })
}

fn client(profile: &ConnectionProfile, secret: Option<SecretString>) -> Result<Client, DbError> {
    let tls = use_tls(profile)?;
    let mut client = Client::default()
        .with_url(url(profile, tls))
        .with_user(if profile.username.trim().is_empty() {
            "default"
        } else {
            profile.username.trim()
        })
        .with_product_info("Fathom", env!("CARGO_PKG_VERSION"));
    if let Some(db) = profile
        .database
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        client = client.with_database(db);
    }
    if let Some(secret) = secret {
        client = client.with_password(secret.expose_secret());
    }
    Ok(client)
}

#[async_trait]
impl DatabaseDriver for ClickHouseDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::new("clickhouse")
    }

    fn dialect(&self) -> Dialect {
        CLICKHOUSE_DIALECT
    }

    async fn connect(
        &self,
        profile: &ConnectionProfile,
        secret: Option<SecretString>,
    ) -> Result<Box<dyn DbSession>, DbError> {
        let client = client(profile, secret)?;
        // The HTTP client connects lazily: ask the server something so a wrong address, user or
        // password fails here and not on the first query.
        let mut cursor = client
            .query("SELECT 1")
            .fetch_bytes("TabSeparated")
            .map_err(map_error)?;
        cursor.collect().await.map_err(map_error)?;
        Ok(Box::new(ClickHouseSession::new(client)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(options: &[(&str, &str)]) -> ConnectionProfile {
        let mut p = ConnectionProfile::new("t", DriverKind::new("clickhouse"), "ch.internal");
        for (k, v) in options {
            p.options.insert((*k).into(), (*v).into());
        }
        p
    }

    #[test]
    fn identity_and_dialect() {
        let d = ClickHouseDriver::new();
        assert_eq!(d.kind().as_str(), "clickhouse");
        assert_eq!(d.dialect().name, "clickhouse");
        assert!(d.dialect().quote_mixed_case);
        assert_eq!(d.dialect().quote_identifier("Weird name"), "`Weird name`");
    }

    #[test]
    fn plain_http_is_the_default_on_port_8123() {
        let p = profile(&[]);
        assert_eq!(use_tls(&p), Ok(false));
        assert_eq!(url(&p, false), "http://ch.internal:8123");
    }

    #[test]
    fn tls_options_switch_to_https_on_8443() {
        for options in [
            vec![("secure", "true")],
            vec![("secure", "TRUE")],
            vec![("sslmode", "require")],
            vec![("sslmode", "verify-full")],
        ] {
            let p = profile(&options);
            assert_eq!(use_tls(&p), Ok(true), "{options:?}");
            assert_eq!(url(&p, true), "https://ch.internal:8443");
        }
        assert_eq!(use_tls(&profile(&[("sslmode", "disable")])), Ok(false));
    }

    #[test]
    fn unknown_sslmode_is_rejected_and_explicit_ports_win() {
        assert!(matches!(
            use_tls(&profile(&[("sslmode", "bogus")])),
            Err(DbError::Unsupported(_))
        ));
        let mut p = profile(&[]);
        p.port = Some(9000);
        assert_eq!(url(&p, false), "http://ch.internal:9000");
    }

    #[test]
    fn ipv6_literals_get_brackets() {
        let mut p = profile(&[]);
        p.host = "::1".into();
        assert_eq!(url(&p, false), "http://[::1]:8123");
        p.host = "[::1]".into();
        assert_eq!(url(&p, false), "http://[::1]:8123");
    }
}
