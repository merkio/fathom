use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use application::ports::{DbError, DbSession, IntrospectScope};
use application::sql::changes::{self, BuildOptions, ParamStyle};
use async_trait::async_trait;
use clickhouse::Client;
use domain::{
    ApplyOutcome, DataType, QueryLanguage, QueryRequest, QueryResult, ResultColumn, SchemaTree,
    TableChanges, Value,
};
use serde_json::Value as Json;

use crate::error::map_error;
use crate::introspect;
use crate::values::map_value;

static QUERY_SEQ: AtomicU64 = AtomicU64::new(0);

/// An id the server can be asked to kill: unique per process and per statement.
fn new_query_id() -> String {
    let n = QUERY_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("fathom-{}-{nanos}-{n}", std::process::id())
}

/// One logical connection. The HTTP client is stateless, so a "session" is the client plus the id
/// of the statement that is running (if any), which `cancel` and timeouts kill server-side.
pub(crate) struct ClickHouseSession {
    client: Client,
    running: Arc<Mutex<Option<String>>>,
    cancelled: Arc<AtomicBool>,
}

impl ClickHouseSession {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            running: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_running(&self, id: Option<String>) {
        *self.running.lock().unwrap_or_else(PoisonError::into_inner) = id;
    }

    async fn kill(&self, id: &str) {
        // Best effort: the statement may already have finished.
        let _ = self
            .client
            .query("KILL QUERY WHERE query_id = ?")
            .bind(id)
            .execute()
            .await;
    }
}

/// A parsed `JSONCompact` document.
struct Document {
    columns: Vec<ResultColumn>,
    types: Vec<String>,
    rows: Vec<Vec<Json>>,
}

