//! Storage connections the user manages in the app (add, edit, delete, test), next to the ones
//! declared in `fathom.toml`.
//!
//! Non-secret settings are stored as JSON in the key/value settings (no schema change). Secrets
//! (access keys, passwords) go only through the `SecretStore`; the JSON records just their names.
//! Connections from the configuration file are read-only here.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use domain::SecretRef;

use crate::error::UseCaseError;
use crate::ports::{
    DestinationConfig, DestinationFactory, ExportDestination, SecretStore, SettingsRepository,
};
use crate::registry::{DestinationInfo, DestinationRegistry};

const KEY: &str = "storage.connections";

type StorageConnectionError = UseCaseError;

/// One input of the add/edit form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub required: bool,
    pub hint: &'static str,
}

const fn field(
    key: &'static str,
    label: &'static str,
    required: bool,
    hint: &'static str,
) -> FieldSpec {
    FieldSpec {
        key,
        label,
        secret: false,
        required,
        hint,
    }
}

const fn secret(key: &'static str, label: &'static str, hint: &'static str) -> FieldSpec {
    FieldSpec {
        key,
        label,
        secret: true,
        required: false,
        hint,
    }
}

/// The fields the form shows for an adapter kind, and the label of the kind.
pub fn form_for(kind: &str) -> (&'static str, Vec<FieldSpec>) {
    match kind {
        "s3" => (
            "Amazon S3 / S3-compatible",
            vec![
                field("bucket", "Bucket", true, "my-exports"),
                field("region", "Region", false, "eu-west-1"),
                field("prefix", "Default folder prefix", false, "exports/"),
                field(
                    "endpoint",
                    "Endpoint URL",
                    false,
                    "https://minio.local:9000 (S3-compatible only)",
                ),
                field(
                    "profile",
                    "AWS profile",
                    false,
                    "leave empty for the default credential chain",
                ),
                secret(
                    "access_key_id",
                    "Access key ID",
                    "empty = use the AWS credential chain",
                ),
                secret("secret_access_key", "Secret access key", ""),
                secret(
                    "session_token",
                    "Session token",
                    "temporary credentials only",
                ),
            ],
        ),
        "smb" => (
            "SMB share (mounted)",
            vec![field("path", "Mount folder", true, "/mnt/reports or Z:\\")],
        ),
        "nfs" => (
            "NFS export (mounted)",
            vec![field("path", "Mount folder", true, "/mnt/nfs/reports")],
        ),
        _ => (
            "Local folder",
            vec![field("path", "Folder", true, "/home/me/fathom-exports")],
        ),
    }
}

/// What the form submits.
#[derive(Clone, Default)]
pub struct ConnectionDraft {
    /// The name the connection had before this edit (`None` for a new one).
    pub original_name: Option<String>,
    pub name: String,
    pub kind: String,
    pub settings: BTreeMap<String, String>,
    /// Secrets typed in the form. An empty value on an edit keeps the stored one.
    pub secrets: BTreeMap<String, SecretString>,
}

impl std::fmt::Debug for ConnectionDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionDraft")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Stored {
    name: String,
    kind: String,
    #[serde(default)]
    settings: BTreeMap<String, String>,
    /// Names of the secrets kept in the secret store.
    #[serde(default)]
    secrets: Vec<String>,
}

struct State {
    stored: Vec<Stored>,
    built: DestinationRegistry,
}

pub struct StorageConnections {
    settings: Arc<dyn SettingsRepository>,
    secrets: Arc<dyn SecretStore>,
    factories: Vec<Box<dyn DestinationFactory>>,
    /// Kinds the configuration allows.
    enabled: Vec<String>,
    /// Destinations from the configuration file, read-only.
    file: DestinationRegistry,
    state: RwLock<State>,
}

fn secret_ref(name: &str, secret: &str) -> SecretRef {
    SecretRef::new(format!("storage/{name}/{secret}"))
}

impl StorageConnections {
    pub fn new(
        settings: Arc<dyn SettingsRepository>,
        secrets: Arc<dyn SecretStore>,
        factories: Vec<Box<dyn DestinationFactory>>,
        enabled: Vec<String>,
        file: DestinationRegistry,
    ) -> Self {
        Self {
            settings,
            secrets,
            factories,
            enabled,
            file,
            state: RwLock::new(State {
                stored: Vec::new(),
                built: DestinationRegistry::new(),
            }),
        }
    }

    /// The adapter kinds the user may add (built in and enabled).
    pub fn kinds(&self) -> Vec<String> {
        self.factories
            .iter()
            .map(|f| f.kind().to_owned())
            .filter(|k| self.enabled.contains(k))
            .collect()
    }

