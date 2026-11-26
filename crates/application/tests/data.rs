#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use application::error::UseCaseError;
use application::ports::{DatabaseDriver, DbError, DbSession, IntrospectScope};
use application::usecases::{BrowseSchema, ExecuteOptions, ExecuteQuery, QueryHistory};
use domain::{
    Catalog, ConnectionId, ConnectionProfile, DomainError, DriverKind, QueryRequest, QueryResult,
    Schema, SchemaTree, Table, TableKind,
};
use futures::executor::block_on;
use test_support::{FakeDriver, FixedClock, InMemoryHistory};

fn session(driver: &FakeDriver) -> Box<dyn DbSession> {
    let profile = ConnectionProfile::new("p", DriverKind::new("fake"), "h");
    block_on(driver.connect(&profile, None)).expect("connect")
}

fn execute_query(history: Arc<InMemoryHistory>, options: ExecuteOptions) -> ExecuteQuery {
    ExecuteQuery::new(history, Arc::new(FixedClock(UNIX_EPOCH)), options)
}

#[test]
fn execute_records_history_for_success_and_applies_defaults() {
    let driver = FakeDriver::new("fake").with_result(QueryResult::empty());
    let s = session(&driver);
    let history = Arc::new(InMemoryHistory::default());
    let uc = execute_query(history.clone(), ExecuteOptions::default());
    let id = ConnectionId::new();

    block_on(uc.execute(id, s.as_ref(), QueryRequest::sql("select 1"))).expect("run");

    let entries = block_on(QueryHistory::new(history).recent(id, 10)).expect("history");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "select 1");
    assert!(entries[0].succeeded);
    assert_eq!(entries[0].executed_at, UNIX_EPOCH);
}

#[test]
fn empty_query_is_rejected_and_not_recorded() {
    let driver = FakeDriver::new("fake");
    let s = session(&driver);
    let history = Arc::new(InMemoryHistory::default());
    let uc = execute_query(history.clone(), ExecuteOptions::default());
    let id = ConnectionId::new();

    let err = block_on(uc.execute(id, s.as_ref(), QueryRequest::sql("  \n "))).err();
    assert!(matches!(
        err,
        Some(UseCaseError::Domain(DomainError::Invalid {
            field: "query",
            ..
        }))
    ));
    assert!(driver.executed().is_empty());
    assert!(
        block_on(QueryHistory::new(history).recent(id, 10))
            .expect("history")
            .is_empty()
    );
}

/// Session that fails every statement, to check that failures are recorded and surfaced.
struct FailingSession;

#[async_trait::async_trait]
impl DbSession for FailingSession {
    async fn introspect(&self, _: IntrospectScope) -> Result<SchemaTree, DbError> {
        Err(DbError::Query("nope".into()))
    }
    async fn execute(&self, _: QueryRequest) -> Result<QueryResult, DbError> {
        Err(DbError::Query("boom".into()))
    }
    async fn cancel(&self) -> Result<(), DbError> {
        Ok(())
    }
    async fn close(self: Box<Self>) {}
}

#[test]
fn failed_query_is_recorded_as_failed_and_error_is_returned() {
    let history = Arc::new(InMemoryHistory::default());
    let uc = execute_query(history.clone(), ExecuteOptions::default());
    let id = ConnectionId::new();

    let err = block_on(uc.execute(id, &FailingSession, QueryRequest::sql("bad"))).err();
    assert_eq!(err, Some(UseCaseError::Db(DbError::Query("boom".into()))));
    let entries = block_on(QueryHistory::new(history).recent(id, 10)).expect("history");
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].succeeded);
}

/// Session that records the limits it receives.
struct LimitProbe(std::sync::Mutex<Vec<(Option<u32>, Option<Duration>)>>);

#[async_trait::async_trait]
impl DbSession for LimitProbe {
    async fn introspect(&self, _: IntrospectScope) -> Result<SchemaTree, DbError> {
        Ok(SchemaTree::default())
    }
    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError> {
        self.0
            .lock()
            .expect("lock")
            .push((req.row_limit, req.timeout));
        Ok(QueryResult::empty())
    }
    async fn cancel(&self) -> Result<(), DbError> {
        Ok(())
    }
    async fn close(self: Box<Self>) {}
}

#[test]
fn default_limits_apply_only_when_the_request_has_none() {
    let probe = LimitProbe(Default::default());
    let options = ExecuteOptions {
        default_row_limit: Some(10),
        default_timeout: Some(Duration::from_secs(5)),
    };
    let uc = execute_query(Arc::new(InMemoryHistory::default()), options);
    let id = ConnectionId::new();

    block_on(uc.execute(id, &probe, QueryRequest::sql("a"))).expect("run");
    let mut explicit = QueryRequest::sql("b");
    explicit.row_limit = Some(3);
    explicit.timeout = Some(Duration::from_secs(1));
    block_on(uc.execute(id, &probe, explicit)).expect("run");

    let seen = probe.0.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![
            (Some(10), Some(Duration::from_secs(5))),
            (Some(3), Some(Duration::from_secs(1)))
        ]
    );
}

#[test]
fn browse_schema_returns_the_session_schema() {
    let tree = SchemaTree {
        catalogs: vec![Catalog {
            name: "db".into(),
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![Table::new("users", TableKind::Table)],
            }],
        }],
    };
    let driver = FakeDriver::new("fake").with_schema(tree.clone());
    let s = session(&driver);
    let got = block_on(BrowseSchema.execute(s.as_ref(), IntrospectScope::All)).expect("browse");
    assert_eq!(got, tree);
}
