//! Use cases. Each is a small struct holding the ports it needs.

pub mod ai;
pub mod assistant;
pub mod autocomplete;
pub mod bundle;
pub mod connections;
pub mod data;
pub mod explorer;
pub mod export;
pub mod inline;
pub mod ui_prefs;

pub use ai::{AiSettings, AiSettingsStore, ApiKeys, ListModels};
pub use assistant::{AssistReply, AssistRequest, AssistTask, Assistant, PromptPreview};
pub use autocomplete::CompleteSql;
pub use bundle::{ExportOptions, Ports, UseCases};
pub use connections::{
    CloseSession, DeleteConnection, ListConnections, OpenSession, SaveConnection, TestConnection,
};
pub use data::{BrowseSchema, ExecuteOptions, ExecuteQuery, QueryHistory};
pub use explorer::ExplorerVisibilityStore;
pub use export::{DEFAULT_MAX_EXPORT_ROWS, ExportResult, ExportSummary};
pub use inline::{
    InlineCompletion, InlineRequest, InlineSettings, InlineSettingsStore, SuggestInline,
};
pub use ui_prefs::{ThemePreference, UiPrefs, UiPrefsStore};
