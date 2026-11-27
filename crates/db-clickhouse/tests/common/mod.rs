#![allow(dead_code)]
//! Shared helpers for the integration tests.
//!
//! Point the tests at a server through the HTTP interface:
//! `FATHOM_TEST_CLICKHOUSE_HOST` (required), `_PORT` (8123), `_USER` (default), `_PASSWORD`
//! (empty) and `_DB` (default). The user needs rights to create tables and to `KILL QUERY`.

use domain::{ConnectionProfile, DriverKind};
use secrecy::SecretString;

pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

pub fn server() -> Server {
    let Ok(host) = std::env::var("FATHOM_TEST_CLICKHOUSE_HOST") else {
        panic!("set FATHOM_TEST_CLICKHOUSE_HOST (and friends) to run the ClickHouse tests");
    };
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    Server {
        host,
        port: var("FATHOM_TEST_CLICKHOUSE_PORT", "8123")
            .parse()
            .unwrap_or(8123),
        user: var("FATHOM_TEST_CLICKHOUSE_USER", "default"),
        password: var("FATHOM_TEST_CLICKHOUSE_PASSWORD", ""),
        database: var("FATHOM_TEST_CLICKHOUSE_DB", "default"),
    }
}

pub fn profile_for(s: &Server) -> (ConnectionProfile, SecretString) {
    let mut profile =
        ConnectionProfile::new("integration", DriverKind::new("clickhouse"), s.host.clone());
    profile.port = Some(s.port);
    profile.username = s.user.clone();
    profile.database = Some(s.database.clone());
    (profile, SecretString::from(s.password.clone()))
}
