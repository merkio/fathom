#![allow(clippy::expect_used)] // test helpers

use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use application::error::UseCaseError;
use application::export::ExportFormat;
use application::ports::DatabaseDriver;
use application::usecases::{ExecuteOptions, ExecuteQuery, ExportResult};
use domain::{
    ConnectionId, ConnectionProfile, DataType, Dialect, DriverKind, QueryResult, ResultColumn,
    Value,
};
use futures::executor::block_on;
use test_support::{FakeDriver, FixedClock, InMemoryHistory};

fn rows(n: i64) -> QueryResult {
    QueryResult {
        columns: vec![
            ResultColumn {
                name: "id".into(),
                data_type: DataType::new("int"),
            },
            ResultColumn {
                name: "name".into(),
                data_type: DataType::new("text"),
            },
        ],
        rows: (1..=n)
            .map(|i| vec![Value::Int(i), Value::Text(format!("n{i}"))])
            .collect(),
        elapsed: Duration::ZERO,
        ..QueryResult::empty()
    }
}

struct Env {
    driver: FakeDriver,
    session: Box<dyn application::ports::DbSession>,
    execute: ExecuteQuery,
}

fn env(result: QueryResult) -> Env {
    let driver = FakeDriver::new("fake").with_result(result);
    let profile = ConnectionProfile::new("p", DriverKind::new("fake"), "h");
    let session = block_on(driver.connect(&profile, None)).expect("connect");
    let execute = ExecuteQuery::new(
        Arc::new(InMemoryHistory::default()),
        Arc::new(FixedClock(UNIX_EPOCH)),
        ExecuteOptions::default(),
    );
    Env {
        driver,
        session,
        execute,
    }
}

fn export(
    e: &Env,
    sql: &str,
    read_only: bool,
    format: ExportFormat,
    max_rows: u32,
    out: &mut (dyn Write + Send),
) -> Result<application::usecases::ExportSummary, UseCaseError> {
    let dialect = Dialect {
        name: "postgres",
        ..Dialect::ANSI
    };
    block_on(ExportResult::new(&e.execute).run(
        ConnectionId::new(),
        e.session.as_ref(),
        sql,
        &dialect,
        read_only,
        format,
        max_rows,
        out,
    ))
}

#[test]
fn exports_the_full_result_as_csv_and_json() {
    let e = env(rows(3));
    let mut csv = Vec::new();
    let summary = export(
        &e,
        "select * from t",
        false,
        ExportFormat::Csv,
        100,
        &mut csv,
    )
    .expect("export");
    assert_eq!((summary.rows, summary.truncated), (3, false));
    assert_eq!(
        String::from_utf8(csv).expect("utf8"),
        "id,name\r\n1,n1\r\n2,n2\r\n3,n3\r\n"
    );

    let mut json = Vec::new();
    export(
        &e,
        "select * from t",
        false,
        ExportFormat::Json,
        100,
        &mut json,
    )
    .expect("export");
    assert!(
        String::from_utf8(json)
            .expect("utf8")
            .contains("{\"id\": 2, \"name\": \"n2\"}")
    );
}

#[test]
fn the_row_cap_is_enforced_and_reported_as_truncation() {
    let e = env(rows(10));
    let mut out = Vec::new();
    let summary =
        export(&e, "select * from t", false, ExportFormat::Csv, 4, &mut out).expect("export");
    assert_eq!((summary.rows, summary.truncated), (4, true));
    assert_eq!(
        String::from_utf8(out).expect("utf8").lines().count(),
        5,
        "header plus four rows"
    );
}

#[test]
fn a_cap_of_zero_still_exports_at_least_one_row() {
    let e = env(rows(5));
    let mut out = Vec::new();
    assert_eq!(
        export(&e, "select 1", false, ExportFormat::Csv, 0, &mut out)
            .expect("export")
            .rows,
        1
    );
}

#[test]
fn exports_respect_read_only_mode() {
    let e = env(rows(2));
    let mut out = Vec::new();
    let err = export(
        &e,
        "DELETE FROM t RETURNING *",
        true,
        ExportFormat::Csv,
        10,
        &mut out,
    )
    .err();
    assert!(matches!(err, Some(UseCaseError::Blocked(_))), "{err:?}");
    assert!(
        e.driver.executed().is_empty(),
        "nothing reached the database"
    );
    assert!(out.is_empty(), "nothing was written");
    // A plain read still exports in read-only mode.
    export(&e, "select * from t", true, ExportFormat::Csv, 10, &mut out).expect("read is fine");
}

#[test]
fn statements_without_a_result_set_have_nothing_to_export() {
    let e = env(QueryResult {
        rows_affected: Some(3),
        ..QueryResult::empty()
    });
    let mut out = Vec::new();
    let err = export(
        &e,
        "update t set a = 1",
        false,
        ExportFormat::Csv,
        10,
        &mut out,
    )
    .err();
    assert!(matches!(&err, Some(UseCaseError::Blocked(m)) if m.contains("no rows")));
    assert!(out.is_empty());
}

struct DiskFull;

impl Write for DiskFull {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("no space left on device"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_failures_become_io_errors_with_the_reason() {
    let e = env(rows(2));
    let err = export(&e, "select 1", false, ExportFormat::Csv, 10, &mut DiskFull).err();
    assert!(
        matches!(&err, Some(UseCaseError::Io(m)) if m.contains("no space left")),
        "{err:?}"
    );
}
