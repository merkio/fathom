use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use application::ports::{SecretError, SecretStore};
use async_trait::async_trait;
use domain::SecretRef;
use secrecy::SecretString;

/// Process-local `SecretStore`. Fallback for headless environments (CI, containers) where no OS
/// keychain exists. Secrets vanish when the process exits.
#[derive(Default)]
pub struct MemorySecretStore {
    items: Mutex<HashMap<String, SecretString>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SecretStore for MemorySecretStore {
    async fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(items.get(key.as_str()).cloned())
    }

    async fn set(&self, key: &SecretRef, value: SecretString) -> Result<(), SecretError> {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.insert(key.as_str().to_owned(), value);
        Ok(())
    }

    async fn delete(&self, key: &SecretRef) -> Result<(), SecretError> {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.remove(key.as_str());
        Ok(())
    }
}
