//! Messages exchanged between the UI thread and the background runtime.
//!
//! Errors cross the bridge as display strings: the UI only shows them.

use std::path::PathBuf;

use application::export::ExportFormat;
use application::ports::{HistoryEntry, ModelInfo};
use application::usecases::{
    AiSettings, AssistReply, AssistTask, InlineCompletion, InlineSettings, PromptPreview, UiPrefs,
};
use domain::{
    ChatMessage, ConnectionId, ConnectionProfile, Dialect, ModelId, ProviderId, QueryResult,
    SchemaTree,
};
use secrecy::SecretString;

/// An open database session, as known to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// An editor tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

/// One AI request, from the click on "Ask" to its finished reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AiId(pub u64);

/// What the user asked the assistant to do, with owned data so it can cross threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiTask {
    Generate {
        question: String,
        current_sql: Option<String>,
    },
    Explain {
        sql: String,
    },
    Optimize {
        sql: String,
    },
    Fix {
        sql: String,
        error: String,
    },
    Chat {
        history: Vec<ChatMessage>,
        question: String,
    },
}

impl AiTask {
    pub fn as_task(&self) -> AssistTask<'_> {
        match self {
            AiTask::Generate {
                question,
                current_sql,
            } => AssistTask::Generate {
                question,
                current_sql: current_sql.as_deref(),
            },
            AiTask::Explain { sql } => AssistTask::Explain { sql },
            AiTask::Optimize { sql } => AssistTask::Optimize { sql },
            AiTask::Fix { sql, error } => AssistTask::Fix { sql, error },
            AiTask::Chat { history, question } => AssistTask::Chat { history, question },
        }
    }
}

/// Everything a request needs. The schema is metadata only; `sample_rows` is `Some` only when
/// the user opted in for this single request.
#[derive(Debug, Clone)]
pub struct AiAsk {
    pub id: AiId,
    pub provider: ProviderId,
    pub model: ModelId,
    pub dialect: Dialect,
    pub schema: SchemaTree,
    pub task: AiTask,
    pub sample_rows: Option<QueryResult>,
}

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
    /// Starts an AI request; the reply streams back as `AiText` events.
    AiAsk(Box<AiAsk>),
    /// Stops a running AI request; the partial reply is delivered as `AiFinished`.
    AiStop(AiId),
    /// Builds the exact prompt a request would send, without sending anything.
    AiPreview(Box<AiAsk>),
    LoadAiSettings,
    SaveAiSettings(AiSettings),
    SaveApiKey {
        provider: ProviderId,
        key: SecretString,
    },
    ClearApiKey(ProviderId),
    LoadApiKeyStatus(ProviderId),
    ListModels(ProviderId),
    /// Asks for an inline completion at `cursor` (byte offset) of `text`. The use case refuses,
    /// without contacting any provider, unless inline suggestions are enabled.
    InlineSuggest {
        id: AiId,
        session: SessionId,
        text: String,
        cursor: usize,
    },
    /// Cancels a pending or running inline request (it became stale).
    InlineCancel(AiId),
    LoadInlineSettings,
    SaveInlineSettings(InlineSettings),
    LoadUiPrefs,
    SaveUiPrefs(UiPrefs),
    /// Which databases of an instance the explorer shows.
    LoadVisibility(ConnectionId),
    /// Applies the user's reviewed row edits (never sent without their explicit action).
    ApplyChanges {
        tab: TabId,
        session: SessionId,
        changes: domain::TableChanges,
    },
    SaveVisibility {
        connection: ConnectionId,
        visible: Vec<String>,
    },
    /// Re-runs `text` (up to the export row cap) and writes the result to a new file.
    Export {
        tab: TabId,
        session: SessionId,
        connection: ConnectionId,
        text: String,
        format: ExportFormat,
        name: String,
    },
}

impl Command {
    /// Commands that write settings or keys. The backend runs them one after another in the
    /// order they were sent; every other command runs concurrently as its own task.
    pub fn is_ordered(&self) -> bool {
        matches!(
            self,
            Command::SaveAiSettings(_)
                | Command::SaveInlineSettings(_)
                | Command::SaveUiPrefs(_)
                | Command::SaveVisibility { .. }
                | Command::SaveApiKey { .. }
                | Command::ClearApiKey(_)
        )
    }
}

/// A finished export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDone {
    pub path: PathBuf,
    pub rows: usize,
    /// More rows existed than the cap allowed.
    pub truncated: bool,
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
    /// A piece of a streaming AI reply.
    AiText {
        id: AiId,
        text: String,
    },
    /// The AI request ended: the parsed reply, or an error (text already streamed stays).
    AiFinished {
        id: AiId,
        result: Result<AssistReply, String>,
    },
    AiPreview(PromptPreview),
    AiSettings(Result<AiSettings, String>),
    /// Whether a key is stored for the provider (`None`: it needs none).
    ApiKeyStatus {
        provider: ProviderId,
        set: Option<bool>,
    },
    Models {
        provider: ProviderId,
        result: Result<Vec<ModelInfo>, String>,
    },
    /// Plain confirmation or failure of a settings action.
    Notice(Result<String, String>),
    /// The inline completion as it grows.
    InlineText {
        id: AiId,
        completion: InlineCompletion,
    },
    /// The inline request ended. `error` is `None` for success and for "suggestions are off".
    InlineDone {
        id: AiId,
        error: Option<String>,
    },
    InlineSettings(Result<InlineSettings, String>),
    UiPrefs(Result<UiPrefs, String>),
    ChangesApplied {
        tab: TabId,
        result: Result<domain::ApplyOutcome, String>,
    },
    /// `None`: the user has not chosen yet (everything is shown).
    Visibility {
        connection: ConnectionId,
        result: Result<Option<Vec<String>>, String>,
    },
    Exported {
        tab: TabId,
        result: Result<ExportDone, String>,
    },
}
