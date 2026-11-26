use application::ports::DbError;
use sqlx::mysql::MySqlDatabaseError;

/// MySQL / MariaDB error numbers we map to dedicated errors.
const ACCESS_DENIED: u16 = 1045;
const QUERY_INTERRUPTED: u16 = 1317;
const MAX_EXECUTION_TIME_EXCEEDED: u16 = 3024; // MySQL `max_execution_time`
const MAX_STATEMENT_TIME_EXCEEDED: u16 = 1969; // MariaDB `max_statement_time`

/// Maps an `sqlx` error to a port error. Messages come from the server or OS and never contain
/// the password.
pub(crate) fn map_error(e: sqlx::Error) -> DbError {
    match &e {
        sqlx::Error::Database(db) => match db
            .try_downcast_ref::<MySqlDatabaseError>()
            .map(MySqlDatabaseError::number)
        {
            Some(ACCESS_DENIED) => DbError::Authentication,
            Some(QUERY_INTERRUPTED) => DbError::Cancelled,
            Some(MAX_EXECUTION_TIME_EXCEEDED | MAX_STATEMENT_TIME_EXCEEDED) => DbError::Timeout,
            _ => DbError::Query(db.message().to_owned()),
        },
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::Configuration(_) => DbError::Connection(e.to_string()),
        _ => DbError::Query(e.to_string()),
    }
}

/// Connection-phase variant: failures here are connection failures unless the server said why.
pub(crate) fn map_connect_error(e: sqlx::Error) -> DbError {
    match map_error(e) {
        DbError::Query(m) => DbError::Connection(m),
        other => other,
    }
}
