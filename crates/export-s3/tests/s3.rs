//! Runs the S3 destination against a real or emulated S3 endpoint.
//!
//! Enable with `--features integration` and set `FATHOM_TEST_S3_ENDPOINT` (for example
//! `http://127.0.0.1:5000` for moto or `http://127.0.0.1:9090` for S3Mock) and
//! `FATHOM_TEST_S3_BUCKET` (must exist). Optional: `FATHOM_TEST_S3_ACCESS_KEY` /
//! `FATHOM_TEST_S3_SECRET_KEY` (default `test`/`test`) and `FATHOM_TEST_S3_REGION`.
#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use application::ports::{DestinationConfig, DestinationFactory, ExportError};
use export_s3::S3Factory;
use secrecy::SecretString;

fn destination(prefix: &str) -> std::sync::Arc<dyn application::ports::ExportDestination> {
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    let endpoint = std::env::var("FATHOM_TEST_S3_ENDPOINT")
        .expect("set FATHOM_TEST_S3_ENDPOINT to run the S3 tests");
    let bucket = std::env::var("FATHOM_TEST_S3_BUCKET")
        .expect("set FATHOM_TEST_S3_BUCKET to an existing bucket");
    let mut settings = BTreeMap::new();
    settings.insert("bucket".to_owned(), bucket);
    settings.insert("endpoint".to_owned(), endpoint);
    settings.insert(
        "region".to_owned(),
        var("FATHOM_TEST_S3_REGION", "us-east-1"),
    );
    settings.insert("prefix".to_owned(), prefix.to_owned());
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "access_key_id".to_owned(),
        SecretString::from(var("FATHOM_TEST_S3_ACCESS_KEY", "test")),
    );
    secrets.insert(
        "secret_access_key".to_owned(),
        SecretString::from(var("FATHOM_TEST_S3_SECRET_KEY", "test")),
    );
    S3Factory
        .create(&DestinationConfig {
            name: "integration".into(),
            kind: "s3".into(),
            settings,
            secrets,
        })
        .expect("destination")
}

/// A prefix that no earlier run used.
fn unique_prefix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("fathom-test/{}-{nanos}", std::process::id())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_connection_test_passes_and_leaves_no_files_that_matter() {
    destination(&unique_prefix()).test().await.expect("test");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_are_written_under_the_prefix_and_never_overwritten() {
    let d = destination(&unique_prefix());
    let at = d
        .put("exports/customers.csv", b"id\r\n1\r\n".to_vec())
        .await
        .expect("put");
    assert!(
        at.starts_with("s3://") && at.ends_with("/exports/customers.csv"),
        "{at}"
    );

    let again = d.put("exports/customers.csv", b"other".to_vec()).await;
    assert!(
        matches!(again, Err(ExportError::AlreadyExists(_))),
        "{again:?}"
    );

    // A different name in the same folder is fine.
    d.put("exports/orders.csv", b"x".to_vec())
        .await
        .expect("second file");
}

#[tokio::test(flavor = "multi_thread")]
async fn paths_leaving_the_prefix_are_refused_before_any_request() {
    let d = destination(&unique_prefix());
    for bad in ["../x.csv", "/x.csv", "a/../../x.csv", ""] {
        assert!(
            matches!(d.put(bad, vec![1]).await, Err(ExportError::Invalid(_))),
            "{bad:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_bucket_is_reported_not_swallowed() {
    // SAFETY-free: build a destination for a bucket that does not exist.
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    let mut settings = BTreeMap::new();
    settings.insert("bucket".to_owned(), "fathom-no-such-bucket-xyz".to_owned());
    settings.insert(
        "endpoint".to_owned(),
        std::env::var("FATHOM_TEST_S3_ENDPOINT").expect("endpoint"),
    );
    settings.insert(
        "region".to_owned(),
        var("FATHOM_TEST_S3_REGION", "us-east-1"),
    );
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "access_key_id".to_owned(),
        SecretString::from("test".to_owned()),
    );
    secrets.insert(
        "secret_access_key".to_owned(),
        SecretString::from("test".to_owned()),
    );
    let d = S3Factory
        .create(&DestinationConfig {
            name: "missing".into(),
            kind: "s3".into(),
            settings,
            secrets,
        })
        .expect("destination");
    assert!(d.put("a.csv", vec![1]).await.is_err());
    assert!(d.test().await.is_err());
}
