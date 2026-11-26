//! Messages exchanged between the UI thread and the background runtime.
//!
//! Errors cross the bridge as display strings: the UI only shows them.

use application::ports::HistoryEntry;
use domain::{ConnectionId, ConnectionProfile, QueryResult, SchemaTree};
use secrecy::SecretString;

/// An open database session, as known to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// An editor tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

/// UI to backend. `Debug` is safe: `SecretString` redacts itself.
#[derive(Debug)]
pub enum Command {
    LoadConnections,
    SaveConnection {
        profile: ConnectionProfile,
        password: Option<SecretString>,
    },
    DeleteConnection(ConnectionId),
    TestConnection {
        profile: ConnectionProfile,
        password: Option<SecretString>,
    },
    Connect(ConnectionId),
    Disconnect(SessionId),
    LoadSchema(SessionId),
    Execute {
        tab: TabId,
        session: SessionId,
        connection: ConnectionId,
        text: String,
    },
    Cancel(SessionId),
    LoadHistory(ConnectionId),
}

/// Backend to UI.
#[derive(Debug)]
pub enum Event {
    Connections(Result<Vec<ConnectionProfile>, String>),
    Saved(Result<ConnectionProfile, String>),
    Deleted(Result<ConnectionId, String>),
    TestFinished(Result<(), String>),
    Connected {
        connection: ConnectionId,
        result: Result<SessionId, String>,
    },
    Disconnected(SessionId),
    Schema {
        session: SessionId,
        result: Result<SchemaTree, String>,
    },
    QueryFinished {
        tab: TabId,
        result: Result<QueryResult, String>,
    },
    History {
        connection: ConnectionId,
        result: Result<Vec<HistoryEntry>, String>,
    },
}
