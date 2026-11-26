//! Connection profiles. A profile never holds a password, only a reference to the secret store.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConnectionId(pub Uuid);

impl ConnectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies a database kind by string id (for example `"postgres"`). Deliberately not a closed
/// enum, so adding a database never touches the domain crate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DriverKind(String);

impl DriverKind {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Key under which a secret is stored in the `SecretStore`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretRef(String);

impl SecretRef {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// Conventional key for the password of a connection profile.
    pub fn for_connection(id: ConnectionId) -> Self {
        Self(format!("connection/{}", id.0))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Option key for per-connection read-only mode.
const READ_ONLY_OPTION: &str = "read_only";
/// Option key for the per-connection inline-suggestion override.
const INLINE_AI_OPTION: &str = "inline_ai";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: ConnectionId,
    pub name: String,
    pub driver_kind: DriverKind,
    pub host: String,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: String,
    /// Where the password lives. There is intentionally no password field.
    pub secret_ref: Option<SecretRef>,
    /// Driver-specific options (TLS mode, timeouts, ...).
    pub options: BTreeMap<String, String>,
}

impl ConnectionProfile {
    pub fn new(name: impl Into<String>, driver_kind: DriverKind, host: impl Into<String>) -> Self {
        Self {
            id: ConnectionId::new(),
            name: name.into(),
            driver_kind,
            host: host.into(),
            port: None,
            database: None,
            username: String::new(),
            secret_ref: None,
            options: BTreeMap::new(),
        }
    }

    /// Whether this connection only allows plain reads (the `read_only` option).
    pub fn read_only(&self) -> bool {
        self.options
            .get(READ_ONLY_OPTION)
            .is_some_and(|v| v == "true")
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        if read_only {
            self.options
                .insert(READ_ONLY_OPTION.to_owned(), "true".to_owned());
        } else {
            self.options.remove(READ_ONLY_OPTION);
        }
    }

    /// Per-connection override of LLM inline suggestions: `Some(true)` forces them on,
    /// `Some(false)` forces them off, `None` follows the global setting.
    pub fn inline_ai(&self) -> Option<bool> {
        match self.options.get(INLINE_AI_OPTION).map(String::as_str) {
            Some("on") => Some(true),
            Some("off") => Some(false),
            _ => None,
        }
    }

    pub fn set_inline_ai(&mut self, value: Option<bool>) {
        match value {
            Some(true) => self
                .options
                .insert(INLINE_AI_OPTION.to_owned(), "on".to_owned()),
            Some(false) => self
                .options
                .insert(INLINE_AI_OPTION.to_owned(), "off".to_owned()),
            None => self.options.remove(INLINE_AI_OPTION),
        };
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.name.trim().is_empty() {
            return Err(DomainError::Invalid {
                field: "name",
                reason: "must not be empty".into(),
            });
        }
        if self.host.trim().is_empty() {
            return Err(DomainError::Invalid {
                field: "host",
                reason: "must not be empty".into(),
            });
        }
        if self.driver_kind.as_str().is_empty() {
            return Err(DomainError::Invalid {
                field: "driver_kind",
                reason: "must not be empty".into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ConnectionProfile {
        ConnectionProfile::new("local", DriverKind::new("postgres"), "localhost")
    }

    #[test]
    fn valid_profile_passes() {
        assert_eq!(profile().validate(), Ok(()));
    }

    #[test]
    fn empty_name_or_host_is_rejected() {
        let mut p = profile();
        p.name = "  ".into();
        assert!(matches!(
            p.validate(),
            Err(DomainError::Invalid { field: "name", .. })
        ));
        let mut p = profile();
        p.host.clear();
        assert!(matches!(
            p.validate(),
            Err(DomainError::Invalid { field: "host", .. })
        ));
    }

    #[test]
    fn secret_ref_is_derived_from_connection_id() {
        let id = ConnectionId::new();
        assert_eq!(
            SecretRef::for_connection(id).as_str(),
            format!("connection/{}", id.0)
        );
    }

    #[test]
    fn read_only_is_an_option_that_can_be_toggled() {
        let mut p = profile();
        assert!(!p.read_only());
        p.set_read_only(true);
        assert!(p.read_only());
        assert_eq!(p.options.get("read_only").map(String::as_str), Some("true"));
        p.set_read_only(false);
        assert!(!p.read_only() && p.options.is_empty());
    }

    #[test]
    fn inline_ai_override_round_trips_and_defaults_to_inherit() {
        let mut p = profile();
        assert_eq!(p.inline_ai(), None);
        p.set_inline_ai(Some(true));
        assert_eq!(p.inline_ai(), Some(true));
        p.set_inline_ai(Some(false));
        assert_eq!(p.inline_ai(), Some(false));
        p.set_inline_ai(None);
        assert_eq!(p.inline_ai(), None);
        assert!(p.options.is_empty());
        p.options.insert("inline_ai".into(), "garbage".into());
        assert_eq!(
            p.inline_ai(),
            None,
            "unknown values inherit the global setting"
        );
    }

    #[test]
    fn driver_kind_is_open_ended() {
        assert_eq!(DriverKind::new("clickhouse").as_str(), "clickhouse");
    }
}
