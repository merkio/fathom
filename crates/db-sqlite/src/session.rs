use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use application::ports::{DbError, DbSession, IntrospectScope};
use async_trait::async_trait;
use domain::{DataType, QueryLanguage, QueryRequest, QueryResult, ResultColumn, SchemaTree, Value};
use rusqlite::{Connection, InterruptHandle};

use crate::introspect;
use crate::values::map_value;

/// One connection. SQLite calls are blocking, so they run on the blocking thread pool; the
/// interrupt handle lets `cancel` and timeouts stop a statement from another thread.
pub(crate) struct SqliteSession {
    conn: Arc<Mutex<Connection>>,
    interrupt: InterruptHandle,
}

fn lock(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Maps a SQLite error; an interrupt means the statement was cancelled.
fn map_error(e: rusqlite::Error) -> DbError {
    if let rusqlite::Error::SqliteFailure(failure, _) = &e
        && failure.code == rusqlite::ErrorCode::OperationInterrupted
    {
        return DbError::Cancelled;
    }
    DbError::Query(e.to_string())
}

impl SqliteSession {
    pub(crate) fn new(conn: Connection) -> Self {
        let interrupt = conn.get_interrupt_handle();
        Self {
            conn: Arc::new(Mutex::new(conn)),
            interrupt,
        }
    }
}

fn run(conn: &Connection, text: &str, limit: Option<usize>) -> Result<QueryResult, DbError> {
    let started = Instant::now();
    let mut stmt = conn.prepare(text).map_err(map_error)?;
    let columns: Vec<ResultColumn> = stmt
        .columns()
        .map(|c| ResultColumn {
            name: c.name().to_owned(),
            data_type: DataType::new(c.decl_type().map_or("any".to_owned(), str::to_lowercase)),
        })
        .collect();

    if columns.is_empty() {
        let affected = stmt.execute([]).map_err(map_error)?;
        return Ok(QueryResult {
            rows_affected: Some(affected as u64),
            elapsed: started.elapsed(),
            ..QueryResult::empty()
        });
    }

    let width = columns.len();
    let mut rows_iter = stmt.query([]).map_err(map_error)?;
    let mut rows: Vec<Vec<Value>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows_iter.next().map_err(map_error)? {
        if limit.is_some_and(|l| rows.len() >= l) {
            truncated = true;
            break;
        }
        let mut cells = Vec::with_capacity(width);
        for i in 0..width {
            cells.push(map_value(row.get_ref(i).map_err(map_error)?));
        }
        rows.push(cells);
    }
    Ok(QueryResult {
        columns,
        rows,
        rows_affected: None,
        elapsed: started.elapsed(),
        truncated,
    })
}

#[async_trait]
impl DbSession for SqliteSession {
    async fn introspect(&self, scope: IntrospectScope) -> Result<SchemaTree, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || introspect::load(&lock(&conn), &scope))
            .await
            .map_err(|e| DbError::Query(e.to_string()))?
    }

    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError> {
        if req.language != QueryLanguage::Sql {
            return Err(DbError::Unsupported("SQLite only runs SQL".into()));
        }
        let conn = Arc::clone(&self.conn);
        let limit = req.row_limit.map(|l| l as usize);
        let text = req.text;
        let work = tokio::task::spawn_blocking(move || run(&lock(&conn), &text, limit));

        let outcome = match req.timeout {
            None => work.await,
            Some(after) => match tokio::time::timeout(after, work).await {
                Ok(joined) => joined,
                Err(_) => {
                    // Stop the statement; the blocked thread then finishes on its own.
                    self.interrupt.interrupt();
                    return Err(DbError::Timeout);
                }
            },
        };
        outcome.map_err(|e| DbError::Query(e.to_string()))?
    }

    async fn cancel(&self) -> Result<(), DbError> {
        self.interrupt.interrupt();
        Ok(())
    }

    async fn close(self: Box<Self>) {
        // Dropping the connection closes the file. Wait for any running statement to end first.
        let conn = Arc::clone(&self.conn);
        let _ = tokio::task::spawn_blocking(move || drop(lock(&conn))).await;
    }
}
