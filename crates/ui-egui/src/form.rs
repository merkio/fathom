//! Connection editor form state.

use std::fmt;

use domain::{ConnectionProfile, DriverKind};
use secrecy::SecretString;

#[derive(Clone, Default)]
pub struct ConnectionForm {
    /// The profile being edited, if any. Keeps its id, secret reference and options.
    base: Option<ConnectionProfile>,
    pub name: String,
    pub driver_kind: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub username: String,
    /// Per-connection read-only mode: only plain reads may run.
    pub read_only: bool,
    /// Per-connection override of LLM inline suggestions: `None` follows the global setting.
    pub inline_ai: Option<bool>,
    /// Entered text only; moved into a `SecretString` on submit and never stored in a profile.
    pub password: String,
}

// Manual `Debug`: the password must never reach logs.
impl fmt::Debug for ConnectionForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionForm")
            .field("name", &self.name)
            .field("driver_kind", &self.driver_kind)
            .field("host", &self.host)
            .field("password", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ConnectionForm {
    pub fn new(default_kind: Option<&DriverKind>) -> Self {
        Self {
            driver_kind: default_kind
                .map(|k| k.as_str().to_owned())
                .unwrap_or_default(),
            host: "localhost".into(),
            ..Self::default()
        }
    }

    pub fn from_profile(p: &ConnectionProfile) -> Self {
        Self {
            base: Some(p.clone()),
            name: p.name.clone(),
            driver_kind: p.driver_kind.as_str().to_owned(),
            host: p.host.clone(),
            port: p.port.map(|p| p.to_string()).unwrap_or_default(),
            database: p.database.clone().unwrap_or_default(),
            username: p.username.clone(),
            read_only: p.read_only(),
            inline_ai: p.inline_ai(),
            password: String::new(),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.base.is_some()
    }

    /// Builds the profile. Field validation beyond the port happens in the use case.
    pub fn to_profile(&self) -> Result<ConnectionProfile, String> {
        let port = match self.port.trim() {
            "" => None,
            p => Some(
                p.parse::<u16>()
                    .map_err(|_| format!("`{p}` is not a valid port"))?,
            ),
        };
        let mut profile = match &self.base {
            Some(base) => base.clone(),
            None => ConnectionProfile::new(
                self.name.trim(),
                DriverKind::new(self.driver_kind.trim()),
                self.host.trim(),
            ),
        };
        profile.name = self.name.trim().to_owned();
        profile.driver_kind = DriverKind::new(self.driver_kind.trim());
        profile.host = self.host.trim().to_owned();
        profile.port = port;
        profile.database = Some(self.database.trim().to_owned()).filter(|d| !d.is_empty());
        profile.username = self.username.trim().to_owned();
        profile.set_read_only(self.read_only);
        profile.set_inline_ai(self.inline_ai);
        Ok(profile)
    }

    /// Takes the typed password (leaving the field empty). `None` when nothing was typed, which
    /// keeps an existing stored secret.
    pub fn take_password(&mut self) -> Option<SecretString> {
        if self.password.is_empty() {
            None
        } else {
            Some(SecretString::from(std::mem::take(&mut self.password)))
        }
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn builds_profile_and_trims_fields() {
        let mut f = ConnectionForm::new(Some(&DriverKind::new("postgres")));
        f.name = " main ".into();
        f.port = "5433".into();
        f.database = " app ".into();
        f.username = "alice".into();
        let p = f.to_profile().expect("valid");
        assert_eq!(
            (p.name.as_str(), p.port, p.database.as_deref()),
            ("main", Some(5433), Some("app"))
        );
        assert_eq!(p.driver_kind.as_str(), "postgres");
    }

    #[test]
    fn invalid_port_is_reported_and_empty_means_default() {
        let mut f = ConnectionForm::new(None);
        f.port = "70000".into();
        assert!(f.to_profile().is_err());
        f.port = "  ".into();
        assert_eq!(f.to_profile().map(|p| p.port), Ok(None));
    }

    #[test]
    fn editing_keeps_identity_and_secret_ref() {
        let mut original = ConnectionProfile::new("a", DriverKind::new("postgres"), "h");
        original.secret_ref = Some(domain::SecretRef::new("k"));
        let mut f = ConnectionForm::from_profile(&original);
        f.name = "b".into();
        let edited = f.to_profile().expect("valid");
        assert_eq!(
            (edited.id, edited.secret_ref.clone()),
            (original.id, original.secret_ref)
        );
        assert_eq!(edited.name, "b");
        assert!(f.is_editing());
    }

    #[test]
    fn read_only_round_trips_through_the_profile() {
        let mut f = ConnectionForm::new(None);
        f.name = "n".into();
        f.read_only = true;
        let p = f.to_profile().expect("valid");
        assert!(p.read_only());
        assert!(ConnectionForm::from_profile(&p).read_only);
        f.read_only = false;
        assert!(!f.to_profile().expect("valid").read_only());
    }

    #[test]
    fn the_inline_override_round_trips_and_defaults_to_the_global_setting() {
        let mut f = ConnectionForm::new(None);
        f.name = "n".into();
        assert_eq!(f.inline_ai, None);
        assert_eq!(f.to_profile().expect("valid").inline_ai(), None);
        for choice in [Some(true), Some(false), None] {
            f.inline_ai = choice;
            let p = f.to_profile().expect("valid");
            assert_eq!(p.inline_ai(), choice);
            assert_eq!(ConnectionForm::from_profile(&p).inline_ai, choice);
        }
    }

    #[test]
    fn password_is_taken_once_and_never_debug_printed() {
        let mut f = ConnectionForm::new(None);
        assert!(f.take_password().is_none());
        f.password = "hunter2".into();
        assert!(!format!("{f:?}").contains("hunter2"));
        let taken = f.take_password().expect("typed");
        assert_eq!(taken.expose_secret(), "hunter2");
        assert!(f.password.is_empty() && f.take_password().is_none());
    }
}
