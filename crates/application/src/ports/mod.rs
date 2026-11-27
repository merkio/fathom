//! Ports: the interfaces adapters implement.

pub mod clock;
pub mod database;
pub mod export;
pub mod llm;
pub mod repositories;
pub mod secrets;

pub use clock::{Clock, SystemClock};
pub use database::{DatabaseDriver, DbError, DbSession, IntrospectScope};
pub use export::{DestinationConfig, DestinationFactory, ExportDestination, ExportError};
pub use llm::{LlmChunk, LlmError, LlmProvider, LlmRequest, LlmResponse, ModelInfo, Usage};
pub use repositories::{
    ConnectionRepository, HistoryEntry, QueryHistoryRepository, SavedQueryRepository,
    SettingsRepository, StorageError,
};
pub use secrets::{SecretError, SecretStore};
