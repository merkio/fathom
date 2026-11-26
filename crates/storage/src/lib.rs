//! Storage adapters: SQLite repositories and `SecretStore` implementations.
//!
//! Secrets are never written to SQLite. Connection rows hold only a `secret_ref` key.

mod keyring_store;
mod memory_secrets;
mod sqlite;

pub use keyring_store::KeyringSecretStore;
pub use memory_secrets::MemorySecretStore;
pub use sqlite::SqliteStore;
