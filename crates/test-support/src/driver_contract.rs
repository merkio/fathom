//! Reusable contract suite for `DatabaseDriver` adapters. Every `db-*` crate runs it against a
//! real server, so new adapters are verified by the same checks.
//!
//! The suite panics on failure (it is meant to be called from a test). It is runtime-agnostic:
//! the caller supplies a `sleep` function for the cancel scenario.

use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};

use application::ports::{DatabaseDriver, DbError, DbSession, IntrospectScope};
use domain::{ConnectionProfile, QueryRequest, SchemaTree, Table, Value};
use futures::future::BoxFuture;
use secrecy::SecretString;

/// Async sleep supplied by the caller's runtime.
pub type Sleeper = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// What the database under test cannot do. Each `false` skips the matching check; the default is
/// a full relational database.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// Enforces and reports foreign keys.
    pub foreign_keys: bool,
    /// `UPDATE` runs synchronously and reports how many rows it changed (analytical stores that
    /// apply updates as background mutations do not).
    pub update_row_counts: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            foreign_keys: true,
            update_row_counts: true,
        }
    }
}

/// Everything dialect-specific the suite needs.
pub struct DriverFixture {
    pub profile: ConnectionProfile,
    pub password: Option<SecretString>,
    /// A password the server must reject. `None` for databases without authentication (for
    /// example a local file), which skip that check.
    pub wrong_password: Option<SecretString>,
    /// A profile that cannot be reached (nothing listens there, or the file does not exist).
    /// Connecting must fail with `DbError::Connection`.
    pub unreachable: ConnectionProfile,
    /// Creates `contract_parent(id PK, name NOT NULL, note NULL)` and
    /// `contract_child(id PK, parent_id FK -> contract_parent(id), label)` with a plain
    /// (non-unique) index on `contract_child(parent_id)`.
    pub setup: Vec<String>,
    /// Removes everything `setup` created. Must be safe to run when nothing exists.
    pub teardown: Vec<String>,
    /// A statement that runs for at least 20 seconds unless cancelled.
    pub sleep_statement: String,
    pub capabilities: Capabilities,
}

fn must<T, E: Debug>(r: Result<T, E>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("contract: {what} failed: {e:?}"),
    }
}

fn table<'a>(tree: &'a SchemaTree, name: &str) -> &'a Table {
    match tree.find_table(name) {
        Some((_, t)) => t,
        None => panic!("contract: introspection did not return table `{name}`"),
    }
}

async fn connect(driver: &dyn DatabaseDriver, f: &DriverFixture) -> Box<dyn DbSession> {
    must(
        driver.connect(&f.profile, f.password.clone()).await,
        "connect",
    )
}

async fn run(session: &dyn DbSession, sql: &str) -> domain::QueryResult {
    must(session.execute(QueryRequest::sql(sql)).await, sql)
}

pub async fn run_database_driver_contract(
    driver: &dyn DatabaseDriver,
    f: &DriverFixture,
    sleep: Sleeper,
) {
    metadata(driver);
    connection_errors(driver, f).await;

    let session = connect(driver, f).await;
    for sql in &f.teardown {
        run(session.as_ref(), sql).await;
    }
    for sql in &f.setup {
        run(session.as_ref(), sql).await;
    }

    values(session.as_ref()).await;
    errors(session.as_ref()).await;
    introspection(session.as_ref(), f).await;
    writes_and_limits(session.as_ref(), f).await;
    timeout_and_cancel(session.as_ref(), f, &sleep).await;

    for sql in &f.teardown {
        run(session.as_ref(), sql).await;
    }
    session.close().await;
}

fn metadata(driver: &dyn DatabaseDriver) {
    assert!(
        !driver.kind().as_str().is_empty(),
        "contract: empty driver kind"
    );
    let d = driver.dialect();
    assert!(
        !d.keywords.is_empty() && !d.functions.is_empty(),
        "contract: dialect lists empty"
    );
    assert_eq!(
        d.quote_identifier("a"),
        format!("{0}a{0}", d.identifier_quote)
    );
}

async fn connection_errors(driver: &dyn DatabaseDriver, f: &DriverFixture) {
    if let Some(wrong) = &f.wrong_password {
        let bad = driver.connect(&f.profile, Some(wrong.clone())).await.err();
        assert_eq!(
            bad,
            Some(DbError::Authentication),
            "contract: wrong password"
        );
    }

    match driver
        .connect(&f.unreachable, f.password.clone())
        .await
        .err()
    {
        Some(DbError::Connection(_)) => {}
        other => panic!("contract: unreachable target gave {other:?}"),
    }
}

