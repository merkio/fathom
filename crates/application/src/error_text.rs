//! Turns errors into messages a person can act on: what happened, and what to try.

use crate::error::UseCaseError;
use crate::ports::{DbError, LlmError, SecretError};
use crate::redact::redact;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserError {
    pub message: String,
    /// A concrete next step, when one is known.
    pub hint: Option<String>,
}

impl UserError {
    fn new(message: impl Into<String>, hint: Option<&str>) -> Self {
        Self {
            message: redact(&message.into()),
            hint: hint.map(str::to_owned),
        }
    }

    /// One text for the status bar or result pane: the message, then the hint on its own line.
    pub fn to_text(&self) -> String {
        match &self.hint {
            Some(hint) => format!("{}\nHint: {hint}", self.message),
            None => self.message.clone(),
        }
    }
}

fn has(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_lowercase();
    needles.iter().any(|n| lower.contains(n))
}

fn connection_hint(message: &str) -> &'static str {
    if has(message, &["refused"]) {
        "Is the database server running, and is the host and port right? A local firewall can also block it."
    } else if has(message, &["timed out", "timeout"]) {
        "The server did not answer in time. Check the host, the port and any firewall or VPN."
    } else if has(
        message,
        &[
            "lookup",
            "resolve",
            "name or service",
            "no such host",
            "dns",
        ],
    ) {
        "The host name could not be found. Check it for typos, or try the IP address."
    } else if has(message, &["tls", "ssl", "certificate"]) {
        "A secure-connection (TLS) problem. Check the SSL mode and certificate settings of the connection."
    } else {
        "Check the connection settings (host, port, database) and that the server is reachable."
    }
}

fn query_hint(message: &str) -> Option<&'static str> {
    if has(
        message,
        &[
            "does not exist",
            "doesn't exist",
            "unknown column",
            "unknown table",
            "no such table",
            "no such column",
        ],
    ) {
        Some(
            "Check the spelling against the schema tree on the left; names may need quotes if they have capitals.",
        )
    } else if has(message, &["syntax error", "you have an error in your sql"]) {
        Some(
            "Check the SQL near the position the server reports. \"Fix with AI\" can suggest a correction.",
        )
    } else if has(
        message,
        &["permission denied", "access denied", "must be owner"],
    ) {
        Some(
            "This database user is not allowed to do that. Ask an administrator or use another connection.",
        )
    } else if has(message, &["read-only", "read only"]) {
        Some(
            "The server or this connection is read-only. Turn read-only mode off in the connection settings if that is not intended.",
        )
    } else if has(
        message,
        &["deadlock", "lock timeout", "could not obtain lock"],
    ) {
        Some("Another session holds a lock. Try again in a moment.")
    } else {
        None
    }
}

