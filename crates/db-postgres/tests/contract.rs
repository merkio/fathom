//! Runs the shared `DatabaseDriver` contract against a real PostgreSQL.
//!
//! Enable with `--features integration` and point at a server through environment variables:
//! `DBIEVER_TEST_PG_HOST`, `_PORT`, `_USER`, `_PASSWORD`, `_DB` (host and password required).
//! With `--features containers` a server is started through Docker instead.
#![cfg(feature = "integration")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use db_postgres::PostgresDriver;
use secrecy::SecretString;
use test_support::driver_contract::{DriverFixture, Sleeper, run_database_driver_contract};

#[tokio::test(flavor = "multi_thread")]
async fn postgres_passes_the_driver_contract() {
    let s = common::server().await;
    let (profile, password) = common::profile_for(&s);
    // Nothing listens on port 1.
    let mut unreachable = profile.clone();
    unreachable.port = Some(1);

    let fixture = DriverFixture {
        profile,
        password: Some(password),
        wrong_password: Some(SecretString::from("definitely-not-the-password")),
        unreachable,
        setup: vec![
            "CREATE TABLE contract_parent (id integer PRIMARY KEY, name text NOT NULL, note text)"
                .into(),
            "CREATE TABLE contract_child (id integer PRIMARY KEY, \
             parent_id integer REFERENCES contract_parent(id), label text)"
                .into(),
            "CREATE INDEX contract_child_parent_idx ON contract_child (parent_id)".into(),
        ],
        teardown: vec![
            "DROP TABLE IF EXISTS contract_child".into(),
            "DROP TABLE IF EXISTS contract_parent".into(),
        ],
        sleep_statement: "SELECT pg_sleep(30)".into(),
    };
    let sleep: Sleeper = Arc::new(|d: Duration| Box::pin(tokio::time::sleep(d)));
    run_database_driver_contract(&PostgresDriver::new(), &fixture, sleep).await;
}
