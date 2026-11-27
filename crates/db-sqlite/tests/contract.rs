//! Runs the shared `DatabaseDriver` contract against a real SQLite file, and checks the
//! behaviour specific to SQLite.
#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;
use std::time::Duration;

use application::ports::{DatabaseDriver, DbError, DbSession, IntrospectScope};
use db_sqlite::SqliteDriver;
use domain::{ConnectionProfile, DriverKind, QueryLanguage, QueryRequest, TableKind, Value};
use test_support::driver_contract::{
    Capabilities, DriverFixture, Sleeper, run_database_driver_contract,
};

fn profile(path: &std::path::Path, options: &[(&str, &str)]) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("sqlite", DriverKind::new("sqlite"), "local file");
    p.database = Some(path.display().to_string());
    for (k, v) in options {
        p.options.insert((*k).into(), (*v).into());
    }
    p
}

async fn connect(path: &std::path::Path, options: &[(&str, &str)]) -> Box<dyn DbSession> {
    SqliteDriver::new()
        .connect(&profile(path, options), None)
        .await
        .expect("connect")
}

async fn exec(s: &dyn DbSession, sql: &str) -> domain::QueryResult {
    s.execute(QueryRequest::sql(sql))
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_passes_the_driver_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fixture = DriverFixture {
        profile: profile(&dir.path().join("contract.db"), &[("create", "true")]),
        password: None,
        wrong_password: None, // a file has no authentication
        unreachable: profile(&dir.path().join("missing-dir").join("x.db"), &[]),
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
        // Counts to two billion in a recursive query: minutes of work unless interrupted.
        sleep_statement:
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2000000000) \
                          SELECT count(*) FROM c"
                .into(),
        capabilities: Capabilities::default(),
    };
    let sleep: Sleeper = Arc::new(|d: Duration| Box::pin(tokio::time::sleep(d)));
    run_database_driver_contract(&SqliteDriver::new(), &fixture, sleep).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_file_is_an_error_and_is_not_created_by_accident() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("typo.db");
    let err = SqliteDriver::new()
        .connect(&profile(&path, &[]), None)
        .await
        .err();
    assert!(matches!(err, Some(DbError::Connection(_))), "{err:?}");
    assert!(!path.exists(), "a typo must not create an empty database");

    // Asking for it explicitly creates it.
    connect(&path, &[("create", "true")]).await.close().await;
    assert!(path.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_path_or_a_file_that_is_not_a_database_is_a_clear_connection_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut p = profile(&dir.path().join("x.db"), &[]);
    p.database = None;
    let err = SqliteDriver::new().connect(&p, None).await.err();
    assert!(
        matches!(&err, Some(DbError::Connection(m)) if m.contains("Database field")),
        "{err:?}"
    );

    let junk = dir.path().join("junk.db");
    std::fs::write(
        &junk,
        "this is not a sqlite database, just some text padded to be long enough ".repeat(8),
    )
    .expect("write");
    let err = SqliteDriver::new()
        .connect(&profile(&junk, &[]), None)
        .await
        .err();
    assert!(matches!(err, Some(DbError::Connection(_))), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_mode_rejects_writes_at_the_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ro.db");
    let rw = connect(&path, &[("create", "true")]).await;
    exec(rw.as_ref(), "CREATE TABLE t (a integer)").await;
    exec(rw.as_ref(), "INSERT INTO t VALUES (1)").await;
    rw.close().await;

    let ro = connect(&path, &[("mode", "ro")]).await;
    assert_eq!(
        exec(ro.as_ref(), "SELECT a FROM t").await.rows,
        vec![vec![Value::Int(1)]]
    );
    let err = ro
        .execute(QueryRequest::sql("INSERT INTO t VALUES (2)"))
        .await
        .err();
    assert!(
        matches!(err, Some(DbError::Query(m)) if m.contains("readonly")),
        "writes fail in the engine"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn data_persists_across_connections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("p.db");
    let a = connect(&path, &[("create", "true")]).await;
    exec(a.as_ref(), "CREATE TABLE t (a integer)").await;
    exec(a.as_ref(), "INSERT INTO t VALUES (42)").await;
    a.close().await;
    let b = connect(&path, &[]).await;
    assert_eq!(
        exec(b.as_ref(), "SELECT a FROM t").await.rows,
        vec![vec![Value::Int(42)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_values_and_reports_columns_even_for_empty_results() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = connect(&dir.path().join("v.db"), &[("create", "true")]).await;
    exec(
        s.as_ref(),
        "CREATE TABLE v (i integer, r real, t text, b blob, n text)",
    )
    .await;
    exec(
        s.as_ref(),
        "INSERT INTO v VALUES (7, 1.5, 'héllo', x'dead', NULL)",
    )
    .await;

    let r = exec(s.as_ref(), "SELECT * FROM v").await;
    assert_eq!(
        r.rows[0],
        vec![
            Value::Int(7),
            Value::Float(1.5),
            Value::Text("héllo".into()),
            Value::Bytes(vec![0xde, 0xad]),
            Value::Null
        ]
    );
    assert_eq!(
        r.columns
            .iter()
            .map(|c| c.data_type.0.as_str())
            .collect::<Vec<_>>(),
        ["integer", "real", "text", "blob", "text"]
    );

    let empty = exec(s.as_ref(), "SELECT * FROM v WHERE i = 99").await;
    assert!(empty.rows.is_empty());
    assert_eq!(
        empty.columns.len(),
        5,
        "column headers are known even without rows"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn introspection_covers_views_composite_keys_and_unique_indexes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = connect(&dir.path().join("i.db"), &[("create", "true")]).await;
    for sql in [
        "CREATE TABLE a (x integer, y integer, v text DEFAULT 'd', PRIMARY KEY (y, x))",
        "CREATE TABLE b (id integer PRIMARY KEY, ax integer, ay integer, FOREIGN KEY (ax, ay) REFERENCES a (x, y))",
        "CREATE UNIQUE INDEX b_unique ON b (ax, ay)",
        "CREATE VIEW b_view AS SELECT id FROM b",
        "CREATE TABLE \"odd \"\"name\" (q integer)",
    ] {
        exec(s.as_ref(), sql).await;
    }
    let tree = s
        .introspect(IntrospectScope::All)
        .await
        .expect("introspect");

    let (schema, a) = tree.find_table("a").expect("a");
    assert_eq!(schema, "main");
    assert_eq!(
        a.primary_key,
        ["y", "x"],
        "key columns in key order, not table order"
    );
    assert_eq!(
        a.column("v").and_then(|c| c.default.as_deref()),
        Some("'d'")
    );

    let (_, b) = tree.find_table("b").expect("b");
    assert_eq!(b.foreign_keys.len(), 1, "two rows, one composite key");
    assert_eq!(b.foreign_keys[0].columns, ["ax", "ay"]);
    assert_eq!(b.foreign_keys[0].referenced_columns, ["x", "y"]);
    assert!(
        b.indexes
            .iter()
            .any(|i| i.name == "b_unique" && i.unique && i.columns == ["ax", "ay"])
    );
    assert!(
        !b.indexes
            .iter()
            .any(|i| i.name.starts_with("sqlite_autoindex")),
        "internal indexes are hidden"
    );

    assert_eq!(
        tree.find_table("b_view").map(|(_, t)| t.kind),
        Some(TableKind::View)
    );
    assert!(
        tree.find_table("odd \"name").is_some(),
        "names that need quoting work"
    );
    assert!(
        tree.tables().all(|(_, t)| !t.name.starts_with("sqlite_")),
        "system tables are hidden"
    );

    let other = s
        .introspect(IntrospectScope::Schema("elsewhere".into()))
        .await
        .expect("introspect");
    assert_eq!(other.tables().count(), 0);
    let main = s
        .introspect(IntrospectScope::Schema("MAIN".into()))
        .await
        .expect("introspect");
    assert!(main.tables().count() >= 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_while_idle_does_not_poison_the_next_statement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = connect(&dir.path().join("c.db"), &[("create", "true")]).await;
    s.cancel().await.expect("cancel");
    s.cancel().await.expect("cancel again");
    assert_eq!(
        exec(s.as_ref(), "SELECT 1").await.rows,
        vec![vec![Value::Int(1)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn other_query_languages_are_refused_and_ddl_reports_no_columns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = connect(&dir.path().join("l.db"), &[("create", "true")]).await;
    let mut req = QueryRequest::sql("db.t.find()");
    req.language = QueryLanguage::Mql;
    assert!(matches!(
        s.execute(req).await.err(),
        Some(DbError::Unsupported(_))
    ));

    let r = exec(s.as_ref(), "CREATE TABLE t (a integer)").await;
    assert!(r.columns.is_empty() && r.rows_affected == Some(0));
}