    fn factory(&self, kind: &str) -> Result<&dyn DestinationFactory, StorageConnectionError> {
        if !self.enabled.iter().any(|k| k == kind) {
            return Err(UseCaseError::Blocked(format!(
                "The `{kind}` adapter is disabled. Add it to `enabled` under [export] in fathom.toml."
            )));
        }
        self.factories
            .iter()
            .find(|f| f.kind() == kind)
            .map(|f| f.as_ref())
            .ok_or_else(|| UseCaseError::Blocked(format!("This build has no `{kind}` adapter.")))
    }

    /// Reads the stored connections and builds their destinations. Broken entries become
    /// problems; they never stop start-up.
    pub async fn load(&self) -> Result<(), StorageConnectionError> {
        let stored: Vec<Stored> = match self.settings.get(KEY).await? {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => Vec::new(),
        };
        let mut built = DestinationRegistry::new();
        for s in &stored {
            match self.build(s, &BTreeMap::new()).await {
                Ok(d) => built.register(s.name.clone(), d),
                Err(e) => built.report_problem(format!("{}: {e}", s.name)),
            }
        }
        if let Ok(mut state) = self.state.write() {
            *state = State { stored, built };
        }
        Ok(())
    }

    /// Builds a destination from stored settings. `typed` secrets win over stored ones.
    async fn build(
        &self,
        s: &Stored,
        typed: &BTreeMap<String, SecretString>,
    ) -> Result<Arc<dyn ExportDestination>, StorageConnectionError> {
        let mut secrets = BTreeMap::new();
        for name in &s.secrets {
            let value = match typed.get(name).filter(|v| !v.expose_secret().is_empty()) {
                Some(v) => Some(v.clone()),
                None => self.secrets.get(&secret_ref(&s.name, name)).await?,
            };
            if let Some(v) = value {
                secrets.insert(name.clone(), v);
            }
        }
        let config = DestinationConfig {
            name: s.name.clone(),
            kind: s.kind.clone(),
            settings: s.settings.clone(),
            secrets,
        };
        Ok(self.factory(&s.kind)?.create(&config)?)
    }

