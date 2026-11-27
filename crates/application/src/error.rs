use domain::{ConnectionId, DomainError, DriverKind, ProviderId};
use thiserror::Error;

use crate::ports::{DbError, ExportError, LlmError, SecretError, StorageError};

/// Error returned by use cases; wraps the lower-level errors they combine.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UseCaseError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error("no LLM provider registered for `{}`", .0.as_str())]
    UnknownProvider(ProviderId),
    #[error("{0}")]
    Blocked(String),
    #[error("could not write the export: {0}")]
    Io(String),
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error("no driver registered for `{}`", .0.as_str())]
    UnknownDriver(DriverKind),
    #[error("connection {} not found", .0.0)]
    ConnectionNotFound(ConnectionId),
}
