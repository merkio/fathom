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
    fn driver_kind_is_open_ended() {
        assert_eq!(DriverKind::new("clickhouse").as_str(), "clickhouse");
    }
}
