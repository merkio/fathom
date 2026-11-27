//! Runs the shared `DatabaseDriver` contract against a real MySQL or MariaDB.
//!
//! Enable with `--features integration` and point at a server through environment variables
//! (see `common/mod.rs`), or use `--features containers` to start one through Docker.
#![cfg(feature = "integration")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use db_mysql::MySqlDriver;
use secrecy::SecretString;
use test_support::driver_contract::{
    Capabilities, DriverFixture, Sleeper, run_database_driver_contract,
};

#[tokio::test(flavor = "multi_thread")]
async fn mysql_passes_the_driver_contract() {
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
            "CREATE TABLE contract_parent (id int PRIMARY KEY, name varchar(100) NOT NULL, note varchar(100)) ENGINE=InnoDB".into(),
            "CREATE TABLE contract_child (id int PRIMARY KEY, parent_id int, label varchar(100), \
             CONSTRAINT contract_child_fk FOREIGN KEY (parent_id) REFERENCES contract_parent(id)) ENGINE=InnoDB"
                .into(),
            "CREATE INDEX contract_child_parent_idx ON contract_child (parent_id)".into(),
        ],
        teardown: vec![
            "DROP TABLE IF EXISTS contract_child".into(),
            "DROP TABLE IF EXISTS contract_parent".into(),
        ],
        sleep_statement: "SELECT SLEEP(30)".into(),
        capabilities: Capabilities::default(),
    };
    let sleep: Sleeper = Arc::new(|d: Duration| Box::pin(tokio::time::sleep(d)));
    run_database_driver_contract(&MySqlDriver::new(), &fixture, sleep).await;
}