pub fn describe(error: &UseCaseError) -> UserError {
    match error {
        UseCaseError::Db(DbError::Connection(m)) => {
            UserError::new(format!("Could not connect: {m}"), Some(connection_hint(m)))
        }
        UseCaseError::Db(DbError::Authentication) => UserError::new(
            "The server rejected the user name or password.",
            Some("Edit the connection and enter the password again."),
        ),
        UseCaseError::Db(DbError::Query(m)) => UserError::new(m.clone(), query_hint(m)),
        UseCaseError::Db(DbError::Timeout) => UserError::new(
            "The query took too long and was stopped.",
            Some("Narrow it with a WHERE clause or LIMIT, or check for a missing index."),
        ),
        UseCaseError::Db(DbError::Cancelled) => UserError::new("Query cancelled.", None),
        UseCaseError::Db(DbError::Unsupported(m)) => {
            UserError::new(format!("Not supported: {m}"), None)
        }
        UseCaseError::Blocked(m) => UserError::new(
            m.clone(),
            m.contains("Read-only")
                .then_some("Turn off read-only mode in the connection settings to run this."),
        ),
        UseCaseError::UnknownDriver(kind) => UserError::new(
            format!("This build has no driver for `{}`.", kind.as_str()),
            Some("Choose another database type in the connection settings."),
        ),
        UseCaseError::ConnectionNotFound(_) => UserError::new(
            "That connection no longer exists.",
            Some("It may have been deleted. The connection list has been refreshed."),
        ),
        UseCaseError::Secret(SecretError::Unavailable(m)) => UserError::new(
            format!("The secret store is unavailable: {m}"),
            Some(
                "Passwords need the OS keychain. Set DBIEVER_SECRET_STORE=memory to keep them in memory only (they are not saved).",
            ),
        ),
        UseCaseError::Llm(LlmError::Authentication) => UserError::new(
            "The AI provider rejected the credentials, or no API key is set.",
            Some("Enter a valid API key in the AI settings."),
        ),
        UseCaseError::Llm(LlmError::RateLimited) => UserError::new(
            "The AI provider is rate limiting requests.",
            Some("Wait a moment and try again, or use another provider or model."),
        ),
        UseCaseError::Llm(LlmError::Network(m)) => UserError::new(
            format!("Could not reach the AI provider: {m}"),
            Some(
                "Check your network. For Ollama, check that it is running and the address is right.",
            ),
        ),
        UseCaseError::Llm(LlmError::Malformed(m)) => UserError::new(
            format!("The AI provider sent a reply that could not be read: {m}"),
            Some("Try again; if it keeps happening, try another model."),
        ),
        UseCaseError::Llm(other) => UserError::new(other.to_string(), None),
        UseCaseError::UnknownProvider(id) => UserError::new(
            format!("The AI provider `{}` is not available.", id.as_str()),
            Some("Choose another provider in the AI settings."),
        ),
        UseCaseError::Io(m) => UserError::new(
            format!("Could not write the file: {m}"),
            Some("Check the folder exists and you may write to it."),
        ),
        other => UserError::new(other.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ConnectionId, DriverKind, ProviderId};

    fn text(e: UseCaseError) -> String {
        describe(&e).to_text()
    }

    #[test]
    fn connection_problems_get_a_specific_hint() {
        let t = |m: &str| text(UseCaseError::Db(DbError::Connection(m.into())));
        assert!(t("Connection refused (os error 111)").contains("server running"));
        assert!(t("connection timed out").contains("did not answer in time"));
        assert!(
            t("failed to lookup address information: Name or service not known")
                .contains("host name")
        );
        assert!(t("invalid certificate").contains("TLS"));
        assert!(t("something odd").contains("Check the connection settings"));
        assert!(t("refused").starts_with("Could not connect: refused\nHint:"));
    }

    #[test]
    fn authentication_failures_say_what_to_do() {
        let t = text(UseCaseError::Db(DbError::Authentication));
        assert!(
            t.contains("user name or password") && t.contains("Edit the connection"),
            "{t}"
        );
    }

    #[test]
    fn query_errors_keep_the_servers_message_and_add_a_hint_when_recognised() {
        let t = |m: &str| text(UseCaseError::Db(DbError::Query(m.into())));
        assert!(
            t("relation \"usres\" does not exist")
                .starts_with("relation \"usres\" does not exist\nHint:")
        );
        assert!(t("syntax error at or near \"SELEKT\"").contains("Fix with AI"));
        assert!(t("permission denied for table t").contains("not allowed"));
        assert_eq!(
            t("division by zero"),
            "division by zero",
            "no hint when none applies"
        );
    }

    #[test]
    fn timeouts_cancellation_and_blocks() {
        assert!(text(UseCaseError::Db(DbError::Timeout)).contains("LIMIT"));
        assert_eq!(
            text(UseCaseError::Db(DbError::Cancelled)),
            "Query cancelled."
        );
        let blocked = text(UseCaseError::Blocked(
            "Read-only mode: only plain reads, found a `ddl` statement".into(),
        ));
        assert!(blocked.contains("Turn off read-only mode"), "{blocked}");
        assert_eq!(
            text(UseCaseError::Blocked("Inline suggestions are off".into())),
            "Inline suggestions are off"
        );
    }

    #[test]
    fn llm_errors_point_at_the_ai_settings_or_the_network() {
        assert!(text(UseCaseError::Llm(LlmError::Authentication)).contains("API key"));
        assert!(text(UseCaseError::Llm(LlmError::RateLimited)).contains("Wait"));
        assert!(
            text(UseCaseError::Llm(LlmError::Network(
                "connection refused".into()
            )))
            .contains("Ollama")
        );
        assert!(
            text(UseCaseError::UnknownProvider(ProviderId::new("x")))
                .contains("Choose another provider")
        );
    }

    #[test]
    fn other_errors_are_described_plainly() {
        assert!(text(UseCaseError::UnknownDriver(DriverKind::new("oracle"))).contains("`oracle`"));
        assert!(
            text(UseCaseError::ConnectionNotFound(ConnectionId::new()))
                .contains("no longer exists")
        );
        assert!(
            text(UseCaseError::Secret(SecretError::Unavailable(
                "no dbus".into()
            )))
            .contains("DBIEVER_SECRET_STORE")
        );
        assert!(text(UseCaseError::Io("permission denied".into())).contains("may write"));
    }

    #[test]
    fn secrets_echoed_by_a_server_never_reach_the_message() {
        let t = text(UseCaseError::Db(DbError::Connection(
            "could not connect to postgres://bob:hunter2@db/app".into(),
        )));
        assert!(!t.contains("hunter2"), "{t}");
        let t = text(UseCaseError::Db(DbError::Query(
            "bad value for password=hunter2".into(),
        )));
        assert!(!t.contains("hunter2"), "{t}");
    }
}
