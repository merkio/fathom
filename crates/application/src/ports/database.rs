use async_trait::async_trait;
use domain::{ConnectionProfile, Dialect, DriverKind, QueryRequest, QueryResult, SchemaTree};
use secrecy::SecretString;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DbError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("authentication failed")]
    Authentication,
    #[error("query failed: {0}")]
    Query(String),
    #[error("query timed out")]
    Timeout,
    #[error("query cancelled")]
    Cancelled,
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// What part of the schema to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntrospectScope {
    /// Everything the user can see.
    All,
    /// A single schema by name.
    Schema(String),
}

/// A database kind. One implementation per `db-*` crate.
#[async_trait]
pub trait DatabaseDriver: Send + Sync {
    fn kind(&self) -> DriverKind;
    fn dialect(&self) -> Dialect;
    async fn connect(
        &self,
        profile: &ConnectionProfile,
        secret: Option<SecretString>,
    ) -> Result<Box<dyn DbSession>, DbError>;
}

/// An open connection to a database.
#[async_trait]
pub trait DbSession: Send + Sync {
    async fn introspect(&self, scope: IntrospectScope) -> Result<SchemaTree, DbError>;
    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError>;
    /// Cancels the statement currently running on this session, if any.
    async fn cancel(&self) -> Result<(), DbError>;
    async fn close(self: Box<Self>);
}
