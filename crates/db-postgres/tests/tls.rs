//! TLS behaviour against a TLS-enabled PostgreSQL.
//!
//! Needs a server with `ssl = on` whose certificate is valid for the test host (SAN `127.0.0.1`
//! or the host in `FATHOM_TEST_PG_HOST`), plus:
//! - `FATHOM_TEST_PG_TLS_CA`: the CA certificate that signed the server certificate
//! - `FATHOM_TEST_PG_TLS_OTHER_CA`: an unrelated CA certificate (must NOT verify the server)
//!
//! Without these variables the tests skip, because plain CI servers have no TLS.
#![cfg(feature = "integration")]
#![allow(clippy::expect_used)] // test helpers

mod common;

use application::ports::{DatabaseDriver, DbError, DbSession};
use db_postgres::PostgresDriver;
use domain::{QueryRequest, Value};

struct Tls {
    ca: String,
    other_ca: String,
}

fn tls() -> Option<Tls> {
    let ca = std::env::var("FATHOM_TEST_PG_TLS_CA").ok()?;
    let other_ca = std::env::var("FATHOM_TEST_PG_TLS_OTHER_CA").ok()?;
    Some(Tls { ca, other_ca })
}

async fn connect(options: &[(&str, &str)]) -> Result<Box<dyn DbSession>, DbError> {
    let s = common::server().await;
    let (mut profile, password) = common::profile_for(&s);
    profile.options.clear();
    for (k, v) in options {
        profile.options.insert((*k).into(), (*v).into());
    }
    PostgresDriver::new()
        .connect(&profile, Some(password))
        .await
}

async fn is_encrypted(session: &dyn DbSession) -> bool {
    let r = session
        .execute(QueryRequest::sql(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
        ))
        .await
        .expect("pg_stat_ssl");
    r.rows.first().and_then(|row| row.first()) == Some(&Value::Bool(true))
}

#[tokio::test(flavor = "multi_thread")]
async fn require_encrypts_and_disable_does_not() {
    let Some(_) = tls() else {
        return eprintln!("skipped: TLS environment not set");
    };
    let on = connect(&[("sslmode", "require")]).await.expect("require");
    assert!(
        is_encrypted(on.as_ref()).await,
        "sslmode=require must encrypt"
    );
    let off = connect(&[("sslmode", "disable")]).await.expect("disable");
    assert!(
        !is_encrypted(off.as_ref()).await,
        "sslmode=disable must not"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_full_and_verify_ca_accept_a_server_signed_by_the_given_ca() {
    let Some(t) = tls() else {
        return eprintln!("skipped: TLS environment not set");
    };
    for mode in ["verify-full", "verify-ca"] {
        let s = connect(&[("sslmode", mode), ("sslrootcert", &t.ca)])
            .await
            .expect(mode);
        assert!(is_encrypted(s.as_ref()).await, "{mode}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn verification_rejects_a_server_the_given_ca_did_not_sign() {
    let Some(t) = tls() else {
        return eprintln!("skipped: TLS environment not set");
    };
    for mode in ["verify-full", "verify-ca"] {
        let err = connect(&[("sslmode", mode), ("sslrootcert", &t.other_ca)])
            .await
            .err();
        assert!(
            matches!(&err, Some(DbError::Connection(_))),
            "{mode} with the wrong CA must fail: {err:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn verification_without_a_ca_fails_for_a_private_ca_instead_of_trusting_blindly() {
    let Some(_) = tls() else {
        return eprintln!("skipped: TLS environment not set");
    };
    let err = connect(&[("sslmode", "verify-full")]).await.err();
    assert!(matches!(&err, Some(DbError::Connection(_))), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_certificate_file_is_reported_before_connecting() {
    let err = connect(&[
        ("sslmode", "verify-full"),
        ("sslrootcert", "/no/such/ca.pem"),
    ])
    .await
    .err();
    assert!(
        matches!(&err, Some(DbError::Connection(m)) if m.contains("not found")),
        "{err:?}"
    );
}
