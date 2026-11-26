use std::path::PathBuf;
use std::time::Duration;

use application::ports::{DatabaseDriver, DbError, DbSession};
use async_trait::async_trait;
use domain::{ConnectionProfile, Dialect, DriverKind};
use secrecy::{ExposeSecret, SecretString};
use sqlx::ConnectOptions;
use sqlx::mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode};

use crate::dialect::MYSQL_DIALECT;
use crate::errors::map_connect_error;
use crate::session::MySqlSession;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Clone, Copy)]
pub struct MySqlDriver;

impl MySqlDriver {
    pub fn new() -> Self {
        Self
    }
}

fn ssl_mode(profile: &ConnectionProfile) -> Result<MySqlSslMode, DbError> {
    match profile.options.get("sslmode").map(String::as_str) {
        None | Some("prefer" | "allow") => Ok(MySqlSslMode::Preferred),
        Some("disable") => Ok(MySqlSslMode::Disabled),
        Some("require") => Ok(MySqlSslMode::Required),
        Some("verify-ca") => Ok(MySqlSslMode::VerifyCa),
        Some("verify-full" | "verify-identity") => Ok(MySqlSslMode::VerifyIdentity),
        Some(other) => Err(DbError::Unsupported(format!("sslmode `{other}`"))),
    }
}

/// A path option that must point at an existing file; absent or blank means "not set".
fn file_option(profile: &ConnectionProfile, key: &str, what: &str) -> Result<Option<PathBuf>, DbError> {
    let Some(value) = profile.options.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.is_file() {
        Ok(Some(path))
    } else {
        Err(DbError::Connection(format!("The {what} file was not found: {value}")))
    }
}

fn options(profile: &ConnectionProfile, secret: Option<&SecretString>) -> Result<MySqlConnectOptions, DbError> {
    let mut opts = MySqlConnectOptions::new()
        .host(&profile.host)
        .port(profile.port.unwrap_or(3306))
        .username(&profile.username)
        .charset("utf8mb4")
        .ssl_mode(ssl_mode(profile)?);
    if let Some(db) = &profile.database {
        opts = opts.database(db);
    }
    if let Some(secret) = secret {
        opts = opts.password(secret.expose_secret());
    }
    if let Some(path) = file_option(profile, "sslrootcert", "CA certificate")? {
        opts = opts.ssl_ca(path);
    }
    if let Some(path) = file_option(profile, "sslcert", "client certificate")? {
        opts = opts.ssl_client_cert(path);
    }
    if let Some(path) = file_option(profile, "sslkey", "client key")? {
        opts = opts.ssl_client_key(path);
    }
    // Statement text may contain sensitive literals; do not log it.
    Ok(opts.disable_statement_logging())
}

async fn open(opts: &MySqlConnectOptions) -> Result<MySqlConnection, DbError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, opts.connect()).await {
        Ok(result) => result.map_err(map_connect_error),
        Err(_) => Err(DbError::Connection("connection timed out".into())),
    }
}

#[async_trait]
impl DatabaseDriver for MySqlDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::new("mysql")
    }

    fn dialect(&self) -> Dialect {
        MYSQL_DIALECT
    }

    async fn connect(
        &self,
        profile: &ConnectionProfile,
        secret: Option<SecretString>,
    ) -> Result<Box<dyn DbSession>, DbError> {
        let opts = options(profile, secret.as_ref())?;
        let conn = open(&opts).await?;
        // A second connection lets `cancel` reach the server while a statement is running.
        let control = open(&opts).await?;
        Ok(Box::new(MySqlSession::new(conn, control).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(options: &[(&str, &str)]) -> ConnectionProfile {
        let mut p = ConnectionProfile::new("p", DriverKind::new("mysql"), "localhost");
        for (k, v) in options {
            p.options.insert((*k).into(), (*v).into());
        }
        p
    }

    #[test]
    fn sslmode_defaults_to_prefer_and_maps_the_shared_names() {
        assert!(matches!(ssl_mode(&profile(&[])), Ok(MySqlSslMode::Preferred)));
        assert!(matches!(ssl_mode(&profile(&[("sslmode", "disable")])), Ok(MySqlSslMode::Disabled)));
        assert!(matches!(ssl_mode(&profile(&[("sslmode", "require")])), Ok(MySqlSslMode::Required)));
        assert!(matches!(ssl_mode(&profile(&[("sslmode", "verify-ca")])), Ok(MySqlSslMode::VerifyCa)));
        assert!(matches!(ssl_mode(&profile(&[("sslmode", "verify-full")])), Ok(MySqlSslMode::VerifyIdentity)));
        assert!(matches!(ssl_mode(&profile(&[("sslmode", "bogus")])), Err(DbError::Unsupported(_))));
    }

    #[test]
    fn tls_file_options_must_point_at_existing_files() {
        for key in ["sslrootcert", "sslcert", "sslkey"] {
            let err = options(&profile(&[(key, "/definitely/not/here.pem")]), None).err();
            assert!(matches!(&err, Some(DbError::Connection(m)) if m.contains("not found")), "{key}: {err:?}");
        }
        assert!(options(&profile(&[("sslrootcert", "  ")]), None).is_ok());
    }

    #[test]
    fn driver_identity() {
        let d = MySqlDriver::new();
        assert_eq!(d.kind().as_str(), "mysql");
        assert_eq!(d.dialect().identifier_quote, '`');
        assert!(!d.dialect().quote_mixed_case);
    }
}
