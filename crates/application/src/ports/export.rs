//! Where exported files go. One adapter per kind of place (a folder, an S3 bucket, a mounted
//! network share); which ones exist is decided by the user's configuration, not by the code.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use secrecy::SecretString;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExportError {
    #[error("`{0}` already exists. Choose another name; existing files are never overwritten.")]
    AlreadyExists(String),
    /// The path or the configuration is not acceptable (the message says why).
    #[error("{0}")]
    Invalid(String),
    #[error("access denied: {0}")]
    Denied(String),
    #[error("could not reach the destination: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Other(String),
}

/// One configured destination, with environment variables and files already resolved.
#[derive(Clone, Default)]
pub struct DestinationConfig {
    /// The name shown to the user ("Analytics archive").
    pub name: String,
    /// The adapter kind ("local", "s3", ...).
    pub kind: String,
    /// Non-secret settings (bucket, region, folder, ...).
    pub settings: BTreeMap<String, String>,
    /// Secret settings (keys, passwords). Never logged or displayed.
    pub secrets: BTreeMap<String, SecretString>,
}

// Manual `Debug`: secrets must never reach logs.
impl fmt::Debug for DestinationConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DestinationConfig")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("settings", &self.settings)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[async_trait]
pub trait ExportDestination: Send + Sync {
    fn kind(&self) -> &'static str;

    /// Where files land, for display ("s3://bucket/exports/", "/mnt/share/reports").
    fn describe(&self) -> String;

    /// Checks that the destination can be reached and written to, without leaving anything behind.
    async fn test(&self) -> Result<(), ExportError>;

    /// Writes `data` at `path` (relative to the destination's root, `/`-separated). Must fail with
    /// `AlreadyExists` instead of overwriting, and must refuse paths that leave the root.
    /// Returns where the file ended up, for display.
    async fn put(&self, path: &str, data: Vec<u8>) -> Result<String, ExportError>;
}

/// Builds destinations of one kind from configuration. One implementation per adapter crate.
pub trait DestinationFactory: Send + Sync {
    fn kind(&self) -> &'static str;
    fn create(&self, config: &DestinationConfig)
    -> Result<Arc<dyn ExportDestination>, ExportError>;
}
