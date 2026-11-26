//! Domain entities and value objects. No I/O, no async runtime, no framework dependencies.

pub mod connection;
pub mod dialect;
pub mod error;
pub mod llm;
pub mod query;
pub mod schema;

pub use connection::{ConnectionId, ConnectionProfile, DriverKind, SecretRef};
pub use dialect::{Dialect, LimitStyle};
pub use error::DomainError;
pub use llm::{ChatMessage, ModelId, ProviderId, Role};
pub use query::{QueryLanguage, QueryRequest, QueryResult, ResultColumn, Value};
pub use schema::{
    Catalog, Column, DataType, ForeignKey, Index, Schema, SchemaTree, Table, TableKind,
};
