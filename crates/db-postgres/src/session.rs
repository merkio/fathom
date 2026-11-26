use std::time::{Duration, Instant};

use application::ports::{DbError, DbSession, IntrospectScope};
use async_trait::async_trait;
use domain::{DataType, QueryLanguage};
use domain::{QueryRequest, QueryResult, ResultColumn, SchemaTree};
use futures::StreamExt;
use sqlx::postgres::PgConnection;
use sqlx::{AssertSqlSafe, Column, Either, Executor, Row};
use tokio::sync::Mutex;

use crate::errors::map_error;
use crate::introspect;
use crate::values::{map_row, type_name};

const RESYNC_TIMEOUT: Duration = Duration::from_secs(5);

/// One query connection plus one control connection used only to cancel running statements.
///
/// `execute` and `introspect` share the query connection, so they run one at a time.
pub(crate) struct PostgresSession {
    conn: Mutex<PgConnection>,
    control: Mutex<PgConnection>,
    backend_pid: i32,
}

impl PostgresSession {
    pub(crate) async fn new(
        mut conn: PgConnection,
        control: PgConnection,
    ) -> Result<Self, DbError> {
        let pid: i32 = sqlx::query("SELECT pg_backend_pid()")
            .fetch_one(&mut conn)
            .await
            .map_err(map_error)?
            .try_get(0)
            .map_err(map_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
            control: Mutex::new(control),
            backend_pid: pid,
        })
    }

    async fn cancel_backend(&self) -> Result<(), DbError> {
        let mut control = self.control.lock().await;
        sqlx::query("SELECT pg_cancel_backend($1)")
            .bind(self.backend_pid)
            .execute(&mut *control)
            .await
            .map_err(map_error)?;
        Ok(())
    }

    /// After a statement was abandoned, `sqlx` reports that statement's cancellation error on the
    /// connection's next use. Absorb it here so callers never see a stale error.
    async fn resync(&self) {
        let mut conn = self.conn.lock().await;
        for _ in 0..2 {
            let probe = sqlx::query("SELECT 1").execute(&mut *conn);
            match tokio::time::timeout(RESYNC_TIMEOUT, probe).await {
                Ok(Ok(_)) | Err(_) => break,
                Ok(Err(_)) => continue,
            }
        }
    }

    async fn run(&self, req: &QueryRequest) -> Result<QueryResult, DbError> {
        let started = Instant::now();
        let mut conn = self.conn.lock().await;
        let mut stream = (&mut *conn).fetch_many(sqlx::query(AssertSqlSafe(req.text.clone())));

        let limit = req.row_limit.map(|l| l as usize);
        let mut columns: Vec<ResultColumn> = Vec::new();
        let mut rows = Vec::new();
        let mut affected: u64 = 0;
        let mut truncated = false;

        while let Some(step) = stream.next().await {
            match step.map_err(map_error)? {
                Either::Left(done) => affected += done.rows_affected(),
                Either::Right(row) => {
                    if limit.is_some_and(|l| rows.len() >= l) {
                        truncated = true;
                        break;
                    }
                    if columns.is_empty() {
                        columns = row
                            .columns()
                            .iter()
                            .enumerate()
                            .map(|(i, c)| ResultColumn {
                                name: c.name().to_owned(),
                                data_type: DataType::new(type_name(&row, i)),
                            })
                            .collect();
                    }
                    rows.push(map_row(&row));
                }
            }
        }
        drop(stream);
        Ok(QueryResult {
            rows_affected: columns.is_empty().then_some(affected),
            columns,
            rows,
            elapsed: started.elapsed(),
            truncated,
        })
    }
}

#[async_trait]
impl DbSession for PostgresSession {
    async fn introspect(&self, scope: IntrospectScope) -> Result<SchemaTree, DbError> {
        let mut conn = self.conn.lock().await;
        introspect::load(&mut conn, &scope).await
    }

    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError> {
        if req.language != QueryLanguage::Sql {
            return Err(DbError::Unsupported("PostgreSQL only runs SQL".into()));
        }
        match req.timeout {
            None => self.run(&req).await,
            Some(limit) => match tokio::time::timeout(limit, self.run(&req)).await {
                Ok(result) => result,
                Err(_) => {
                    // Stop the statement on the server; the dropped future cannot.
                    let _ = self.cancel_backend().await;
                    self.resync().await;
                    Err(DbError::Timeout)
                }
            },
        }
    }

    async fn cancel(&self) -> Result<(), DbError> {
        self.cancel_backend().await
    }

    async fn close(self: Box<Self>) {
        use sqlx::Connection;
        let session = *self;
        let _ = session.conn.into_inner().close().await;
        let _ = session.control.into_inner().close().await;
    }
}
