//! Registries map ids to adapters. Adding a database or provider is one `register` call in `app`.

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{DriverKind, ProviderId, SecretRef};

use crate::ports::{DatabaseDriver, ExportDestination, LlmProvider};

#[derive(Default)]
pub struct DriverRegistry {
    drivers: BTreeMap<DriverKind, Arc<dyn DatabaseDriver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a driver under its own `kind()`, replacing any previous one.
    pub fn register(&mut self, driver: Arc<dyn DatabaseDriver>) {
        self.drivers.insert(driver.kind(), driver);
    }

    pub fn get(&self, kind: &DriverKind) -> Option<Arc<dyn DatabaseDriver>> {
        self.drivers.get(kind).cloned()
    }

    pub fn kinds(&self) -> Vec<DriverKind> {
        self.drivers.keys().cloned().collect()
    }

    /// Each registered kind with its dialect.
    pub fn dialects(&self) -> Vec<(DriverKind, domain::Dialect)> {
        self.drivers
            .iter()
            .map(|(k, d)| (k.clone(), d.dialect()))
            .collect()
    }
}

#[derive(Default)]
pub struct LlmRegistry {
    providers: BTreeMap<ProviderId, Arc<dyn LlmProvider>>,
    key_refs: BTreeMap<ProviderId, SecretRef>,
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn LlmProvider>) {
        self.providers.insert(provider.id(), provider);
    }

    /// Registers a provider that authenticates with an API key stored under `key`, so the
    /// settings screen knows where to save the key the user enters.
    pub fn register_with_key(&mut self, provider: Arc<dyn LlmProvider>, key: SecretRef) {
        self.key_refs.insert(provider.id(), key);
        self.register(provider);
    }

    /// The secret-store key of a provider that needs an API key.
    pub fn key_ref(&self, id: &ProviderId) -> Option<&SecretRef> {
        self.key_refs.get(id)
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(id).cloned()
    }

    pub fn ids(&self) -> Vec<ProviderId> {
        self.providers.keys().cloned().collect()
    }
}

/// A configured export destination.
pub struct RegisteredDestination {
    pub name: String,
    pub destination: Arc<dyn ExportDestination>,
}

/// The destinations the user configured, in configuration order.
#[derive(Default)]
pub struct DestinationRegistry {
    items: Vec<RegisteredDestination>,
    problems: Vec<String>,
}

impl DestinationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a destination; a later one with the same name replaces the earlier.
    pub fn register(&mut self, name: impl Into<String>, destination: Arc<dyn ExportDestination>) {
        let name = name.into();
        self.items.retain(|i| i.name != name);
        self.items.push(RegisteredDestination { name, destination });
    }

    /// Notes something wrong with the configuration (shown to the user, never fatal).
    pub fn report_problem(&mut self, problem: impl Into<String>) {
        self.problems.push(problem.into());
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn remove(&mut self, name: &str) {
        self.items.retain(|i| i.name != name);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ExportDestination>> {
        self.items
            .iter()
            .find(|i| i.name == name)
            .map(|i| Arc::clone(&i.destination))
    }

    /// Name, kind and where files land, for each destination.
    pub fn list(&self) -> Vec<DestinationInfo> {
        self.items
            .iter()
            .map(|i| DestinationInfo {
                name: i.name.clone(),
                kind: i.destination.kind().to_owned(),
                location: i.destination.describe(),
                editable: false,
                settings: std::collections::BTreeMap::new(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DestinationInfo {
    pub name: String,
    pub kind: String,
    pub location: String,
    /// Whether the user can edit it in the app (those from `fathom.toml` cannot).
    pub editable: bool,
    /// The non-secret settings, for the edit form.
    pub settings: std::collections::BTreeMap<String, String>,
}
