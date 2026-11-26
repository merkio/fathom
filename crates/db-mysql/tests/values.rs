//! MySQL/MariaDB value mapping, against a real server (see `contract.rs` for setup).
#![cfg(feature = "integration")]
#![allow(clippy::expect_used)] // test helpers

mod common;

use application::ports::{DatabaseDriver, DbError, DbSession};
use db_mysql::MySqlDriver;
use domain::{QueryLanguage, QueryRequest, Value};

async fn connect() -> Box<dyn DbSession> {
    let s = common::server().await;
    let (profile, password) = common::profile_for(&s);
    MySqlDriver::new().connect(&profile, Some(password)).await.expect("connect")
}

async fn exec(s: &dyn DbSession, sql: &str) -> domain::QueryResult {
    s.execute(QueryRequest::sql(sql)).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_mysql_types_to_domain_values() {
    let s = connect().await;
    exec(s.as_ref(), "DROP TABLE IF EXISTS value_types").await;
    exec(
        s.as_ref(),
        "CREATE TABLE value_types (
            ti tinyint, si smallint, i int, bi bigint, ubi bigint unsigned,
            f float, d double, price decimal(10,2), big decimal(30,10),
            s varchar(20), t text, raw varbinary(8), bl blob,
            dt date, tm time, dts datetime, y year, flag boolean)",
    )
    .await;
    exec(
        s.as_ref(),
        "INSERT INTO value_types VALUES (
            -5, 300, -70000, -9000000000, 18446744073709551615,
            1.5, 2.25, 1.50, 12345678901234567890.1234567890,
            'héllo', 'some text', x'dead', x'beef',
            '2024-02-29', '-838:59:59', '2024-02-29 13:14:15', 2024, true)",
    )
    .await;
    exec(s.as_ref(), "INSERT INTO value_types (ti) VALUES (NULL)").await;

    let r = exec(s.as_ref(), "SELECT * FROM value_types ORDER BY ti IS NULL, ti LIMIT 1").await;
    let row = &r.rows[0];
    let by_name = |n: &str| {
        let i = r.columns.iter().position(|c| c.name == n).expect("column");
        row[i].clone()
    };
    assert_eq!(by_name("ti"), Value::Int(-5));
    assert_eq!(by_name("si"), Value::Int(300));
    assert_eq!(by_name("i"), Value::Int(-70000));
    assert_eq!(by_name("bi"), Value::Int(-9_000_000_000));
    assert_eq!(by_name("ubi"), Value::Decimal("18446744073709551615".into()), "beyond i64 keeps every digit");
    assert_eq!(by_name("f"), Value::Float(1.5));
    assert_eq!(by_name("d"), Value::Float(2.25));
    assert_eq!(by_name("price"), Value::Decimal("1.50".into()), "declared scale is kept");
    assert_eq!(by_name("big"), Value::Decimal("12345678901234567890.1234567890".into()));
    assert_eq!(by_name("s"), Value::Text("héllo".into()));
    assert_eq!(by_name("t"), Value::Text("some text".into()));
    assert_eq!(by_name("raw"), Value::Bytes(vec![0xde, 0xad]));
    assert_eq!(by_name("bl"), Value::Bytes(vec![0xbe, 0xef]));
    assert_eq!(by_name("dt"), Value::Temporal("2024-02-29".into()));
    assert_eq!(by_name("dts"), Value::Temporal("2024-02-29 13:14:15".into()));
    assert_eq!(by_name("y"), Value::Int(2024));
    assert_eq!(by_name("tm"), Value::Temporal("-838:59:59".into()), "TIME beyond 24h and negative");

    // NULLs in every column of the second row.
    let nulls = exec(s.as_ref(), "SELECT * FROM value_types WHERE ti IS NULL").await;
    assert!(nulls.rows[0].iter().all(Value::is_null));

    exec(s.as_ref(), "DROP TABLE value_types").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn json_and_timestamps_are_mapped() {
    let s = connect().await;
    let r = exec(
        s.as_ref(),
        "SELECT CAST('{\"a\": 1}' AS JSON) AS j, CAST('2024-02-29 13:14:15' AS DATETIME) AS d",
    )
    .await;
    // MySQL has a JSON type; MariaDB aliases it to LONGTEXT. Both must read back the same text.
    match &r.rows[0][0] {
        Value::Json(text) | Value::Text(text) => assert!(text.contains("\"a\""), "{text}"),
        other => panic!("unexpected JSON mapping: {other:?}"),
    }
    assert_eq!(r.rows[0][1], Value::Temporal("2024-02-29 13:14:15".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_non_sql_languages() {
    let s = connect().await;
    let mut req = QueryRequest::sql("db.users.find()");
    req.language = QueryLanguage::Mql;
    assert!(matches!(s.execute(req).await.err(), Some(DbError::Unsupported(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_while_idle_does_not_disturb_the_next_statement() {
    let s = connect().await;
    s.cancel().await.expect("cancel while idle");
    assert_eq!(exec(s.as_ref(), "SELECT 1 + 1").await.rows, vec![vec![Value::Int(2)]]);
}
