//! Exporting a query's result to CSV or JSON.
//!
//! The query is run again with a higher row limit than the grid uses, so an export can hold more
//! than what is on screen. It goes through the same read-only guard as any execution.

use std::io::Write;
use std::time::Duration;

use domain::{ConnectionId, Dialect, QueryRequest};

use crate::error::UseCaseError;
use crate::export::{ExportFormat, write};
use crate::ports::DbSession;
use crate::usecases::ExecuteQuery;

/// Default cap on exported rows. Results are held in memory, so this is a deliberate limit.
pub const DEFAULT_MAX_EXPORT_ROWS: u32 = 100_000;
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub rows: usize,
    /// More rows existed than the cap allowed.
    pub truncated: bool,
}

pub struct ExportResult<'a> {
    execute: &'a ExecuteQuery,
}

impl<'a> ExportResult<'a> {
    pub fn new(execute: &'a ExecuteQuery) -> Self {
        Self { execute }
    }

    #[allow(clippy::too_many_arguments)] // one call site; each argument is a distinct input
    pub async fn run(
        &self,
        connection: ConnectionId,
        session: &dyn DbSession,
        sql: &str,
        dialect: &Dialect,
        read_only: bool,
        format: ExportFormat,
        max_rows: u32,
        out: &mut (dyn Write + Send),
    ) -> Result<ExportSummary, UseCaseError> {
        let mut req = QueryRequest::sql(sql);
        req.row_limit = Some(max_rows.max(1));
        req.timeout = Some(EXPORT_TIMEOUT);
        let result = self
            .execute
            .execute_guarded(connection, session, req, dialect, read_only)
            .await?;
        if result.columns.is_empty() {
            return Err(UseCaseError::Blocked(
                "This statement returns no rows to export.".into(),
            ));
        }
        write(&result, format, out).map_err(|e| UseCaseError::Io(e.to_string()))?;
        Ok(ExportSummary {
            rows: result.rows.len(),
            truncated: result.truncated,
        })
    }
}
