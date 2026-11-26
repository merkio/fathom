use application::ports::{SecretError, SecretStore};
use async_trait::async_trait;
use domain::SecretRef;
use secrecy::{ExposeSecret, SecretString};

/// `SecretStore` backed by the OS keychain (Keychain, Credential Manager, Secret Service).
///
/// Calls are synchronous and short; use cases run on the background runtime, never the UI thread.
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &SecretRef) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, key.as_str()).map_err(unavailable)
    }
}

fn unavailable(e: keyring::Error) -> SecretError {
    // keyring error text never contains the secret value.
    SecretError::Unavailable(e.to_string())
}

#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(SecretString::from(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(unavailable(e)),
        }
    }

    async fn set(&self, key: &SecretRef, value: SecretString) -> Result<(), SecretError> {
        self.entry(key)?
            .set_password(value.expose_secret())
            .map_err(unavailable)
    }

    async fn delete(&self, key: &SecretRef) -> Result<(), SecretError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(unavailable(e)),
        }
    }
}
