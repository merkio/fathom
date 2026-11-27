//! Exporting a query's result to CSV or JSON.
//!
//! The query is run again with a higher row limit than the grid uses, so an export can hold more
//! than what is on screen. It goes through the same read-only guard as any execution.

use std::io::Write;
use std::time::Duration;

use domain::{ConnectionId, Dialect, QueryRequest};

use crate::error::UseCaseError;
use crate::export::{ExportFormat, SqlOptions, render, write};
use crate::ports::{DbSession, ExportDestination, ExportError};
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

    /// Runs the statement with the export row cap (same guard as any execution) and returns the
    /// rows. Refuses statements that return no rows.
    #[allow(clippy::too_many_arguments)] // one call site; each argument is a distinct input
    pub async fn fetch(
        &self,
        connection: ConnectionId,
        session: &dyn DbSession,
        sql: &str,
        dialect: &Dialect,
        read_only: bool,
        max_rows: u32,
    ) -> Result<domain::QueryResult, UseCaseError> {
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
        Ok(result)
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
        let result = self
            .fetch(connection, session, sql, dialect, read_only, max_rows)
            .await?;
        write(&result, format, out).map_err(|e| UseCaseError::Io(e.to_string()))?;
        Ok(ExportSummary {
            rows: result.rows.len(),
            truncated: result.truncated,
        })
    }
}

/// Where an export ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub location: String,
    pub bytes: usize,
}

/// Renders `result` and writes it through `destination` at the path built from `folder` and
/// `name`. The destination never overwrites an existing file.
pub async fn deliver(
    destination: &dyn ExportDestination,
    folder: &str,
    name: &str,
    format: ExportFormat,
    result: &domain::QueryResult,
    sql: &SqlOptions<'_>,
) -> Result<Delivered, UseCaseError> {
    let key = export_key(folder, name, format)?;
    let data = render(result, format, sql).map_err(|e| UseCaseError::Io(e.to_string()))?;
    let bytes = data.len();
    let location = destination.put(&key, data).await?;
    Ok(Delivered { location, bytes })
}

/// Builds the `/`-separated path of an export inside a destination: `folder/name.ext`.
///
/// An empty name means `fathom-export`; a missing extension is added. Folders and names that try
/// to leave the destination (`..`, absolute paths, NUL) are refused here, and again by every
/// destination.
pub fn export_key(folder: &str, name: &str, format: ExportFormat) -> Result<String, ExportError> {
    let invalid = |m: &str| Err(ExportError::Invalid(m.to_owned()));
    let normalise = |s: &str| s.trim().replace('\\', "/");
    let (folder, name) = (normalise(folder), normalise(name));
    if folder.contains('\0') || name.contains('\0') {
        return invalid("The path contains an invalid character.");
    }
    if name.contains('/') {
        return invalid("Enter a file name only; use the folder field for folders.");
    }
    if name == ".." || name == "." {
        return invalid("Enter a file name.");
    }
    if folder.starts_with('/') {
        return invalid("Use a folder path relative to the destination (no leading `/`).");
    }
    let mut segments = Vec::new();
    for segment in folder.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if segment == ".." {
            return invalid("The folder path cannot contain `..`.");
        }
        segments.push(segment.to_owned());
    }
    let name = if name.is_empty() {
        "fathom-export".to_owned()
    } else {
        name
    };
    let file = if name
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
    {
        name
    } else {
        format!("{name}.{}", format.extension())
    };
    segments.push(file);
    Ok(segments.join("/"))
}

#[cfg(test)]
mod key_tests {
    use super::*;

    fn key(folder: &str, name: &str) -> Result<String, ExportError> {
        export_key(folder, name, ExportFormat::Csv)
    }

    #[test]
    fn folder_and_name_are_joined_with_an_extension() {
        assert_eq!(
            key("exports/2026-11-27/", "customers").unwrap(),
            "exports/2026-11-27/customers.csv"
        );
        assert_eq!(key("", "customers.csv").unwrap(), "customers.csv");
        assert_eq!(key("a//b/./c", "x").unwrap(), "a/b/c/x.csv");
    }

    #[test]
    fn empty_name_gets_a_default_and_dotfiles_still_get_an_extension() {
        assert_eq!(key("", "  ").unwrap(), "fathom-export.csv");
        assert_eq!(key("", ".hidden").unwrap(), ".hidden.csv");
        assert_eq!(export_key("", "d", ExportFormat::Sql).unwrap(), "d.sql");
    }

    #[test]
    fn an_existing_extension_is_kept() {
        assert_eq!(key("", "data.txt").unwrap(), "data.txt");
    }

    #[test]
    fn windows_separators_are_normalised() {
        assert_eq!(key("a\\b", "x").unwrap(), "a/b/x.csv");
    }

    #[test]
    fn paths_that_leave_the_destination_are_refused() {
        for (folder, name) in [
            ("../x", "f"),
            ("a/../../x", "f"),
            ("/etc", "f"),
            ("", "../f"),
            ("", "a/b"),
            ("", ".."),
            ("a\0b", "f"),
            ("", "f\0"),
        ] {
            assert!(key(folder, name).is_err(), "{folder:?} {name:?}");
        }
    }
}

#[cfg(test)]
mod deliver_tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use domain::{DataType, QueryResult, ResultColumn, Value};

    use super::*;

    #[derive(Default)]
    struct Memory(Mutex<BTreeMap<String, Vec<u8>>>);

    #[async_trait]
    impl ExportDestination for Memory {
        fn kind(&self) -> &'static str {
            "memory"
        }
        fn describe(&self) -> String {
            "memory://".into()
        }
        async fn test(&self) -> Result<(), ExportError> {
            Ok(())
        }
        async fn put(&self, path: &str, data: Vec<u8>) -> Result<String, ExportError> {
            let mut files = self.0.lock().unwrap();
            if files.contains_key(path) {
                return Err(ExportError::AlreadyExists(path.into()));
            }
            files.insert(path.into(), data);
            Ok(format!("memory://{path}"))
        }
    }

    fn result() -> QueryResult {
        QueryResult {
            columns: vec![ResultColumn {
                name: "id".into(),
                data_type: DataType::new("int"),
            }],
            rows: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            ..QueryResult::empty()
        }
    }

    fn deliver_it(d: &Memory, folder: &str, name: &str) -> Result<Delivered, UseCaseError> {
        futures::executor::block_on(deliver(
            d,
            folder,
            name,
            ExportFormat::Csv,
            &result(),
            &SqlOptions::default(),
        ))
    }

    #[test]
    fn the_rendered_file_lands_at_the_built_path() {
        let d = Memory::default();
        let done = deliver_it(&d, "exports/today", "customers").unwrap();
        assert_eq!(done.location, "memory://exports/today/customers.csv");
        let files = d.0.lock().unwrap();
        let body = String::from_utf8(files["exports/today/customers.csv"].clone()).unwrap();
        assert_eq!(body, "id\r\n1\r\n2\r\n");
        assert_eq!(done.bytes, body.len());
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let d = Memory::default();
        deliver_it(&d, "", "a").unwrap();
        let again = deliver_it(&d, "", "a");
        assert!(matches!(
            again,
            Err(UseCaseError::Export(ExportError::AlreadyExists(_)))
        ));
    }

    #[test]
    fn bad_paths_never_reach_the_destination() {
        let d = Memory::default();
        assert!(deliver_it(&d, "../x", "a").is_err());
        assert!(d.0.lock().unwrap().is_empty());
    }
}
