//! Use cases. Each is a small struct holding the ports it needs.

pub mod connections;

pub use connections::{
    CloseSession, DeleteConnection, ListConnections, OpenSession, SaveConnection, TestConnection,
};
