//! Connection editor form state.

use std::fmt;

use domain::{ConnectionProfile, DriverKind};
use secrecy::SecretString;

/// Options edited through their own controls rather than the free-form box.
const RESERVED_OPTIONS: &[&str] = &["read_only", "inline_ai"];

fn format_options(p: &ConnectionProfile) -> String {
    p.options
        .iter()
        .filter(|(k, _)| !RESERVED_OPTIONS.contains(&k.as_str()))
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses `key=value` lines. Blank lines and `#` comments are skipped; the value may contain `=`.
fn parse_options(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "Advanced options, line {}: expected key=value, found `{line}`",
                i + 1
            ));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!(
                "Advanced options, line {}: the key is empty",
                i + 1
            ));
        }
        if RESERVED_OPTIONS.contains(&key) {
            return Err(format!(
                "Advanced options, line {}: `{key}` has its own control in this dialog",
                i + 1
            ));
        }
        out.push((key.to_owned(), value.trim().to_owned()));
    }
    Ok(out)
}

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
    /// Driver-specific options, one `key=value` per line (for example `sslmode=verify-full`).
    /// `read_only` and `inline_ai` have their own controls and are not listed here.
    pub extra_options: String,
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
            extra_options: format_options(p),
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
        // Replace the free-form options (so a deleted line is really removed); the options with
        // their own controls are set below.
        profile
            .options
            .retain(|k, _| RESERVED_OPTIONS.contains(&k.as_str()));
        profile.options.extend(parse_options(&self.extra_options)?);
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
    fn advanced_options_are_parsed_trimmed_and_may_contain_equals_signs() {
        let mut f = ConnectionForm::new(None);
        f.name = "n".into();
        f.extra_options =
            "# TLS\n sslmode = verify-full \n\nsslrootcert=/etc/ssl/ca.pem\noptions=a=b=c\n".into();
        let p = f.to_profile().expect("valid");
        assert_eq!(
            p.options.get("sslmode").map(String::as_str),
            Some("verify-full")
        );
        assert_eq!(
            p.options.get("sslrootcert").map(String::as_str),
            Some("/etc/ssl/ca.pem")
        );
        assert_eq!(p.options.get("options").map(String::as_str), Some("a=b=c"));
        assert_eq!(p.options.len(), 3);
    }

    #[test]
    fn malformed_and_reserved_option_lines_are_rejected_with_the_line_number() {
        let mut f = ConnectionForm::new(None);
        f.name = "n".into();
        f.extra_options = "sslmode=require\njust text".into();
        assert!(f.to_profile().expect_err("no =").contains("line 2"));
        f.extra_options = "=value".into();
        assert!(
            f.to_profile()
                .expect_err("empty key")
                .contains("key is empty")
        );
        f.extra_options = "read_only=true".into();
        assert!(
            f.to_profile()
                .expect_err("reserved")
                .contains("own control")
        );
        f.extra_options = "inline_ai=on".into();
        assert!(f.to_profile().is_err());
    }

    #[test]
    fn editing_options_round_trips_and_deleting_a_line_removes_the_option() {
        let mut original = ConnectionProfile::new("a", DriverKind::new("postgres"), "h");
        original.options.insert("sslmode".into(), "require".into());
        original.options.insert("timeout".into(), "5".into());
        original.set_read_only(true);

        let mut f = ConnectionForm::from_profile(&original);
        assert_eq!(
            f.extra_options, "sslmode=require\ntimeout=5",
            "reserved options are not listed"
        );
        assert!(f.read_only);

        f.extra_options = "sslmode=require".into(); // the timeout line was deleted
        let edited = f.to_profile().expect("valid");
        assert!(
            !edited.options.contains_key("timeout"),
            "{:?}",
            edited.options
        );
        assert!(
            edited.read_only(),
            "the checkbox-controlled option survives"
        );
        assert_eq!(
            edited.options.get("sslmode").map(String::as_str),
            Some("require")
        );
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
