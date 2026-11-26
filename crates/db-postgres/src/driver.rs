use std::time::Duration;

use application::ports::{DatabaseDriver, DbError, DbSession};
use async_trait::async_trait;
use domain::{ConnectionProfile, Dialect, DriverKind};
use secrecy::{ExposeSecret, SecretString};
use sqlx::ConnectOptions;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgSslMode};

use crate::dialect::POSTGRES_DIALECT;
use crate::errors::map_connect_error;
use crate::session::PostgresSession;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Clone, Copy)]
pub struct PostgresDriver;

impl PostgresDriver {
    pub fn new() -> Self {
        Self
    }
}

fn ssl_mode(profile: &ConnectionProfile) -> Result<PgSslMode, DbError> {
    match profile.options.get("sslmode").map(String::as_str) {
        None | Some("prefer") => Ok(PgSslMode::Prefer),
        Some("disable") => Ok(PgSslMode::Disable),
        Some("allow") => Ok(PgSslMode::Allow),
        Some("require") => Ok(PgSslMode::Require),
        Some("verify-ca") => Ok(PgSslMode::VerifyCa),
        Some("verify-full") => Ok(PgSslMode::VerifyFull),
        Some(other) => Err(DbError::Unsupported(format!("sslmode `{other}`"))),
    }
}

fn options(
    profile: &ConnectionProfile,
    secret: Option<&SecretString>,
) -> Result<PgConnectOptions, DbError> {
    let mut opts = PgConnectOptions::new_without_pgpass()
        .host(&profile.host)
        .port(profile.port.unwrap_or(5432))
        .username(&profile.username)
        .application_name("dbiever")
        .ssl_mode(ssl_mode(profile)?);
    if let Some(db) = &profile.database {
        opts = opts.database(db);
    }
    if let Some(secret) = secret {
        opts = opts.password(secret.expose_secret());
    }
    // Statement text may contain sensitive literals; do not log it.
    Ok(opts.disable_statement_logging())
}

async fn open(opts: &PgConnectOptions) -> Result<PgConnection, DbError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, opts.connect()).await {
        Ok(result) => result.map_err(map_connect_error),
        Err(_) => Err(DbError::Connection("connection timed out".into())),
    }
}

#[async_trait]
impl DatabaseDriver for PostgresDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::new("postgres")
    }

    fn dialect(&self) -> Dialect {
        POSTGRES_DIALECT
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
        Ok(Box::new(PostgresSession::new(conn, control).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(sslmode: Option<&str>) -> ConnectionProfile {
        let mut p = ConnectionProfile::new("p", DriverKind::new("postgres"), "localhost");
        if let Some(m) = sslmode {
            p.options.insert("sslmode".into(), m.into());
        }
        p
    }

    #[test]
    fn sslmode_defaults_to_prefer_and_parses_known_values() {
        assert!(matches!(ssl_mode(&profile(None)), Ok(PgSslMode::Prefer)));
        assert!(matches!(
            ssl_mode(&profile(Some("require"))),
            Ok(PgSslMode::Require)
        ));
        assert!(matches!(
            ssl_mode(&profile(Some("verify-full"))),
            Ok(PgSslMode::VerifyFull)
        ));
    }

    #[test]
    fn unknown_sslmode_is_rejected() {
        assert!(matches!(
            ssl_mode(&profile(Some("bogus"))),
            Err(DbError::Unsupported(_))
        ));
    }

    #[test]
    fn driver_identity() {
        let d = PostgresDriver::new();
        assert_eq!(d.kind().as_str(), "postgres");
        assert_eq!(d.dialect().identifier_quote, '"');
    }
}
