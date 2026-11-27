//! Runs the shared `DatabaseDriver` contract against a real ClickHouse server.
//!
//! Enable with `--features integration` and set `FATHOM_TEST_CLICKHOUSE_HOST` (see
//! `common/mod.rs`).
#![cfg(feature = "integration")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use db_clickhouse::ClickHouseDriver;
use secrecy::SecretString;
use test_support::driver_contract::{
    Capabilities, DriverFixture, Sleeper, run_database_driver_contract,
};

#[tokio::test(flavor = "multi_thread")]
async fn clickhouse_passes_the_driver_contract() {
    let s = common::server();
    let (profile, password) = common::profile_for(&s);
    // Nothing listens on port 1.
    let mut unreachable = profile.clone();
    unreachable.port = Some(1);

    let fixture = DriverFixture {
        profile,
        password: Some(password),
        // Only meaningful when the server has a password for the user; with the empty default
        // password ClickHouse's `default` user accepts anything, so skip it then.
        wrong_password: (!s.password.is_empty())
            .then(|| SecretString::from("definitely-not-the-password")),
        unreachable,
        setup: vec![
            "CREATE TABLE contract_parent (id Int32, name String, note Nullable(String)) \
             ENGINE = MergeTree ORDER BY id"
                .into(),
            "CREATE TABLE contract_child (id Int32, parent_id Int32, label Nullable(String), \
             INDEX contract_child_parent_idx parent_id TYPE minmax GRANULARITY 1) \
             ENGINE = MergeTree ORDER BY id"
                .into(),
        ],
        teardown: vec![
            "DROP TABLE IF EXISTS contract_child".into(),
            "DROP TABLE IF EXISTS contract_parent".into(),
        ],
        // One block per row, three seconds each: thirty seconds unless killed.
        sleep_statement: "SELECT sleep(3) FROM numbers(10) SETTINGS max_block_size = 1".into(),
        capabilities: Capabilities {
            foreign_keys: false,
            update_row_counts: false,
        },
    };
    let sleep: Sleeper = Arc::new(|d: Duration| Box::pin(tokio::time::sleep(d)));
    run_database_driver_contract(&ClickHouseDriver::new(), &fixture, sleep).await;
}
