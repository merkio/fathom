//! Maps client errors to `DbError`, using the server's own error codes where there are any.

use application::ports::DbError;
use clickhouse::error::Error;

/// The server error code in a `Code: 516. DB::Exception: ...` message.
pub(crate) fn server_code(message: &str) -> Option<u32> {
    let rest = message.split("Code: ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

const AUTHENTICATION_FAILED: u32 = 516;
const UNKNOWN_USER: u32 = 192;
const WRONG_PASSWORD: u32 = 193;
const REQUIRED_PASSWORD: u32 = 194;
const QUERY_WAS_CANCELLED: u32 = 394;
const TIMEOUT_EXCEEDED: u32 = 159;

pub(crate) fn map_error(e: Error) -> DbError {
    match e {
        Error::Network(source) => DbError::Connection(source.to_string()),
        Error::TimedOut => DbError::Timeout,
        Error::BadResponse(message) => map_server_message(&message),
        other => DbError::Query(other.to_string()),
    }
}

pub(crate) fn map_server_message(message: &str) -> DbError {
    match server_code(message) {
        Some(AUTHENTICATION_FAILED | UNKNOWN_USER | WRONG_PASSWORD | REQUIRED_PASSWORD) => {
            DbError::Authentication
        }
        Some(QUERY_WAS_CANCELLED) => DbError::Cancelled,
        Some(TIMEOUT_EXCEEDED) => DbError::Timeout,
        _ => DbError::Query(message.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_read_from_the_server_message() {
        assert_eq!(
            server_code("Code: 516. DB::Exception: default: Authentication failed"),
            Some(516)
        );
        assert_eq!(
            server_code("Code: 62, e.displayText() = DB::Exception"),
            Some(62)
        );
        assert_eq!(server_code("something else"), None);
        assert_eq!(server_code("Code: x"), None);
    }

    #[test]
    fn authentication_failures_are_recognised() {
        for code in [516, 192, 193, 194] {
            assert_eq!(
                map_server_message(&format!("Code: {code}. DB::Exception: nope")),
                DbError::Authentication,
                "code {code}"
            );
        }
    }

    #[test]
    fn cancelled_and_timeout_codes() {
        assert_eq!(
            map_server_message("Code: 394. DB::Exception: Query was cancelled"),
            DbError::Cancelled
        );
        assert_eq!(
            map_server_message("Code: 159. DB::Exception: Timeout exceeded"),
            DbError::Timeout
        );
    }

    #[test]
    fn other_server_errors_keep_their_text() {
        let m = "Code: 62. DB::Exception: Syntax error";
        assert_eq!(map_server_message(m), DbError::Query(m.into()));
    }

    #[test]
    fn client_error_kinds() {
        assert_eq!(map_error(Error::TimedOut), DbError::Timeout);
        assert!(matches!(
            map_error(Error::Network("refused".into())),
            DbError::Connection(m) if m.contains("refused")
        ));
        assert!(matches!(
            map_error(Error::BadResponse("Code: 516. x".into())),
            DbError::Authentication
        ));
    }
}
