//! Wiring helper: builds every use case from the ports once, so `app` stays a thin composition
//! root and the UI receives ready-made use cases.

use std::sync::Arc;

use domain::{DriverKind, ProviderId};

use crate::ports::{
    Clock, ConnectionRepository, QueryHistoryRepository, SecretStore, SettingsRepository,
};
use crate::registry::{DriverRegistry, LlmRegistry};
use crate::usecases::{
    AiSettingsStore, ApiKeys, Assistant, BrowseSchema, CloseSession, DeleteConnection,
    ExecuteOptions, ExecuteQuery, ListConnections, ListModels, OpenSession, QueryHistory,
    SaveConnection, SuggestInline, TestConnection,
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
}

/// Token budget for the schema part of a prompt.
const SCHEMA_TOKEN_BUDGET: usize = 3000;

pub struct UseCases {
    pub list_connections: ListConnections,
    pub save_connection: SaveConnection,
    pub delete_connection: DeleteConnection,
    pub test_connection: TestConnection,
    pub open_session: OpenSession,
    pub close_session: CloseSession,
    pub browse_schema: BrowseSchema,
    pub execute_query: ExecuteQuery,
    pub query_history: QueryHistory,
    pub assistant: Assistant,
    pub ai_settings: AiSettingsStore,
    pub api_keys: ApiKeys,
    pub list_models: ListModels,
    pub suggest_inline: SuggestInline,
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
        } = ports;
        Self {
            list_connections: ListConnections::new(connections.clone()),
            save_connection: SaveConnection::new(connections.clone(), secrets.clone()),
            delete_connection: DeleteConnection::new(connections.clone(), secrets.clone()),
            test_connection: TestConnection::new(drivers.clone(), secrets.clone()),
            open_session: OpenSession::new(connections, drivers.clone(), secrets.clone()),
            close_session: CloseSession,
            browse_schema: BrowseSchema,
            execute_query: ExecuteQuery::new(history.clone(), clock, options),
            query_history: QueryHistory::new(history),
            assistant: Assistant::new(providers.clone(), SCHEMA_TOKEN_BUDGET),
            suggest_inline: SuggestInline::new(providers.clone(), settings.clone()),
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