    fn validate(&self, draft: &ConnectionDraft) -> Result<Stored, StorageConnectionError> {
        let name = draft.name.trim().to_owned();
        if name.is_empty() {
            return Err(UseCaseError::Blocked("Give the connection a name.".into()));
        }
        let (_, fields) = form_for(&draft.kind);
        let mut settings = BTreeMap::new();
        for f in fields.iter().filter(|f| !f.secret) {
            let v = draft.settings.get(f.key).map(|v| v.trim()).unwrap_or("");
            if v.is_empty() {
                if f.required {
                    return Err(UseCaseError::Blocked(format!("{} is required.", f.label)));
                }
            } else {
                settings.insert(f.key.to_owned(), v.to_owned());
            }
        }
        let secrets = fields
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.key.to_owned())
            .collect();
        Ok(Stored {
            name,
            kind: draft.kind.clone(),
            settings,
            secrets,
        })
    }

    /// Checks the draft can be written to, without saving anything.
    pub async fn test_draft(&self, draft: &ConnectionDraft) -> Result<(), StorageConnectionError> {
        let stored = self.validate(draft)?;
        let typed = self.typed_with_existing(draft).await?;
        let destination = self.build(&stored, &typed).await?;
        Ok(destination.test().await?)
    }

    /// An edit keeps the secrets stored under the original name when the form leaves them empty.
    async fn typed_with_existing(
        &self,
        draft: &ConnectionDraft,
    ) -> Result<BTreeMap<String, SecretString>, StorageConnectionError> {
        let mut typed = draft.secrets.clone();
        if let Some(old) = &draft.original_name {
            for (name, _) in form_for(&draft.kind)
                .1
                .iter()
                .filter(|f| f.secret)
                .map(|f| (f.key, ()))
            {
                let empty = typed.get(name).is_none_or(|v| v.expose_secret().is_empty());
                if empty && let Some(v) = self.secrets.get(&secret_ref(old, name)).await? {
                    typed.insert(name.to_owned(), v);
                }
            }
        }
        Ok(typed)
    }

    pub async fn save(&self, draft: ConnectionDraft) -> Result<(), StorageConnectionError> {
        let stored = self.validate(&draft)?;
        let renamed = draft
            .original_name
            .as_ref()
            .is_some_and(|o| o != &stored.name);
        let taken = self.file.get(&stored.name).is_some()
            || self
                .state
                .read()
                .map(|s| {
                    s.stored.iter().any(|c| {
                        c.name == stored.name && draft.original_name.as_deref() != Some(&c.name)
                    })
                })
                .unwrap_or(false);
        if taken {
            return Err(UseCaseError::Blocked(format!(
                "A connection named `{}` already exists.",
                stored.name
            )));
        }
        let typed = self.typed_with_existing(&draft).await?;
        // Build first: a connection that cannot be created is never saved.
        let destination = self.build(&stored, &typed).await?;
        for (name, value) in &typed {
            if stored.secrets.contains(name) && !value.expose_secret().is_empty() {
                self.secrets
                    .set(&secret_ref(&stored.name, name), value.clone())
                    .await?;
            }
        }
        if renamed && let Some(old) = &draft.original_name {
            for name in &stored.secrets {
                self.secrets.delete(&secret_ref(old, name)).await?;
            }
        }
        let list = {
            let Ok(mut state) = self.state.write() else {
                return Ok(());
            };
            let original = draft.original_name.as_deref().unwrap_or(&stored.name);
            state.stored.retain(|c| c.name != original);
            state.built.remove(original);
            state.stored.push(stored.clone());
            state.stored.sort_by_key(|c| c.name.to_lowercase());
            state.built.register(stored.name.clone(), destination);
            state.stored.clone()
        };
        self.persist(&list).await
    }

    pub async fn delete(&self, name: &str) -> Result<(), StorageConnectionError> {
        let (list, secrets) = {
            let Ok(mut state) = self.state.write() else {
                return Ok(());
            };
            let secrets = state
                .stored
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.secrets.clone())
                .unwrap_or_default();
            state.stored.retain(|c| c.name != name);
            state.built.remove(name);
            (state.stored.clone(), secrets)
        };
        for s in secrets {
            self.secrets.delete(&secret_ref(name, &s)).await?;
        }
        self.persist(&list).await
    }

    async fn persist(&self, list: &[Stored]) -> Result<(), StorageConnectionError> {
        let json = serde_json::to_string(list).map_err(|e| UseCaseError::Blocked(e.to_string()))?;
        Ok(self.settings.set(KEY, &json).await?)
    }

    /// Looks a destination up by name: configuration file first, then the app's own.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ExportDestination>> {
        self.file
            .get(name)
            .or_else(|| self.state.read().ok().and_then(|s| s.built.get(name)))
    }

    pub fn list(&self) -> Vec<DestinationInfo> {
        let mut items: Vec<DestinationInfo> = self
            .file
            .list()
            .into_iter()
            .map(|mut i| {
                i.editable = false;
                i
            })
            .collect();
        if let Ok(state) = self.state.read() {
            for s in &state.stored {
                let location = state
                    .built
                    .get(&s.name)
                    .map(|d| d.describe())
                    .unwrap_or_else(|| "not available".into());
                items.push(DestinationInfo {
                    name: s.name.clone(),
                    kind: s.kind.clone(),
                    location,
                    editable: true,
                    settings: s.settings.clone(),
                });
            }
        }
        items
    }

    pub fn problems(&self) -> Vec<String> {
        let mut p = self.file.problems().to_vec();
        if let Ok(state) = self.state.read() {
            p.extend(state.built.problems().iter().cloned());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::ports::{ExportError, SecretError, StorageError};

    #[derive(Default)]
    struct Settings(Mutex<BTreeMap<String, String>>);

    #[async_trait]
    impl SettingsRepository for Settings {
        async fn get(&self, key: &str) -> Result<Option<String>, StorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &str) -> Result<(), StorageError> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
    }

    #[derive(Default)]
    struct Secrets(Mutex<BTreeMap<String, String>>);

    #[async_trait]
    impl SecretStore for Secrets {
        async fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(key.as_str())
                .map(|v| SecretString::from(v.clone())))
        }
        async fn set(&self, key: &SecretRef, value: SecretString) -> Result<(), SecretError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.as_str().into(), value.expose_secret().into());
            Ok(())
        }
        async fn delete(&self, key: &SecretRef) -> Result<(), SecretError> {
            self.0.lock().unwrap().remove(key.as_str());
            Ok(())
        }
    }

    struct Dest(DestinationConfig);

    #[async_trait]
    impl ExportDestination for Dest {
        fn kind(&self) -> &'static str {
            "local"
        }
        fn describe(&self) -> String {
            self.0.settings.get("path").cloned().unwrap_or_default()
        }
        async fn test(&self) -> Result<(), ExportError> {
            if self.0.settings.get("path").map(String::as_str) == Some("/denied") {
                Err(ExportError::Denied("read-only".into()))
            } else {
                Ok(())
            }
        }
        async fn put(&self, path: &str, _: Vec<u8>) -> Result<String, ExportError> {
            Ok(path.into())
        }
    }

    struct Factory(&'static str);

    impl DestinationFactory for Factory {
        fn kind(&self) -> &'static str {
            self.0
        }
        fn create(&self, c: &DestinationConfig) -> Result<Arc<dyn ExportDestination>, ExportError> {
            Ok(Arc::new(Dest(c.clone())))
        }
    }

    fn make(
        settings: &Arc<Settings>,
        secrets: &Arc<Secrets>,
        enabled: &[&str],
    ) -> StorageConnections {
        StorageConnections::new(
            settings.clone(),
            secrets.clone(),
            vec![Box::new(Factory("local")), Box::new(Factory("s3"))],
            enabled.iter().map(|s| (*s).to_owned()).collect(),
            DestinationRegistry::new(),
        )
    }

    fn draft(name: &str, kind: &str, path: &str) -> ConnectionDraft {
        let mut d = ConnectionDraft {
            name: name.into(),
            kind: kind.into(),
            ..ConnectionDraft::default()
        };
        d.settings.insert("path".into(), path.into());
        d
    }

    #[tokio::test]
    async fn saved_connections_survive_a_restart_and_can_be_used() {
        let (st, se) = (Arc::new(Settings::default()), Arc::new(Secrets::default()));
        let uc = make(&st, &se, &["local"]);
        uc.save(draft("Reports", "local", "/data")).await.unwrap();
        assert!(uc.get("Reports").is_some());

        let again = make(&st, &se, &["local"]);
        again.load().await.unwrap();
        let list = again.list();
        assert_eq!(list.len(), 1);
        assert!(list[0].editable);
        assert_eq!(list[0].location, "/data");
    }

    #[tokio::test]
    async fn secrets_go_to_the_secret_store_and_never_into_settings() {
        let (st, se) = (Arc::new(Settings::default()), Arc::new(Secrets::default()));
        let uc = make(&st, &se, &["s3"]);
        let mut d = ConnectionDraft {
            name: "Bucket".into(),
            kind: "s3".into(),
            ..ConnectionDraft::default()
        };
        d.settings.insert("bucket".into(), "b".into());
        d.secrets
            .insert("secret_access_key".into(), SecretString::from("hunter2"));
        uc.save(d).await.unwrap();
        let json = st.0.lock().unwrap().get(KEY).cloned().unwrap();
        assert!(!json.contains("hunter2"));
        assert_eq!(
            se.0.lock()
                .unwrap()
                .get("storage/Bucket/secret_access_key")
                .map(String::as_str),
            Some("hunter2")
        );
        uc.delete("Bucket").await.unwrap();
        assert!(se.0.lock().unwrap().is_empty());
        assert!(uc.list().is_empty());
    }

    #[tokio::test]
    async fn rename_keeps_secrets_and_edit_with_empty_secret_keeps_the_old_one() {
        let (st, se) = (Arc::new(Settings::default()), Arc::new(Secrets::default()));
        let uc = make(&st, &se, &["s3"]);
        let mut d = ConnectionDraft {
            name: "A".into(),
            kind: "s3".into(),
            ..ConnectionDraft::default()
        };
        d.settings.insert("bucket".into(), "b".into());
        d.secrets
            .insert("secret_access_key".into(), SecretString::from("k"));
        uc.save(d.clone()).await.unwrap();
        d.original_name = Some("A".into());
        d.name = "B".into();
        d.secrets.clear();
        uc.save(d).await.unwrap();
        let store = se.0.lock().unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.get("storage/B/secret_access_key").map(String::as_str),
            Some("k")
        );
        drop(store);
        assert_eq!(uc.list()[0].name, "B");
    }

    #[tokio::test]
    async fn invalid_duplicate_and_disabled_connections_are_rejected() {
        let (st, se) = (Arc::new(Settings::default()), Arc::new(Secrets::default()));
        let uc = make(&st, &se, &["local"]);
        assert!(uc.save(draft("", "local", "/x")).await.is_err());
        assert!(uc.save(draft("A", "local", " ")).await.is_err());
        assert!(uc.save(draft("S", "s3", "/x")).await.is_err());
        uc.save(draft("A", "local", "/x")).await.unwrap();
        assert!(uc.save(draft("A", "local", "/y")).await.is_err());
        assert_eq!(uc.kinds(), vec!["local".to_owned()]);
    }

    #[tokio::test]
    async fn test_reports_a_destination_that_cannot_be_written() {
        let (st, se) = (Arc::new(Settings::default()), Arc::new(Secrets::default()));
        let uc = make(&st, &se, &["local"]);
        assert!(uc.test_draft(&draft("A", "local", "/ok")).await.is_ok());
        assert!(
            uc.test_draft(&draft("A", "local", "/denied"))
                .await
                .is_err()
        );
        assert!(uc.list().is_empty(), "testing saves nothing");
    }
}
