//! Composition root: the only place that knows concrete adapter types.

// A GUI program: no console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod logging;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use application::ports::{DestinationFactory, SecretStore, SystemClock};
use application::registry::{DriverRegistry, LlmRegistry};
use application::usecases::{ExecuteOptions, ExportOptions, Ports, StorageConnections, UseCases};
use domain::SecretRef;
use secrecy::SecretString;
use storage::{KeyringSecretStore, MemorySecretStore, SqliteStore};

const KEYRING_SERVICE: &str = "fathom";

fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = match std::env::var_os("FATHOM_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => directories::ProjectDirs::from("", "", "fathom")
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
    if std::env::var("FATHOM_SECRET_STORE").as_deref() == Ok("memory") {
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

/// LLM providers: one registration line each. Keys live in the secret store under the given
/// references; the settings screen saves them there. Ollama needs none and defaults to the local
/// machine, so nothing leaves it unless the user picks a cloud provider.
///
/// `FATHOM_OLLAMA_URL` and `FATHOM_OPENAI_BASE_URL` point the adapters at other servers (for
/// example a remote Ollama or an OpenAI-compatible endpoint).
fn llm_providers(secrets: Arc<dyn SecretStore>) -> anyhow::Result<LlmRegistry> {
    let mut registry = LlmRegistry::new();

    let ollama = match std::env::var("FATHOM_OLLAMA_URL") {
        Ok(url) => llm_ollama::OllamaConfig::with_base_url(url),
        Err(_) => llm_ollama::OllamaConfig::default(),
    };
    registry.register(Arc::new(llm_ollama::OllamaProvider::new(ollama)?));

    let anthropic_key = SecretRef::new("llm/anthropic");
    let anthropic = llm_anthropic::AnthropicConfig::new(anthropic_key.clone());
    registry.register_with_key(
        Arc::new(llm_anthropic::AnthropicProvider::new(
            anthropic,
            secrets.clone(),
        )?),
        anthropic_key,
    );

    let openai_key = SecretRef::new("llm/openai");
    let mut openai = llm_openai::OpenAiConfig::new(openai_key.clone());
    if let Ok(url) = std::env::var("FATHOM_OPENAI_BASE_URL") {
        openai = openai.with_base_url(url);
    }
    registry.register_with_key(
        Arc::new(llm_openai::OpenAiProvider::new(openai, secrets)?),
        openai_key,
    );

    Ok(registry)
}

/// Exports go to the user's Documents folder (or home), else the data directory.
fn export_dir(data_dir: &std::path::Path) -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| {
            d.document_dir()
                .map(PathBuf::from)
                .or_else(|| Some(d.home_dir().to_path_buf()))
        })
        .unwrap_or_else(|| data_dir.join("exports"))
}

/// Export adapters: one line each. Which of them may be used is decided by `enabled` in the
/// configuration file, not here.
fn export_factories() -> Vec<Box<dyn DestinationFactory>> {
    vec![
        Box::new(export_local::FolderFactory::local()),
        Box::new(export_local::FolderFactory::smb()),
        Box::new(export_local::FolderFactory::nfs()),
        Box::new(export_s3::S3Factory),
    ]
}

/// Reads `fathom.toml` (or the file named by `FATHOM_CONFIG`). A missing file is normal; a broken
/// one is reported and ignored, so the application always starts.
fn load_config(data: &std::path::Path) -> config::Config {
    let path = std::env::var_os("FATHOM_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("fathom.toml"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return config::Config {
            enabled: config::default_enabled(),
            ..config::Config::default()
        };
    };
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let env = |name: &str| std::env::var(name).ok();
    let read_file = |p: &str| std::fs::read_to_string(p).ok();
    match config::parse(&text, &env, &read_file, home.as_deref()) {
        Ok(config) => {
            tracing::info!(
                file = %path.display(),
                destinations = config.destinations.len(),
                problems = config.problems.len(),
                "configuration loaded"
            );
            config
        }
        Err(message) => {
            tracing::warn!(file = %path.display(), error = %message, "configuration ignored");
            config::Config {
                enabled: config::default_enabled(),
                problems: vec![format!("{}: {message}", path.display())],
                ..config::Config::default()
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let data = data_dir()?;
    logging::init(&data);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
    let store = Arc::new(SqliteStore::open(data.join("fathom.sqlite"))?);

    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(db_postgres::PostgresDriver::new()));
    drivers.register(Arc::new(db_mysql::MySqlDriver::new()));
    drivers.register(Arc::new(db_sqlite::SqliteDriver::new()));
    drivers.register(Arc::new(db_clickhouse::ClickHouseDriver::new()));

    let config = load_config(&data);
    let secrets = secret_store();
    let llm = llm_providers(secrets.clone())?;

    let storage = Arc::new(StorageConnections::new(
        store.clone(),
        secrets.clone(),
        export_factories(),
        config.enabled.clone(),
        config::build_registry(&config, &export_factories(), &export_dir(&data)),
    ));
    let use_cases = UseCases::new(
        Ports {
            connections: store.clone(),
            history: store.clone(),
            settings: store.clone(),
            saved_queries: store,
            secrets,
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(llm),
        },
        ExecuteOptions::default(),
    )
    .with_export(ExportOptions {
        max_rows: config
            .max_export_rows
            .unwrap_or(application::usecases::DEFAULT_MAX_EXPORT_ROWS),
    })
    .with_destinations(storage);

    ui_egui::run(Arc::new(use_cases)).map_err(|e| anyhow!("{e}"))
}
