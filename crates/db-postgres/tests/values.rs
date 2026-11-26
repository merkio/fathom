//! PostgreSQL-specific value mapping, against a real server (see `contract.rs` for setup).
#![cfg(feature = "integration")]

mod common;

use application::ports::DatabaseDriver;
use db_postgres::PostgresDriver;
use domain::{QueryRequest, Value};

#[tokio::test(flavor = "multi_thread")]
async fn maps_postgres_types_to_domain_values() {
    let s = common::server().await;
    let (profile, password) = common::profile_for(&s);
    let session = PostgresDriver::new()
        .connect(&profile, Some(password))
        .await
        .expect("connect");

    let sql = "SELECT true AS b, 7::int2 AS i2, 8::int8 AS i8, 1.5::float8 AS f, \
        12345678901234567890.123456789::numeric AS n, 'héllo'::varchar AS v, \
        '\\xdead'::bytea AS raw, DATE '2024-02-29' AS d, TIMESTAMP '2024-02-29 13:14:15' AS ts, \
        TIMESTAMPTZ '2024-02-29 13:14:15+00' AS tstz, \
        '123e4567-e89b-12d3-a456-426614174000'::uuid AS u, '{\"a\":1}'::jsonb AS j, \
        INTERVAL '1 day' AS unmapped";
    let r = session
        .execute(QueryRequest::sql(sql))
        .await
        .expect("query");
    let row = &r.rows[0];
    assert_eq!(
        row[..11],
        [
            Value::Bool(true),
            Value::Int(7),
            Value::Int(8),
            Value::Float(1.5),
            Value::Decimal("12345678901234567890.123456789".into()),
            Value::Text("héllo".into()),
            Value::Bytes(vec![0xde, 0xad]),
            Value::Temporal("2024-02-29".into()),
            Value::Temporal("2024-02-29 13:14:15".into()),
            Value::Temporal("2024-02-29T13:14:15+00:00".into()),
            Value::Text("123e4567-e89b-12d3-a456-426614174000".into()),
        ]
    );
    assert_eq!(row[11], Value::Json("{\"a\":1}".into()));
    assert_eq!(row[12], Value::Text("<interval>".into()));
    assert_eq!(r.columns[1].data_type.0, "int2");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_non_sql_languages() {
    let s = common::server().await;
    let (profile, password) = common::profile_for(&s);
    let session = PostgresDriver::new()
        .connect(&profile, Some(password))
        .await
        .expect("connect");
    let mut req = QueryRequest::sql("db.users.find()");
    req.language = domain::QueryLanguage::Mql;
    assert!(matches!(
        session.execute(req).await.err(),
        Some(application::ports::DbError::Unsupported(_))
    ));
    session.close().await;
}
