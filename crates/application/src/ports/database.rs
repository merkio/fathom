use async_trait::async_trait;
use domain::{
    ApplyOutcome, ConnectionProfile, Dialect, DriverKind, QueryRequest, QueryResult, SchemaTree,
    TableChanges,
};
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
    /// Applies staged row edits. Every change is keyed by primary key and bound as a parameter
    /// (build the statements with `sql::changes::build`). Databases with transactions apply the
    /// whole batch atomically and roll back on any failure; the rest stop at the first failure.
    /// The default says the database cannot do it.
    async fn apply_changes(&self, _changes: TableChanges) -> Result<ApplyOutcome, DbError> {
        Err(DbError::Unsupported(
            "this database cannot apply row edits".into(),
        ))
    }
    async fn close(self: Box<Self>);
}
