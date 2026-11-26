use application::ports::DbError;

/// SQLSTATE codes we map to dedicated errors.
const INVALID_PASSWORD: &str = "28P01";
const INVALID_AUTH: &str = "28000";
const QUERY_CANCELED: &str = "57014";

/// Maps an `sqlx` error to a port error. Messages come from the server or OS and never contain
/// the password.
pub(crate) fn map_error(e: sqlx::Error) -> DbError {
    match &e {
        sqlx::Error::Database(db) => match db.code().as_deref() {
            Some(INVALID_PASSWORD | INVALID_AUTH) => DbError::Authentication,
            Some(QUERY_CANCELED) => DbError::Cancelled,
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
