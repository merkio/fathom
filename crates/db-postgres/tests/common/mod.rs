#![allow(dead_code, clippy::expect_used)]
//! Shared helpers for the integration tests.

use domain::{ConnectionProfile, DriverKind};
use secrecy::SecretString;

pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    #[cfg(feature = "containers")]
    _container: Option<Box<dyn std::any::Any>>,
}

pub async fn server() -> Server {
    if let Ok(host) = std::env::var("DBIEVER_TEST_PG_HOST") {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        return Server {
            host,
            port: var("DBIEVER_TEST_PG_PORT", "5432").parse().unwrap_or(5432),
            user: var("DBIEVER_TEST_PG_USER", "postgres"),
            password: var("DBIEVER_TEST_PG_PASSWORD", ""),
            database: var("DBIEVER_TEST_PG_DB", "postgres"),
            #[cfg(feature = "containers")]
            _container: None,
        };
    }
    #[cfg(feature = "containers")]
    {
        use testcontainers_modules::postgres::Postgres;
        use testcontainers_modules::testcontainers::runners::AsyncRunner;
        let container = Postgres::default()
            .start()
            .await
            .expect("start postgres container");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("mapped port");
        Server {
            host: "127.0.0.1".into(),
            port,
            user: "postgres".into(),
            password: "postgres".into(),
            database: "postgres".into(),
            _container: Some(Box::new(container)),
        }
    }
    #[cfg(not(feature = "containers"))]
    panic!("set DBIEVER_TEST_PG_HOST (and friends) or enable the `containers` feature");
}

/// Profile and password for the server, with TLS off (the test servers have none).
pub fn profile_for(s: &Server) -> (ConnectionProfile, SecretString) {
    let mut profile =
        ConnectionProfile::new("integration", DriverKind::new("postgres"), s.host.clone());
    profile.port = Some(s.port);
    profile.username = s.user.clone();
    profile.database = Some(s.database.clone());
    profile.options.insert("sslmode".into(), "disable".into());
    (profile, SecretString::from(s.password.clone()))
}