fn parse_document(body: &[u8]) -> Result<Document, DbError> {
    let json: Json = serde_json::from_slice(body)
        .map_err(|e| DbError::Query(format!("unreadable result from the server: {e}")))?;
    let meta = json
        .get("meta")
        .and_then(Json::as_array)
        .ok_or_else(|| DbError::Query("the server's result has no column list".into()))?;
    let mut columns = Vec::with_capacity(meta.len());
    let mut types = Vec::with_capacity(meta.len());
    for m in meta {
        let name = m.get("name").and_then(Json::as_str).unwrap_or_default();
        let ty = m.get("type").and_then(Json::as_str).unwrap_or_default();
        columns.push(ResultColumn {
            name: name.to_owned(),
            data_type: DataType::new(ty),
        });
        types.push(ty.to_owned());
    }
    let rows = json
        .get("data")
        .and_then(Json::as_array)
        .map(|rows| {
            rows.iter()
                .map(|r| r.as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    Ok(Document {
        columns,
        types,
        rows,
    })
}

/// Turns a document into a result, keeping at most `limit` rows.
fn to_result(doc: Document, limit: Option<usize>, elapsed: std::time::Duration) -> QueryResult {
    let truncated = limit.is_some_and(|l| doc.rows.len() > l);
    let keep = limit.map_or(doc.rows.len(), |l| l.min(doc.rows.len()));
    let rows: Vec<Vec<Value>> = doc
        .rows
        .iter()
        .take(keep)
        .map(|row| {
            doc.types
                .iter()
                .zip(row)
                .map(|(ty, v)| map_value(ty, v))
                .collect()
        })
        .collect();
    QueryResult {
        columns: doc.columns,
        rows,
        rows_affected: None,
        elapsed,
        truncated,
    }
}

impl ClickHouseSession {
    async fn run(
        &self,
        id: &str,
        text: &str,
        limit: Option<usize>,
    ) -> Result<QueryResult, DbError> {
        let started = Instant::now();
        let mut query = self
            .client
            .query_raw(text)
            .with_setting("query_id", id)
            // Errors that happen mid-statement become a proper error response, and the summary
            // header (rows written) is complete.
            .with_setting("wait_end_of_query", "1");
        if let Some(limit) = limit {
            // Stop reading once one row more than the limit exists (block granularity); the rest
            // is cut below, so `truncated` stays exact.
            query = query
                .with_setting("max_result_rows", (limit + 1).to_string())
                .with_setting("result_overflow_mode", "break");
        }
        let mut cursor = query.fetch_bytes("JSONCompact").map_err(map_error)?;
        let body = cursor.collect().await.map_err(map_error)?;
        if body.iter().all(u8::is_ascii_whitespace) {
            // No result set: DDL, INSERT, mutations. The server reports rows written.
            let written = cursor
                .summary()
                .and_then(clickhouse::QuerySummary::written_rows);
            return Ok(QueryResult {
                rows_affected: Some(written.unwrap_or(0)),
                elapsed: started.elapsed(),
                ..QueryResult::empty()
            });
        }
        Ok(to_result(parse_document(&body)?, limit, started.elapsed()))
    }
}

#[async_trait]
impl DbSession for ClickHouseSession {
    async fn introspect(&self, scope: IntrospectScope) -> Result<SchemaTree, DbError> {
        introspect::load(&self.client, &scope).await
    }

    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError> {
        if req.language != QueryLanguage::Sql {
            return Err(DbError::Unsupported("ClickHouse only runs SQL".into()));
        }
        let id = new_query_id();
        self.cancelled.store(false, Ordering::SeqCst);
        self.set_running(Some(id.clone()));
        let limit = req.row_limit.map(|l| l as usize);
        let work = self.run(&id, &req.text, limit);

        let outcome = match req.timeout {
            None => work.await,
            Some(after) => match tokio::time::timeout(after, work).await {
                Ok(done) => done,
                Err(_) => {
                    self.kill(&id).await;
                    self.set_running(None);
                    return Err(DbError::Timeout);
                }
            },
        };
        self.set_running(None);
        match outcome {
            // A killed statement answers with an error; report it as the cancel it was.
            Err(DbError::Query(_) | DbError::Connection(_))
                if self.cancelled.load(Ordering::SeqCst) =>
            {
                Err(DbError::Cancelled)
            }
            other => other,
        }
    }

    async fn apply_changes(&self, edits: TableChanges) -> Result<ApplyOutcome, DbError> {
        // ClickHouse has no transactions: edits are `ALTER TABLE .. UPDATE/DELETE` mutations,
        // applied one by one. Each waits (`mutations_sync`) until it has finished, so a failure
        // is reported at the statement that caused it; earlier ones stay applied.
        let plans = changes::build(
            BuildOptions {
                dialect: &crate::driver::CLICKHOUSE_DIALECT,
                style: ParamStyle::Question,
                casts: true,
                mutations: true,
            },
            &edits,
        )?;
        let mut applied = 0;
        for plan in &plans {
            let mut query = self
                .client
                .query(&plan.sql)
                .with_setting("mutations_sync", "1");
            for value in &plan.params {
                let Value::Text(text) = value else {
                    return Err(DbError::Query("internal: expected text parameters".into()));
                };
                query = query.bind(text.as_str());
            }
            query.execute().await.map_err(|e| {
                let cause = map_error(e);
                DbError::Query(format!(
                    "{cause}; {applied} of {} changes were applied before this failed (ClickHouse has no transactions)",
                    plans.len()
                ))
            })?;
            applied += 1;
        }
        Ok(ApplyOutcome {
            applied,
            rows_affected: None,
            atomic: false,
        })
    }

    async fn cancel(&self) -> Result<(), DbError> {
        let id = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(id) = id {
            self.cancelled.store(true, Ordering::SeqCst);
            self.kill(&id).await;
        }
        Ok(())
    }

    async fn close(self: Box<Self>) {
        // Nothing to release: connections are per request.
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const DOC: &[u8] = br#"{
        "meta": [{"name": "id", "type": "Int32"}, {"name": "note", "type": "Nullable(String)"}],
        "data": [[1, null], [2, "b"], [3, "c"]],
        "rows": 3
    }"#;

    #[test]
    fn document_columns_and_rows_are_mapped() {
        let r = to_result(
            parse_document(DOC).unwrap_or_else(|e| panic!("{e}")),
            None,
            Duration::ZERO,
        );
        assert_eq!(r.columns[0].name, "id");
        assert_eq!(r.columns[1].data_type.0, "Nullable(String)");
        assert_eq!(
            r.rows,
            vec![
                vec![Value::Int(1), Value::Null],
                vec![Value::Int(2), Value::Text("b".into())],
                vec![Value::Int(3), Value::Text("c".into())],
            ]
        );
        assert!(!r.truncated);
        assert_eq!(r.rows_affected, None);
    }

    #[test]
    fn rows_beyond_the_limit_are_cut_and_flagged() {
        let doc = parse_document(DOC).unwrap_or_else(|e| panic!("{e}"));
        let r = to_result(doc, Some(2), Duration::ZERO);
        assert_eq!(r.rows.len(), 2);
        assert!(r.truncated);
    }

    #[test]
    fn a_limit_equal_to_the_row_count_does_not_truncate() {
        let doc = parse_document(DOC).unwrap_or_else(|e| panic!("{e}"));
        let r = to_result(doc, Some(3), Duration::ZERO);
        assert_eq!(r.rows.len(), 3);
        assert!(!r.truncated);
    }

    #[test]
    fn garbage_and_missing_meta_are_query_errors() {
        assert!(matches!(
            parse_document(b"not json"),
            Err(DbError::Query(_))
        ));
        assert!(matches!(parse_document(b"{}"), Err(DbError::Query(_))));
    }

    #[test]
    fn query_ids_are_unique() {
        assert_ne!(new_query_id(), new_query_id());
    }
}
