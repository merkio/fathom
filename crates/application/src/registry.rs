//! Registries map ids to adapters. Adding a database or provider is one `register` call in `app`.

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{DriverKind, ProviderId};

use crate::ports::{DatabaseDriver, LlmProvider};

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
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn LlmProvider>) {
        self.providers.insert(provider.id(), provider);
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(id).cloned()
    }

    pub fn ids(&self) -> Vec<ProviderId> {
        self.providers.keys().cloned().collect()
    }
}
