use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use domain::{ConnectionId, ConnectionProfile};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StorageError {
    #[error("not found")]
    NotFound,
    #[error("storage failure: {0}")]
    Backend(String),
}

#[async_trait]
pub trait ConnectionRepository: Send + Sync {
    async fn list(&self) -> Result<Vec<ConnectionProfile>, StorageError>;
    async fn get(&self, id: ConnectionId) -> Result<ConnectionProfile, StorageError>;
    /// Inserts or replaces by id.
    async fn save(&self, profile: ConnectionProfile) -> Result<(), StorageError>;
    async fn delete(&self, id: ConnectionId) -> Result<(), StorageError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub connection_id: ConnectionId,
    pub text: String,
    pub executed_at: SystemTime,
    pub elapsed: Duration,
    pub succeeded: bool,
}

#[async_trait]
pub trait QueryHistoryRepository: Send + Sync {
    async fn append(&self, entry: HistoryEntry) -> Result<(), StorageError>;
    /// Most recent first.
    async fn recent(
        &self,
        connection_id: ConnectionId,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StorageError>;
}

/// Non-secret key/value settings. Never store secrets here.
#[async_trait]
pub trait SettingsRepository: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, StorageError>;
    async fn set(&self, key: &str, value: &str) -> Result<(), StorageError>;
}
