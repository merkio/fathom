//! Domain-level errors.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("invalid value for `{field}`: {reason}")]
    Invalid { field: &'static str, reason: String },
}
