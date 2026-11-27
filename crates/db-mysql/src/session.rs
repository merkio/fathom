use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use application::ports::{DbError, DbSession, IntrospectScope};
use application::sql::changes::{self, BuildOptions, ParamStyle};
use async_trait::async_trait;
use domain::{
    ApplyOutcome, DataType, QueryLanguage, QueryRequest, QueryResult, ResultColumn, SchemaTree,
    TableChanges, Value,
};
use futures::StreamExt;
use sqlx::mysql::MySqlConnection;
use sqlx::{AssertSqlSafe, Column, Either, Executor, Row};
use tokio::sync::Mutex;

use crate::errors::map_error;
use crate::introspect;
use crate::values::{map_row, type_name};

const RESYNC_TIMEOUT: Duration = Duration::from_secs(5);

/// One query connection plus one control connection used only to cancel running statements.
///
/// `execute` and `introspect` share the query connection, so they run one at a time.
pub(crate) struct MySqlSession {
    conn: Mutex<MySqlConnection>,
    control: Mutex<MySqlConnection>,
    connection_id: u64,
    /// A statement is running on `conn`.
    running: AtomicBool,
    /// The user asked to cancel the running statement.
    cancel_requested: AtomicBool,
}

impl MySqlSession {
    pub(crate) async fn new(
        mut conn: MySqlConnection,
        control: MySqlConnection,
    ) -> Result<Self, DbError> {
        let id: u64 = sqlx::query("SELECT CONNECTION_ID()")
            .fetch_one(&mut conn)
            .await
            .map_err(map_error)?
            .try_get(0)
            .map_err(map_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
            control: Mutex::new(control),
            connection_id: id,
            running: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
        })
    }

    /// `KILL QUERY` stops the statement but keeps the connection.
    async fn kill_query(&self) -> Result<(), DbError> {
        let mut control = self.control.lock().await;
        // The id is an integer from the server, so formatting it in is safe.
        sqlx::query(AssertSqlSafe(format!("KILL QUERY {}", self.connection_id)))
            .execute(&mut *control)
            .await
            .map_err(map_error)?;
        Ok(())
    }

    /// After a statement was abandoned, `sqlx` may report that statement's error on the
    /// connection's next use. Absorb it so callers never see a stale error.
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
impl DbSession for MySqlSession {
    async fn introspect(&self, scope: IntrospectScope) -> Result<SchemaTree, DbError> {
        let mut conn = self.conn.lock().await;
        introspect::load(&mut conn, &scope).await
    }

    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError> {
        if req.language != QueryLanguage::Sql {
            return Err(DbError::Unsupported("MySQL only runs SQL".into()));
        }
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);

        let outcome = match req.timeout {
            None => self.run(&req).await,
            Some(limit) => match tokio::time::timeout(limit, self.run(&req)).await {
                Ok(result) => result,
                Err(_) => {
                    // Stop the statement on the server; the dropped future cannot.
                    let _ = self.kill_query().await;
                    self.resync().await;
                    self.running.store(false, Ordering::SeqCst);
                    return Err(DbError::Timeout);
                }
            },
        };
        self.running.store(false, Ordering::SeqCst);

        // A killed `SELECT SLEEP()` finishes normally (it returns 1) instead of failing, so a
        // requested cancel is honoured here whatever the server reported.
        if self.cancel_requested.swap(false, Ordering::SeqCst) {
            return Err(DbError::Cancelled);
        }
        outcome
    }

    async fn apply_changes(&self, edits: TableChanges) -> Result<ApplyOutcome, DbError> {
        // MySQL converts a bound value to the column's type itself, so values bind natively.
        let plans = changes::build(
            BuildOptions {
                dialect: &crate::dialect::MYSQL_DIALECT,
                style: ParamStyle::Question,
                casts: false,
                mutations: false,
            },
            &edits,
        )?;
        let mut conn = self.conn.lock().await;
        let mut tx = sqlx::Connection::begin(&mut *conn)
            .await
            .map_err(map_error)?;
        let mut affected = 0u64;
        for plan in &plans {
            let mut query = sqlx::query(AssertSqlSafe(plan.sql.clone()));
            for value in &plan.params {
                query = match value {
                    Value::Int(i) => query.bind(*i),
                    Value::Float(f) => query.bind(*f),
                    Value::Bool(b) => query.bind(*b),
                    Value::Text(t) | Value::Decimal(t) | Value::Temporal(t) | Value::Json(t) => {
                        query.bind(t.clone())
                    }
                    Value::Bytes(_) | Value::Null => {
                        return Err(DbError::Query("internal: unsupported parameter".into()));
                    }
                };
            }
            let done = query.execute(&mut *tx).await.map_err(map_error)?;
            // The connection asks for matched (not changed) rows, so setting a column to its
            // current value still counts as one. Anything else means the row is gone or the
            // key was not unique; dropping the transaction rolls everything back.
            if done.rows_affected() != 1 {
                return Err(DbError::Query(format!(
                    "a change touched {} rows instead of 1 (the data changed?); nothing was applied",
                    done.rows_affected()
                )));
            }
            affected += done.rows_affected();
        }
        tx.commit().await.map_err(map_error)?;
        Ok(ApplyOutcome {
            applied: plans.len(),
            rows_affected: Some(affected),
            atomic: true,
        })
    }

    async fn cancel(&self) -> Result<(), DbError> {
        if self.running.load(Ordering::SeqCst) {
            self.cancel_requested.store(true, Ordering::SeqCst);
            self.kill_query().await?;
        }
        Ok(())
    }

    async fn close(self: Box<Self>) {
        use sqlx::Connection;
        let session = *self;
        let _ = session.conn.into_inner().close().await;
        let _ = session.control.into_inner().close().await;
    }
}
