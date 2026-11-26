//! Use cases. Each is a small struct holding the ports it needs.

pub mod assistant;
pub mod autocomplete;
pub mod bundle;
pub mod connections;
pub mod data;

pub use assistant::{AssistReply, AssistRequest, AssistTask, Assistant, PromptPreview};
pub use autocomplete::CompleteSql;
pub use bundle::{Ports, UseCases};
pub use connections::{
    CloseSession, DeleteConnection, ListConnections, OpenSession, SaveConnection, TestConnection,
};
pub use data::{BrowseSchema, ExecuteOptions, ExecuteQuery, QueryHistory};
