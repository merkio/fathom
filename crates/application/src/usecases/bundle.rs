//! Wiring helper: builds every use case from the ports once, so `app` stays a thin composition
//! root and the UI receives ready-made use cases.

use std::sync::Arc;

use domain::DriverKind;

use crate::ports::{Clock, ConnectionRepository, QueryHistoryRepository, SecretStore};
use crate::registry::DriverRegistry;
use crate::usecases::{
    BrowseSchema, CloseSession, DeleteConnection, ExecuteOptions, ExecuteQuery, ListConnections,
    OpenSession, QueryHistory, SaveConnection, TestConnection,
};

/// The adapters (as ports) the use cases need.
pub struct Ports {
    pub connections: Arc<dyn ConnectionRepository>,
    pub history: Arc<dyn QueryHistoryRepository>,
    pub secrets: Arc<dyn SecretStore>,
    pub clock: Arc<dyn Clock>,
    pub drivers: Arc<DriverRegistry>,
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
    pub query_history: QueryHistory,
    drivers: Arc<DriverRegistry>,
}

impl UseCases {
    pub fn new(ports: Ports, options: ExecuteOptions) -> Self {
        let Ports {
            connections,
            history,
            secrets,
            clock,
            drivers,
        } = ports;
        Self {
            list_connections: ListConnections::new(connections.clone()),
            save_connection: SaveConnection::new(connections.clone(), secrets.clone()),
            delete_connection: DeleteConnection::new(connections.clone(), secrets.clone()),
            test_connection: TestConnection::new(drivers.clone(), secrets.clone()),
            open_session: OpenSession::new(connections, drivers.clone(), secrets),
            close_session: CloseSession,
            browse_schema: BrowseSchema,
            execute_query: ExecuteQuery::new(history.clone(), clock, options),
            query_history: QueryHistory::new(history),
            drivers,
        }
    }

    /// Each connectable database kind with its dialect (keywords, quoting) for the editor.
    pub fn dialects(&self) -> Vec<(DriverKind, domain::Dialect)> {
        self.drivers.dialects()
    }

    /// Database kinds that can currently be connected to.
    pub fn driver_kinds(&self) -> Vec<DriverKind> {
        self.drivers.kinds()
    }
}
