//! Data use cases: schema browsing, query execution and query history.

use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{ConnectionId, Dialect, DomainError, QueryRequest, QueryResult, SchemaTree};

use crate::error::UseCaseError;
use crate::llm::SqlSafetyGuard;
use crate::ports::{Clock, DbSession, HistoryEntry, IntrospectScope, QueryHistoryRepository};

pub struct BrowseSchema;

impl BrowseSchema {
    pub async fn execute(
        &self,
        session: &dyn DbSession,
        scope: IntrospectScope,
    ) -> Result<SchemaTree, UseCaseError> {
        Ok(session.introspect(scope).await?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteOptions {
    /// Applied when a request carries no row limit, so a stray `SELECT *` cannot flood the UI.
    pub default_row_limit: Option<u32>,
    /// Applied when a request carries no timeout.
    pub default_timeout: Option<Duration>,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            default_row_limit: Some(1000),
            default_timeout: Some(Duration::from_secs(60)),
        }
    }
}

pub struct ExecuteQuery {
    history: Arc<dyn QueryHistoryRepository>,
    clock: Arc<dyn Clock>,
    options: ExecuteOptions,
}

impl ExecuteQuery {
    pub fn new(
        history: Arc<dyn QueryHistoryRepository>,
        clock: Arc<dyn Clock>,
        options: ExecuteOptions,
    ) -> Self {
        Self {
            history,
            clock,
            options,
        }
    }

    /// Like `execute`, but in read-only mode only plain reads run: anything the safety guard
    /// classifies as write, DDL, admin or unknown (including SQL that does not parse) is blocked
    /// before it reaches the database and is not recorded in the history.
    pub async fn execute_guarded(
        &self,
        connection: ConnectionId,
        session: &dyn DbSession,
        req: QueryRequest,
        dialect: &Dialect,
        read_only: bool,
    ) -> Result<QueryResult, UseCaseError> {
        if read_only {
            let report = SqlSafetyGuard.classify(&req.text, dialect);
            if !report.is_read_only() {
                let kind = report.overall().map_or("empty", |k| k.label());
                return Err(UseCaseError::Blocked(format!(
                    "Read-only mode: this connection only allows plain reads, found a `{kind}` statement"
                )));
            }
        }
        self.execute(connection, session, req).await
    }

    /// Runs the request on the session and records it in the history. A failing history write
    /// never fails the query.
    pub async fn execute(
        &self,
        connection: ConnectionId,
        session: &dyn DbSession,
        mut req: QueryRequest,
    ) -> Result<QueryResult, UseCaseError> {
        if req.text.trim().is_empty() {
            return Err(DomainError::Invalid {
                field: "query",
                reason: "is empty".into(),
            }
            .into());
        }
        req.row_limit = req.row_limit.or(self.options.default_row_limit);
        req.timeout = req.timeout.or(self.options.default_timeout);

        let text = req.text.clone();
        let started = Instant::now();
        let outcome = session.execute(req).await;
        let _ = self
            .history
            .append(HistoryEntry {
                connection_id: connection,
                text,
                executed_at: self.clock.now(),
                elapsed: started.elapsed(),
                succeeded: outcome.is_ok(),
            })
            .await;
        Ok(outcome?)
    }
}

pub struct QueryHistory {
    history: Arc<dyn QueryHistoryRepository>,
}

impl QueryHistory {
    pub fn new(history: Arc<dyn QueryHistoryRepository>) -> Self {
        Self { history }
    }

