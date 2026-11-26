use async_trait::async_trait;
use domain::SecretRef;
use secrecy::SecretString;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecretError {
    #[error("secret store unavailable: {0}")]
    Unavailable(String),
}

/// The only place passwords and API keys are persisted (OS keychain in production).
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError>;
    async fn set(&self, key: &SecretRef, value: SecretString) -> Result<(), SecretError>;
    /// Deleting a missing key is not an error.
    async fn delete(&self, key: &SecretRef) -> Result<(), SecretError>;
}
