//! Composition root: the only place that knows concrete adapter types.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use application::ports::{SecretStore, SystemClock};
use application::registry::DriverRegistry;
use application::usecases::{ExecuteOptions, Ports, UseCases};
use domain::SecretRef;
use secrecy::SecretString;
use storage::{KeyringSecretStore, MemorySecretStore, SqliteStore};

const KEYRING_SERVICE: &str = "dbiever";

fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = match std::env::var_os("DBIEVER_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => directories::ProjectDirs::from("", "", "dbiever")
            .context("could not determine the application data directory")?
            .data_dir()
            .to_path_buf(),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// OS keychain when available. Headless machines have none, so passwords then live in memory
/// only (they are never written to disk) and must be re-entered after a restart.
fn secret_store() -> Arc<dyn SecretStore> {
    if std::env::var("DBIEVER_SECRET_STORE").as_deref() == Ok("memory") {
        return Arc::new(MemorySecretStore::new());
    }
    let keyring = KeyringSecretStore::new(KEYRING_SERVICE);
    let probe = SecretRef::new("startup-probe");
    let usable = futures::executor::block_on(async {
        keyring.set(&probe, SecretString::from("probe")).await?;
        keyring.delete(&probe).await
    });
    match usable {
        Ok(()) => Arc::new(keyring),
        Err(e) => {
            eprintln!("OS keychain unavailable ({e}); passwords will be kept in memory only");
            Arc::new(MemorySecretStore::new())
        }
    }
}

fn main() -> anyhow::Result<()> {
    let store = Arc::new(SqliteStore::open(data_dir()?.join("dbiever.sqlite"))?);

    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(db_postgres::PostgresDriver::new()));

    let use_cases = UseCases::new(
        Ports {
            connections: store.clone(),
            history: store,
            secrets: secret_store(),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
        },
        ExecuteOptions::default(),
    );

    ui_egui::run(Arc::new(use_cases)).map_err(|e| anyhow!("{e}"))
}
