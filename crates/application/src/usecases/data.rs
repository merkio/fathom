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
