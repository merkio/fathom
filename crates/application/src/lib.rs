//! Use cases and ports (traits). Adapters implement the ports; only `app` wires concrete types.

pub mod error;
pub mod error_text;
pub mod export;
pub mod export_parquet;
pub mod llm;
pub mod ports;
pub mod redact;
pub mod registry;
pub mod sql;
pub mod streaming;
pub mod usecases;
