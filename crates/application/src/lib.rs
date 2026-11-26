//! Use cases and ports (traits). Adapters implement the ports; only `app` wires concrete types.

pub mod error;
pub mod llm;
pub mod ports;
pub mod registry;
pub mod sql;
pub mod streaming;
pub mod usecases;