    /// Most recent first.
    pub async fn recent(
        &self,
        connection: ConnectionId,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, UseCaseError> {
        Ok(self.history.recent(connection, limit).await?)
    }
}

/// Applies the row edits a user staged in a result grid.
///
/// The statements are generated from structured changes (never from text), keyed by primary key
/// and bound as parameters; nothing is applied unless the caller passes the user's explicit
/// confirmation by calling this at all, and never on a read-only connection.
pub struct ApplyChanges;

impl ApplyChanges {
    pub async fn apply(
        &self,
        session: &dyn crate::ports::DbSession,
        read_only: bool,
        changes: domain::TableChanges,
    ) -> Result<domain::ApplyOutcome, crate::error::UseCaseError> {
        if read_only {
            return Err(crate::error::UseCaseError::Blocked(
                "This connection is read-only, so nothing was changed.".into(),
            ));
        }
        if changes.changes.is_empty() {
            return Err(crate::error::UseCaseError::Blocked(
                "There are no changes to apply.".into(),
            ));
        }
        Ok(session.apply_changes(changes).await?)
    }
}

#[cfg(test)]
mod apply_tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use domain::{
        ApplyOutcome, QueryRequest, QueryResult, RowChange, SchemaTree, TableChanges, Value,
    };

    use super::*;
    use crate::error::UseCaseError;
    use crate::ports::{DbError, DbSession, IntrospectScope};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<TableChanges>>);

    #[async_trait]
    impl DbSession for Recorder {
        async fn introspect(&self, _: IntrospectScope) -> Result<SchemaTree, DbError> {
            Ok(SchemaTree::default())
        }
        async fn execute(&self, _: QueryRequest) -> Result<QueryResult, DbError> {
            Ok(QueryResult::empty())
        }
        async fn cancel(&self) -> Result<(), DbError> {
            Ok(())
        }
        async fn apply_changes(&self, c: TableChanges) -> Result<ApplyOutcome, DbError> {
            let n = c.changes.len();
            self.0.lock().unwrap().push(c);
            Ok(ApplyOutcome {
                applied: n,
                rows_affected: Some(n as u64),
                atomic: true,
            })
        }
        async fn close(self: Box<Self>) {}
    }

    /// A session that does not override `apply_changes`.
    struct Plain;

    #[async_trait]
    impl DbSession for Plain {
        async fn introspect(&self, _: IntrospectScope) -> Result<SchemaTree, DbError> {
            Ok(SchemaTree::default())
        }
        async fn execute(&self, _: QueryRequest) -> Result<QueryResult, DbError> {
            Ok(QueryResult::empty())
        }
        async fn cancel(&self) -> Result<(), DbError> {
            Ok(())
        }
        async fn close(self: Box<Self>) {}
    }

    fn one_change() -> TableChanges {
        TableChanges {
            schema: None,
            table: "t".into(),
            column_types: vec![],
            changes: vec![RowChange::Delete {
                key: vec![("id".into(), Value::Int(1))],
            }],
        }
    }

    #[test]
    fn a_read_only_connection_never_reaches_the_database() {
        let session = Recorder::default();
        let err = futures::executor::block_on(ApplyChanges.apply(&session, true, one_change()));
        assert!(matches!(err, Err(UseCaseError::Blocked(_))));
        assert!(session.0.lock().unwrap().is_empty());
    }

    #[test]
    fn an_empty_batch_is_refused() {
        let mut empty = one_change();
        empty.changes.clear();
        let session = Recorder::default();
        let err = futures::executor::block_on(ApplyChanges.apply(&session, false, empty));
        assert!(matches!(err, Err(UseCaseError::Blocked(_))));
        assert!(session.0.lock().unwrap().is_empty());
    }

    #[test]
    fn changes_reach_the_session_untouched() {
        let session = Recorder::default();
        let out =
            futures::executor::block_on(ApplyChanges.apply(&session, false, one_change())).unwrap();
        assert_eq!(out.applied, 1);
        assert_eq!(session.0.lock().unwrap().as_slice(), [one_change()]);
    }

    #[test]
    fn databases_without_support_say_so() {
        let err = futures::executor::block_on(ApplyChanges.apply(&Plain, false, one_change()));
        assert!(matches!(
            err,
            Err(UseCaseError::Db(DbError::Unsupported(_)))
        ));
    }
}
