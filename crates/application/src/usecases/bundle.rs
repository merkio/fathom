//! Wiring helper: builds every use case from the ports once, so `app` stays a thin composition
//! root and the UI receives ready-made use cases.

use std::sync::Arc;

use domain::{DriverKind, ProviderId};

use crate::ports::{
    Clock, ConnectionRepository, QueryHistoryRepository, SavedQueryRepository, SecretStore,
    SettingsRepository,
};
use crate::registry::{DestinationRegistry, DriverRegistry, LlmRegistry};
use crate::usecases::{
    AiSettingsStore, ApiKeys, ApplyChanges, Assistant, BrowseSchema, CloseSession,
    DeleteConnection, ExecuteOptions, ExecuteQuery, ExplorerVisibilityStore, ListConnections,
    ListModels, OpenSession, QueryHistory, SaveConnection, SavedQueries, StorageConnections,
    SuggestInline, TestConnection, UiPrefsStore,
};

/// The adapters (as ports) the use cases need.
pub struct Ports {
    pub connections: Arc<dyn ConnectionRepository>,
    pub history: Arc<dyn QueryHistoryRepository>,
    pub secrets: Arc<dyn SecretStore>,
    pub clock: Arc<dyn Clock>,
    pub drivers: Arc<DriverRegistry>,
    pub providers: Arc<LlmRegistry>,
    pub settings: Arc<dyn SettingsRepository>,
    pub saved_queries: Arc<dyn SavedQueryRepository>,
}

/// Token budget for the schema part of a prompt.
const SCHEMA_TOKEN_BUDGET: usize = 3000;

/// How many rows an export may hold. (Where files go is decided by the configured destinations.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub max_rows: u32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            max_rows: crate::usecases::DEFAULT_MAX_EXPORT_ROWS,
        }
    }
}

pub struct UseCases {
    pub list_connections: ListConnections,
    pub save_connection: SaveConnection,
    pub delete_connection: DeleteConnection,
    pub test_connection: TestConnection,
    pub open_session: OpenSession,
    pub close_session: CloseSession,
    pub browse_schema: BrowseSchema,
    pub execute_query: ExecuteQuery,
    pub apply_changes: ApplyChanges,
    pub query_history: QueryHistory,
    pub assistant: Assistant,
    pub ai_settings: AiSettingsStore,
    pub api_keys: ApiKeys,
    pub list_models: ListModels,
    pub suggest_inline: SuggestInline,
    pub export: ExportOptions,
    /// The export destinations the user configured.
    pub destinations: Arc<StorageConnections>,
    pub ui_prefs: UiPrefsStore,
    pub saved_queries: SavedQueries,
    pub explorer_visibility: ExplorerVisibilityStore,
    drivers: Arc<DriverRegistry>,
    providers: Arc<LlmRegistry>,
}

impl UseCases {
    pub fn new(ports: Ports, options: ExecuteOptions) -> Self {
        let Ports {
            connections,
            history,
            secrets,
            clock,
            drivers,
            providers,
            settings,
            saved_queries,
        } = ports;
        Self {
            list_connections: ListConnections::new(connections.clone()),
            save_connection: SaveConnection::new(connections.clone(), secrets.clone()),
            delete_connection: DeleteConnection::new(connections.clone(), secrets.clone()),
            test_connection: TestConnection::new(drivers.clone(), secrets.clone()),
            open_session: OpenSession::new(connections, drivers.clone(), secrets.clone()),
            close_session: CloseSession,
            browse_schema: BrowseSchema,
            execute_query: ExecuteQuery::new(history.clone(), clock.clone(), options),
            apply_changes: ApplyChanges,
            query_history: QueryHistory::new(history),
            assistant: Assistant::new(providers.clone(), SCHEMA_TOKEN_BUDGET),
            suggest_inline: SuggestInline::new(providers.clone(), settings.clone()),
            export: ExportOptions::default(),
            destinations: Arc::new(StorageConnections::new(
                settings.clone(),
                secrets.clone(),
                Vec::new(),
                Vec::new(),
                DestinationRegistry::new(),
            )),
            ui_prefs: UiPrefsStore::new(settings.clone()),
            saved_queries: SavedQueries::new(saved_queries, clock.clone()),
            explorer_visibility: ExplorerVisibilityStore::new(settings.clone()),
            ai_settings: AiSettingsStore::new(settings),
            api_keys: ApiKeys::new(providers.clone(), secrets),
            list_models: ListModels::new(providers.clone()),
            drivers,
            providers,
        }
    }

    /// Each connectable database kind with its dialect (keywords, quoting) for the editor.
    pub fn dialects(&self) -> Vec<(DriverKind, domain::Dialect)> {
        self.drivers.dialects()
    }

    /// Sets the export row cap.
    pub fn with_export(mut self, export: ExportOptions) -> Self {
        self.export = export;
        self
    }

    /// Sets the configured export destinations.
    pub fn with_destinations(mut self, destinations: Arc<StorageConnections>) -> Self {
        self.destinations = destinations;
        self
    }

    /// The dialect of a database kind, or the ANSI default if the kind is unknown.
    pub fn dialect_for(&self, kind: &DriverKind) -> domain::Dialect {
        self.drivers
            .dialects()
            .into_iter()
            .find(|(k, _)| k == kind)
            .map_or(domain::Dialect::ANSI, |(_, d)| d)
    }

    /// Registered LLM providers and whether each needs an API key.
    pub fn llm_providers(&self) -> Vec<(ProviderId, bool)> {
        self.providers
            .ids()
            .into_iter()
            .map(|id| {
                let needs_key = self.providers.key_ref(&id).is_some();
                (id, needs_key)
            })
            .collect()
    }

    /// Database kinds that can currently be connected to.
    pub fn driver_kinds(&self) -> Vec<DriverKind> {
        self.drivers.kinds()
    }
}