async fn values(session: &dyn DbSession) {
    let r = run(session, "SELECT 1 AS n, 'x' AS t, NULL AS missing").await;
    assert_eq!(
        r.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["n", "t", "missing"]
    );
    assert_eq!(
        r.rows,
        vec![vec![Value::Int(1), Value::Text("x".into()), Value::Null]]
    );
    assert!(!r.truncated);
    assert_eq!(
        r.rows_affected, None,
        "contract: SELECT reports rows_affected"
    );
}

async fn errors(session: &dyn DbSession) {
    match session
        .execute(QueryRequest::sql("SELEKT nonsense"))
        .await
        .err()
    {
        Some(DbError::Query(_)) => {}
        other => panic!("contract: syntax error gave {other:?}"),
    }
    // The session survives a failed statement.
    run(session, "SELECT 1").await;
}

async fn introspection(session: &dyn DbSession, f: &DriverFixture) {
    let tree = must(session.introspect(IntrospectScope::All).await, "introspect");

    let parent = table(&tree, "contract_parent");
    assert_eq!(parent.primary_key, ["id"]);
    let names: Vec<_> = parent.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "name", "note"], "contract: column order");
    assert!(
        parent.column("name").is_some_and(|c| !c.nullable),
        "contract: NOT NULL lost"
    );
    assert!(
        parent.column("note").is_some_and(|c| c.nullable),
        "contract: nullable lost"
    );

    let child = table(&tree, "contract_child");
    assert_eq!(child.primary_key, ["id"]);
    if f.capabilities.foreign_keys {
        assert_eq!(child.foreign_keys.len(), 1, "contract: foreign keys");
        let fk = &child.foreign_keys[0];
        assert_eq!(fk.columns, ["parent_id"]);
        assert!(fk.referenced_table.eq_ignore_ascii_case("contract_parent"));
        assert_eq!(fk.referenced_columns, ["id"]);
    } else {
        assert!(
            child.foreign_keys.is_empty(),
            "contract: foreign keys reported by a database without them"
        );
    }
    assert!(
        child
            .indexes
            .iter()
            .any(|i| i.columns == ["parent_id"] && !i.unique),
        "contract: index on parent_id missing from {:?}",
        child.indexes
    );

    let none = must(
        session
            .introspect(IntrospectScope::Schema("no_such_schema_xyz".into()))
            .await,
        "introspect missing schema",
    );
    assert_eq!(
        none.tables().count(),
        0,
        "contract: unknown schema returned tables"
    );
}

async fn writes_and_limits(session: &dyn DbSession, f: &DriverFixture) {
    let r = run(session, "INSERT INTO contract_parent (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, 'd'), (5, 'e')").await;
    assert_eq!(r.rows_affected, Some(5));
    assert!(r.rows.is_empty());

    let mut req = QueryRequest::sql("SELECT id, note FROM contract_parent ORDER BY id");
    req.row_limit = Some(3);
    let limited = must(session.execute(req).await, "limited select");
    assert_eq!(limited.rows.len(), 3);
    assert!(limited.truncated, "contract: truncated flag");
    assert_eq!(limited.rows[0], vec![Value::Int(1), Value::Null]);

    let mut req = QueryRequest::sql("SELECT id FROM contract_parent");
    req.row_limit = Some(5);
    let exact = must(session.execute(req).await, "exact limit");
    assert!(
        !exact.truncated,
        "contract: limit equal to row count must not truncate"
    );

    if f.capabilities.update_row_counts {
        let r = run(
            session,
            "UPDATE contract_parent SET note = 'x' WHERE id <= 2",
        )
        .await;
        assert_eq!(r.rows_affected, Some(2));
    }

    let r = run(session, "SELECT id FROM contract_parent WHERE id < 0").await;
    assert!(r.rows.is_empty() && !r.truncated);
}

async fn timeout_and_cancel(session: &dyn DbSession, f: &DriverFixture, sleep: &Sleeper) {
    let started = Instant::now();
    let mut req = QueryRequest::sql(f.sleep_statement.clone());
    req.timeout = Some(Duration::from_millis(300));
    assert_eq!(session.execute(req).await.err(), Some(DbError::Timeout));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "contract: timeout did not stop the wait"
    );
    run(session, "SELECT 1").await; // still usable

    let started = Instant::now();
    let running = session.execute(QueryRequest::sql(f.sleep_statement.clone()));
    let canceller = async {
        sleep(Duration::from_millis(400)).await;
        session.cancel().await
    };
    let (result, cancelled) = futures::join!(running, canceller);
    must(cancelled, "cancel");
    assert_eq!(
        result.err(),
        Some(DbError::Cancelled),
        "contract: cancelled statement"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "contract: cancel did not interrupt"
    );
    run(session, "SELECT 1").await; // still usable
}
