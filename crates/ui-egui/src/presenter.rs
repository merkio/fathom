//! Presenter: all UI state and behaviour, free of egui. Views read the public fields and call
//! the methods; the app drains `take_commands` into the backend and feeds events to `handle`.

use std::collections::HashSet;

use application::ports::HistoryEntry;
use application::sql::{CompletionRequest, Suggestion, changes_schema};
use application::usecases::{AiSettings, CompleteSql};
use domain::{
    ConnectionId, ConnectionProfile, Dialect, DriverKind, ModelId, ProviderId, QueryResult,
    SchemaTree,
};

use crate::ai::{AiMode, AiRun, AiRunState, AiState, Followup};
use crate::bridge::{AiAsk, AiTask, Command, Event, SessionId, TabId};
use crate::form::ConnectionForm;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Info(String),
    Error(String),
}

#[derive(Debug)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Debug)]
pub struct OpenConnection {
    pub session: SessionId,
    pub connection: ConnectionId,
    pub name: String,
    pub dialect: Dialect,
    pub schema: Load<SchemaTree>,
}

#[derive(Debug)]
pub enum TabState {
    Idle,
    Running,
    Done(QueryResult),
    Failed(String),
}

/// Position of a tab's text and cursor, used to tell when the popup state is stale.
type Anchor = (usize, usize);

/// Autocomplete popup state of one editor tab.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompletionState {
    /// Highlighted row of the popup.
    pub selected: usize,
    selected_for: Option<Anchor>,
    /// Where the user pressed Esc; the popup stays closed until the text or cursor changes.
    dismissed: Option<Anchor>,
    /// Where the popup was requested explicitly (Ctrl+Space), so it may show with an empty prefix.
    requested: Option<Anchor>,
}

#[derive(Debug)]
pub struct QueryTab {
    pub id: TabId,
    pub session: SessionId,
    pub title: String,
    pub text: String,
    /// The statement last sent for execution (the text may be edited while it runs).
    executed: String,
    pub state: TabState,
    pub completion: CompletionState,
}

/// Which panel is shown on the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPanel {
    History,
    Ai,
}

/// At most this many result rows are ever shared with an AI provider, and only on request.
const MAX_SHARED_ROWS: usize = 5;

pub struct Presenter {
    pub connections: Vec<ConnectionProfile>,
    pub connecting: HashSet<ConnectionId>,
    pub open: Vec<OpenConnection>,
    pub tabs: Vec<QueryTab>,
    pub selected: Option<TabId>,
    pub history: Vec<HistoryEntry>,
    /// Texts of `history`, kept for completion ranking.
    history_texts: Vec<String>,
    pub form: Option<ConnectionForm>,
    pub testing: bool,
    pub status: Option<Status>,
    pub right_panel: RightPanel,
    pub ai: AiState,
    dialects: Vec<(DriverKind, Dialect)>,
    next_tab: u64,
    outbox: Vec<Command>,
}

impl Presenter {
    pub fn new(
        dialects: Vec<(DriverKind, Dialect)>,
        llm_providers: Vec<(ProviderId, bool)>,
    ) -> Self {
        let mut outbox = vec![Command::LoadConnections, Command::LoadAiSettings];
        outbox.extend(
            llm_providers
                .iter()
                .filter(|(_, needs_key)| *needs_key)
                .map(|(id, _)| Command::LoadApiKeyStatus(id.clone())),
        );
        Self {
            connections: Vec::new(),
            connecting: HashSet::new(),
            open: Vec::new(),
            tabs: Vec::new(),
            selected: None,
            history: Vec::new(),
            history_texts: Vec::new(),
            form: None,
            testing: false,
            status: None,
            right_panel: RightPanel::History,
            ai: AiState::new(llm_providers),
            dialects,
            next_tab: 1,
            outbox,
        }
    }

    pub fn driver_kinds(&self) -> Vec<DriverKind> {
        self.dialects.iter().map(|(k, _)| k.clone()).collect()
    }

    /// Commands produced since the last call.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.outbox)
    }

    // --- connection manager -------------------------------------------------------------

    pub fn new_connection_form(&mut self) {
        self.form = Some(ConnectionForm::new(self.dialects.first().map(|(k, _)| k)));
    }

    pub fn edit_connection(&mut self, id: ConnectionId) {
        if let Some(p) = self.connections.iter().find(|p| p.id == id) {
            self.form = Some(ConnectionForm::from_profile(p));
        }
    }

    pub fn cancel_form(&mut self) {
        self.form = None;
    }

    pub fn save_form(&mut self) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        match form.to_profile() {
            Ok(profile) => {
                let password = form.take_password();
                self.outbox
                    .push(Command::SaveConnection { profile, password });
            }
            Err(e) => self.status = Some(Status::Error(e)),
        }
    }

    pub fn test_form(&mut self) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        match form.to_profile() {
            Ok(profile) => {
                // Testing keeps the typed password in the form so it can still be saved.
                let password = (!form.password.is_empty())
                    .then(|| secrecy::SecretString::from(form.password.clone()));
                self.testing = true;
                self.outbox
                    .push(Command::TestConnection { profile, password });
            }
            Err(e) => self.status = Some(Status::Error(e)),
        }
    }

    pub fn delete_connection(&mut self, id: ConnectionId) {
        self.outbox.push(Command::DeleteConnection(id));
    }

    pub fn connect(&mut self, id: ConnectionId) {
        if self.connecting.insert(id) {
            self.outbox.push(Command::Connect(id));
        }
    }

    pub fn disconnect(&mut self, session: SessionId) {
        self.outbox.push(Command::Disconnect(session));
    }

    pub fn refresh_schema(&mut self, session: SessionId) {
        if let Some(open) = self.open.iter_mut().find(|o| o.session == session) {
            open.schema = Load::Loading;
            self.outbox.push(Command::LoadSchema(session));
        }
    }

    // --- editor tabs ----------------------------------------------------------------------

    pub fn new_tab(&mut self, session: SessionId) -> Option<TabId> {
        let name = self
            .open
            .iter()
            .find(|o| o.session == session)?
            .name
            .clone();
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        let number = self.tabs.iter().filter(|t| t.session == session).count() + 1;
        self.tabs.push(QueryTab {
            id,
            session,
            title: format!("{name} #{number}"),
            text: String::new(),
            executed: String::new(),
            state: TabState::Idle,
            completion: CompletionState::default(),
        });
        self.select_tab(id);
        Some(id)
    }

    pub fn select_tab(&mut self, id: TabId) {
        if self.selected == Some(id) || !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        self.selected = Some(id);
        self.request_history();
    }

    pub fn close_tab(&mut self, id: TabId) {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let tab = self.tabs.remove(pos);
        if matches!(tab.state, TabState::Running) {
            self.outbox.push(Command::Cancel(tab.session));
        }
        if self.selected == Some(id) {
            self.selected = None;
            if let Some(next) = self.tabs.get(pos.min(self.tabs.len().saturating_sub(1))) {
                let next = next.id;
                self.select_tab(next);
            } else {
                self.history.clear();
                self.history_texts.clear();
            }
        }
    }

    pub fn selected_tab(&self) -> Option<&QueryTab> {
        self.tabs.iter().find(|t| Some(t.id) == self.selected)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut QueryTab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn run(&mut self, id: TabId) {
        let Some(connection) = self.tab_connection(id) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if matches!(tab.state, TabState::Running) || tab.text.trim().is_empty() {
            return;
        }
        tab.state = TabState::Running;
        tab.executed = tab.text.clone();
        self.outbox.push(Command::Execute {
            tab: id,
            session: tab.session,
            connection,
            text: tab.text.clone(),
        });
    }

    pub fn cancel(&mut self, id: TabId) {
        if let Some(tab) = self.tabs.iter().find(|t| t.id == id)
            && matches!(tab.state, TabState::Running)
        {
            self.outbox.push(Command::Cancel(tab.session));
        }
    }

    /// Puts a history entry's SQL into the selected tab.
    pub fn use_history(&mut self, index: usize) {
        let Some(text) = self.history.get(index).map(|e| e.text.clone()) else {
            return;
        };
        if let Some(id) = self.selected
            && let Some(tab) = self.tab_mut(id)
        {
            tab.text = text;
        }
    }

    fn tab_connection(&self, id: TabId) -> Option<ConnectionId> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        self.open
            .iter()
            .find(|o| o.session == tab.session)
            .map(|o| o.connection)
    }

    fn request_history(&mut self) {
        if let Some(c) = self.selected.and_then(|id| self.tab_connection(id)) {
            self.outbox.push(Command::LoadHistory(c));
        }
    }

    // --- AI assistant ---------------------------------------------------------------------

    pub fn show_ai_panel(&mut self) {
        self.right_panel = RightPanel::Ai;
    }

    /// Whether the connection runs in read-only mode.
    pub fn is_read_only(&self, connection: ConnectionId) -> bool {
        self.connections
            .iter()
            .find(|p| p.id == connection)
            .is_some_and(ConnectionProfile::read_only)
    }

    pub fn ai_select_provider(&mut self, id: ProviderId) {
        if self.ai.select_provider(id.clone()) {
            self.outbox.push(Command::ListModels(id.clone()));
            self.outbox.push(Command::LoadApiKeyStatus(id));
            self.outbox
                .push(Command::SaveAiSettings(self.ai.settings.clone()));
        }
    }

    pub fn ai_select_model(&mut self, id: ModelId) {
        self.ai.select_model(id);
        self.outbox
            .push(Command::SaveAiSettings(self.ai.settings.clone()));
    }

    pub fn ai_refresh_models(&mut self) {
        if let Some(provider) = self.ai.settings.provider.clone() {
            self.outbox.push(Command::ListModels(provider));
        }
    }

    /// Sends the typed API key to the secret store and forgets it.
    pub fn ai_save_key(&mut self) {
        let Some(provider) = self.ai.settings.provider.clone() else {
            return;
        };
        if self.ai.api_key_input.trim().is_empty() {
            return;
        }
        let key = secrecy::SecretString::from(
            std::mem::take(&mut self.ai.api_key_input).trim().to_owned(),
        );
        self.outbox.push(Command::SaveApiKey { provider, key });
    }

    pub fn ai_clear_key(&mut self) {
        if let Some(provider) = self.ai.settings.provider.clone() {
            self.outbox.push(Command::ClearApiKey(provider));
        }
    }

    /// Builds the request for the selected tab. Everything that would leave the machine is
    /// assembled here: the schema (metadata), the user's text, and result rows only if the user
    /// ticked the box for this request.
    fn build_ask(&mut self, mode: AiMode) -> Result<AiAsk, String> {
        let (provider, model) = self.ai.ready()?;
        let id = self.selected.ok_or("Open a query tab first.")?;
        let tab = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .ok_or("Open a query tab first.")?;
        let open = self
            .open
            .iter()
            .find(|o| o.session == tab.session)
            .ok_or("The connection of this tab is closed.")?;
        let Load::Ready(schema) = &open.schema else {
            return Err("The schema is still loading; try again in a moment.".into());
        };
        let editor = tab.text.trim();
        let input = self.ai.input.trim();

        let task = match mode {
            AiMode::Generate => {
                if input.is_empty() {
                    return Err("Describe the query you want first.".into());
                }
                AiTask::Generate {
                    question: input.to_owned(),
                    current_sql: (!editor.is_empty()).then(|| editor.to_owned()),
                }
            }
            AiMode::Explain | AiMode::Optimize => {
                if editor.is_empty() {
                    return Err("Write or paste some SQL in the editor first.".into());
                }
                if mode == AiMode::Explain {
                    AiTask::Explain {
                        sql: editor.to_owned(),
                    }
                } else {
                    AiTask::Optimize {
                        sql: editor.to_owned(),
                    }
                }
            }
            AiMode::Fix => {
                let TabState::Failed(error) = &tab.state else {
                    return Err("Nothing to fix: the last run of this tab did not fail.".into());
                };
                AiTask::Fix {
                    sql: tab.executed.clone(),
                    error: error.clone(),
                }
            }
            AiMode::Chat => {
                if input.is_empty() {
                    return Err("Type a message first.".into());
                }
                AiTask::Chat {
                    history: self.ai.chat.clone(),
                    question: input.to_owned(),
                }
            }
        };

        // Row data is shared only on explicit, per-request opt-in, and only a few rows.
        let sample_rows = match (&tab.state, self.ai.include_rows) {
            (TabState::Done(result), true) if !result.columns.is_empty() => {
                let mut sample = result.clone();
                sample.rows.truncate(MAX_SHARED_ROWS);
                Some(sample)
            }
            _ => None,
        };

        Ok(AiAsk {
            id: self.ai.next_id(),
            provider,
            model,
            dialect: open.dialect,
            schema: schema.clone(),
            task,
            sample_rows,
        })
    }

    /// Starts an AI request. Nothing is executed: the reply only ever lands in the AI panel,
    /// and SQL reaches the editor when the user clicks "Insert".
    pub fn ai_ask(&mut self, mode: AiMode) {
        if self.ai.is_streaming() {
            return;
        }
        match self.build_ask(mode) {
            Ok(ask) => {
                let prompt = match &ask.task {
                    AiTask::Generate { question, .. } | AiTask::Chat { question, .. } => {
                        question.clone()
                    }
                    AiTask::Explain { .. } => "Explain the SQL in the editor".to_owned(),
                    AiTask::Optimize { .. } => "Optimize the SQL in the editor".to_owned(),
                    AiTask::Fix { .. } => "Fix the failed query".to_owned(),
                };
                self.ai.begin(AiRun {
                    id: ask.id,
                    mode,
                    prompt,
                    text: String::new(),
                    state: AiRunState::Streaming,
                });
                if mode.takes_input() {
                    self.ai.input.clear();
                }
                // Sharing rows never carries over to the next request.
                self.ai.include_rows = false;
                self.right_panel = RightPanel::Ai;
                self.outbox.push(Command::AiAsk(Box::new(ask)));
            }
            Err(message) => self.status = Some(Status::Error(message)),
        }
    }

    /// Asks for the exact prompt a request would send, without sending it.
    pub fn ai_preview(&mut self, mode: AiMode) {
        match self.build_ask(mode) {
            Ok(ask) => self.outbox.push(Command::AiPreview(Box::new(ask))),
            Err(message) => self.status = Some(Status::Error(message)),
        }
    }

    pub fn ai_stop(&mut self) {
        if let Some(AiRun {
            id,
            state: AiRunState::Streaming,
            ..
        }) = &self.ai.run
        {
            self.outbox.push(Command::AiStop(*id));
        }
    }

    /// One-click fix from a failed query.
    pub fn ai_fix_last_error(&mut self) {
        self.ai.mode = AiMode::Fix;
        self.ai_ask(AiMode::Fix);
    }

    /// Puts the reply's SQL into the selected editor tab. It is never run automatically.
    pub fn ai_insert_sql(&mut self, replace: bool) {
        let Some(AiRun {
            state: AiRunState::Done(reply),
            ..
        }) = &self.ai.run
        else {
            return;
        };
        let Some(sql) = reply.sql.clone() else { return };
        let Some(id) = self.selected else { return };
        if let Some(tab) = self.tab_mut(id) {
            if replace || tab.text.trim().is_empty() {
                tab.text = sql;
            } else {
                tab.text = format!("{}\n\n{sql}", tab.text.trim_end());
            }
            tab.completion = CompletionState::default();
        }
    }

    pub fn ai_current_settings(&self) -> &AiSettings {
        &self.ai.settings
    }

    // --- autocomplete ---------------------------------------------------------------------

    /// Local completions for the tab's text at a byte offset. Empty until the schema is loaded.
    /// Pure and synchronous, so the editor can call it on every keystroke.
    pub fn suggestions(&self, id: TabId, cursor: usize) -> Vec<Suggestion> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return Vec::new();
        };
        let Some(open) = self.open.iter().find(|o| o.session == tab.session) else {
            return Vec::new();
        };
        let Load::Ready(schema) = &open.schema else {
            return Vec::new();
        };
        let recent: &[String] = if self.selected == Some(id) {
            &self.history_texts
        } else {
            &[]
        };
        CompleteSql.execute(&CompletionRequest {
            sql: &tab.text,
            cursor,
            schema,
            dialect: &open.dialect,
            recent_queries: recent,
        })
    }

    /// The suggestions the popup should show right now (possibly none). Applies the display
    /// rules: only while typing a name (or after `.`, or when requested with Ctrl+Space), not
    /// after Esc, and not when the only candidate is exactly what is typed already.
    pub fn popup(&mut self, id: TabId, cursor: usize) -> Vec<Suggestion> {
        let suggestions = self.suggestions(id, cursor);
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
            return Vec::new();
        };
        let anchor = (cursor, tab.text.len());
        if tab.completion.selected_for != Some(anchor) {
            tab.completion.selected = 0;
            tab.completion.selected_for = Some(anchor);
        }
        let Some(first) = suggestions.first() else {
            return Vec::new();
        };
        let typing = first.range.start < first.range.end
            || tab
                .text
                .get(..cursor)
                .is_some_and(|before| before.ends_with('.'));
        let requested = tab.completion.requested == Some(anchor);
        let exact =
            suggestions.len() == 1 && tab.text.get(first.range.clone()) == Some(&first.text);
        if tab.completion.dismissed == Some(anchor) || !(typing || requested) || exact {
            return Vec::new();
        }
        tab.completion.selected = tab.completion.selected.min(suggestions.len() - 1);
        suggestions
    }

    /// Moves the highlighted popup row, wrapping around.
    pub fn move_selection(&mut self, id: TabId, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        if let Some(tab) = self.tab_mut(id) {
            let current = tab.completion.selected.min(len - 1) as isize;
            tab.completion.selected = (current + delta).rem_euclid(len as isize) as usize;
        }
    }

    /// Esc: close the popup until the text or cursor changes.
    pub fn dismiss_popup(&mut self, id: TabId, cursor: usize) {
        if let Some(tab) = self.tab_mut(id) {
            tab.completion.dismissed = Some((cursor, tab.text.len()));
        }
    }

    /// Ctrl+Space: show the popup even though nothing has been typed yet.
    pub fn request_popup(&mut self, id: TabId, cursor: usize) {
        if let Some(tab) = self.tab_mut(id) {
            tab.completion.dismissed = None;
            tab.completion.requested = Some((cursor, tab.text.len()));
        }
    }

    /// Applies a suggestion to the tab's text and returns the new cursor (byte offset).
    pub fn accept_suggestion(&mut self, id: TabId, suggestion: &Suggestion) -> Option<usize> {
        let tab = self.tab_mut(id)?;
        let r = suggestion.range.clone();
        let valid = r.start <= r.end
            && r.end <= tab.text.len()
            && tab.text.is_char_boundary(r.start)
            && tab.text.is_char_boundary(r.end);
        if !valid {
            return None;
        }
        tab.text.replace_range(r.clone(), &suggestion.text);
        tab.completion = CompletionState::default();
        Some(r.start + suggestion.text.len())
    }

    /// The dialect of the connection a tab belongs to (for highlighting).
    pub fn tab_dialect(&self, id: TabId) -> Dialect {
        self.tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| self.open.iter().find(|o| o.session == t.session))
            .map_or(Dialect::ANSI, |o| o.dialect)
    }

    // --- events ---------------------------------------------------------------------------

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Notice(Ok(message)) => self.status = Some(Status::Info(message)),
            Event::Notice(Err(message)) => self.error(message),
            e @ (Event::AiText { .. }
            | Event::AiFinished { .. }
            | Event::AiPreview(_)
            | Event::AiSettings(_)
            | Event::ApiKeyStatus { .. }
            | Event::Models { .. }) => {
                if let Event::AiFinished {
                    result: Err(message),
                    ..
                } = &e
                {
                    self.status = Some(Status::Error(format!("AI request failed: {message}")));
                }
                for follow in self.ai.handle(e) {
                    match follow {
                        Followup::ListModels(provider) => {
                            self.outbox.push(Command::ListModels(provider))
                        }
                    }
                }
            }
            Event::Connections(Ok(list)) => self.connections = list,
            Event::Connections(Err(e)) => self.error(format!("Could not load connections: {e}")),
            Event::Saved(Ok(profile)) => {
                self.form = None;
                self.status = Some(Status::Info(format!("Saved `{}`", profile.name)));
                self.outbox.push(Command::LoadConnections);
            }
            Event::Saved(Err(e)) => self.error(format!("Could not save: {e}")),
            Event::Deleted(Ok(_)) => self.outbox.push(Command::LoadConnections),
            Event::Deleted(Err(e)) => self.error(format!("Could not delete: {e}")),
            Event::TestFinished(result) => {
                self.testing = false;
                self.status = Some(match result {
                    Ok(()) => Status::Info("Connection successful".into()),
                    Err(e) => Status::Error(format!("Connection failed: {e}")),
                });
            }
            Event::Connected { connection, result } => {
                self.connecting.remove(&connection);
                match result {
                    Ok(session) => self.on_connected(connection, session),
                    Err(e) => self.error(format!("Could not connect: {e}")),
                }
            }
            Event::Disconnected(session) => self.on_disconnected(session),
            Event::Schema { session, result } => {
                if let Some(open) = self.open.iter_mut().find(|o| o.session == session) {
                    open.schema = match result {
                        Ok(tree) => Load::Ready(tree),
                        Err(e) => Load::Failed(e),
                    };
                }
            }
            Event::QueryFinished { tab, result } => self.on_query_finished(tab, result),
            Event::History { connection, result } => {
                let current = self.selected.and_then(|id| self.tab_connection(id));
                if current == Some(connection) {
                    match result {
                        Ok(entries) => {
                            self.history_texts = entries.iter().map(|e| e.text.clone()).collect();
                            self.history = entries;
                        }
                        Err(e) => self.error(format!("Could not load history: {e}")),
                    }
                }
            }
        }
    }

    fn error(&mut self, message: String) {
        self.status = Some(Status::Error(message));
    }

    fn on_connected(&mut self, connection: ConnectionId, session: SessionId) {
        let name = self
            .connections
            .iter()
            .find(|p| p.id == connection)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "connection".into());
        let dialect = self
            .connections
            .iter()
            .find(|p| p.id == connection)
            .and_then(|p| self.dialects.iter().find(|(k, _)| *k == p.driver_kind))
            .map_or(Dialect::ANSI, |(_, d)| *d);
        self.open.push(OpenConnection {
            session,
            connection,
            name: name.clone(),
            dialect,
            schema: Load::Loading,
        });
        self.outbox.push(Command::LoadSchema(session));
        self.new_tab(session);
        self.status = Some(Status::Info(format!("Connected to `{name}`")));
    }

    fn on_disconnected(&mut self, session: SessionId) {
        self.open.retain(|o| o.session != session);
        let closing: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.session == session)
            .map(|t| t.id)
            .collect();
        for id in closing {
            // The session is already gone; skip the cancel that `close_tab` would send.
            if let Some(t) = self.tab_mut(id) {
                t.state = TabState::Idle;
            }
            self.close_tab(id);
        }
    }

    fn on_query_finished(&mut self, id: TabId, result: Result<QueryResult, String>) {
        let Some(tab) = self.tab_mut(id) else { return };
        let session = tab.session;
        let executed = tab.executed.clone();
        let stale_schema = result.is_ok()
            && self
                .open
                .iter()
                .find(|o| o.session == session)
                .is_some_and(|o| changes_schema(&executed, &o.dialect));
        let Some(tab) = self.tab_mut(id) else { return };
        tab.state = match result {
            Ok(r) => TabState::Done(r),
            Err(e) => TabState::Failed(e),
        };
        if stale_schema {
            // The statement changed the schema, so the cached metadata (and with it the
            // autocomplete data) is out of date.
            self.refresh_schema(session);
        }
        if let Some(connection) = self.tab_connection(id)
            && self.selected.and_then(|s| self.tab_connection(s)) == Some(connection)
        {
            self.outbox.push(Command::LoadHistory(connection));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn presenter() -> Presenter {
        let mut p = Presenter::new(vec![(DriverKind::new("postgres"), Dialect::ANSI)], vec![]);
        assert!(matches!(
            p.take_commands().as_slice(),
            [Command::LoadConnections, Command::LoadAiSettings]
        ));
        p
    }

    fn profile() -> ConnectionProfile {
        ConnectionProfile::new("main", DriverKind::new("postgres"), "localhost")
    }

    /// Presenter with one connection opened and one tab selected.
    fn connected() -> (Presenter, ConnectionId, SessionId, TabId) {
        let mut p = presenter();
        let profile = profile();
        let id = profile.id;
        p.handle(Event::Connections(Ok(vec![profile])));
        p.connect(id);
        p.handle(Event::Connected {
            connection: id,
            result: Ok(SessionId(1)),
        });
        let tab = p.selected.expect("tab opened on connect");
        p.take_commands();
        (p, id, SessionId(1), tab)
    }

    fn result(rows: usize) -> QueryResult {
        QueryResult {
            rows: vec![vec![domain::Value::Int(1)]; rows],
            elapsed: Duration::ZERO,
            ..QueryResult::empty()
        }
    }

    #[test]
    fn connecting_opens_a_tab_and_loads_schema() {
        let mut p = presenter();
        let profile = profile();
        let id = profile.id;
        p.handle(Event::Connections(Ok(vec![profile])));
        p.connect(id);
        p.connect(id); // double click while connecting is ignored
        assert!(matches!(p.take_commands().as_slice(), [Command::Connect(c)] if *c == id));

        p.handle(Event::Connected {
            connection: id,
            result: Ok(SessionId(7)),
        });
        assert!(p.connecting.is_empty());
        assert_eq!(p.open.len(), 1);
        assert_eq!(p.tabs.len(), 1);
        assert_eq!(p.selected_tab().map(|t| t.title.as_str()), Some("main #1"));
        let cmds = p.take_commands();
        assert!(matches!(cmds[0], Command::LoadSchema(SessionId(7))));
        assert!(matches!(cmds[1], Command::LoadHistory(c) if c == id));
    }

    #[test]
    fn failed_connect_reports_error_and_allows_retry() {
        let mut p = presenter();
        let id = ConnectionId::new();
        p.connect(id);
        p.handle(Event::Connected {
            connection: id,
            result: Err("refused".into()),
        });
        assert!(matches!(&p.status, Some(Status::Error(m)) if m.contains("refused")));
        p.take_commands();
        p.connect(id);
        assert_eq!(p.take_commands().len(), 1);
    }

    #[test]
    fn run_sends_execute_once_and_tracks_state() {
        let (mut p, conn, session, tab) = connected();
        p.run(tab); // empty text: nothing happens
        assert!(p.take_commands().is_empty());

        p.tab_mut(tab).expect("tab").text = "select 1".into();
        p.run(tab);
        p.run(tab); // already running
        let cmds = p.take_commands();
        assert_eq!(cmds.len(), 1);
        assert!(
            matches!(&cmds[0], Command::Execute { session: s, connection: c, text, .. }
            if *s == session && *c == conn && text == "select 1")
        );
        assert!(matches!(
            p.selected_tab().map(|t| &t.state),
            Some(TabState::Running)
        ));

        p.handle(Event::QueryFinished {
            tab,
            result: Ok(result(2)),
        });
        assert!(
            matches!(p.selected_tab().map(|t| &t.state), Some(TabState::Done(r)) if r.rows.len() == 2)
        );
        assert!(matches!(p.take_commands().as_slice(), [Command::LoadHistory(c)] if *c == conn));
    }

    #[test]
    fn query_failure_is_shown_in_the_tab() {
        let (mut p, _, _, tab) = connected();
        p.tab_mut(tab).expect("tab").text = "bad".into();
        p.run(tab);
        p.handle(Event::QueryFinished {
            tab,
            result: Err("syntax error".into()),
        });
        assert!(
            matches!(p.selected_tab().map(|t| &t.state), Some(TabState::Failed(m)) if m == "syntax error")
        );
    }

    #[test]
    fn cancel_only_while_running() {
        let (mut p, _, session, tab) = connected();
        p.cancel(tab);
        assert!(p.take_commands().is_empty());
        p.tab_mut(tab).expect("tab").text = "select pg_sleep(60)".into();
        p.run(tab);
        p.take_commands();
        p.cancel(tab);
        assert!(matches!(p.take_commands().as_slice(), [Command::Cancel(s)] if *s == session));
    }

    #[test]
    fn closing_a_running_tab_cancels_it() {
        let (mut p, _, session, tab) = connected();
        p.tab_mut(tab).expect("tab").text = "select 1".into();
        p.run(tab);
        p.take_commands();
        p.close_tab(tab);
        assert!(p.tabs.is_empty() && p.selected.is_none());
        assert!(matches!(p.take_commands().as_slice(), [Command::Cancel(s)] if *s == session));
    }

    #[test]
    fn disconnect_removes_session_and_its_tabs_without_cancelling() {
        let (mut p, _, session, _) = connected();
        p.new_tab(session);
        p.take_commands();
        p.handle(Event::Disconnected(session));
        assert!(p.open.is_empty() && p.tabs.is_empty() && p.selected.is_none());
        assert!(
            !p.take_commands()
                .iter()
                .any(|c| matches!(c, Command::Cancel(_)))
        );
    }

    #[test]
    fn history_applies_only_to_the_selected_tabs_connection() {
        let (mut p, conn, _, _) = connected();
        let entry = |t: &str| HistoryEntry {
            connection_id: conn,
            text: t.into(),
            executed_at: std::time::UNIX_EPOCH,
            elapsed: Duration::ZERO,
            succeeded: true,
        };
        p.handle(Event::History {
            connection: ConnectionId::new(),
            result: Ok(vec![entry("other")]),
        });
        assert!(p.history.is_empty());
        p.handle(Event::History {
            connection: conn,
            result: Ok(vec![entry("select 42")]),
        });
        p.use_history(0);
        assert_eq!(p.selected_tab().map(|t| t.text.as_str()), Some("select 42"));
    }

    #[test]
    fn selecting_another_tab_reloads_history() {
        let (mut p, conn, session, first) = connected();
        let second = p.new_tab(session).expect("tab");
        p.take_commands();
        p.select_tab(first);
        assert!(matches!(p.take_commands().as_slice(), [Command::LoadHistory(c)] if *c == conn));
        p.select_tab(first); // no change, no command
        assert!(p.take_commands().is_empty());
        p.select_tab(second);
        assert_eq!(p.take_commands().len(), 1);
    }

    #[test]
    fn saving_the_form_sends_profile_and_password_once() {
        let mut p = presenter();
        p.new_connection_form();
        let form = p.form.as_mut().expect("form");
        form.name = "db".into();
        form.password = "pw".into();
        p.save_form();
        let cmds = p.take_commands();
        assert!(matches!(
            &cmds[0],
            Command::SaveConnection {
                password: Some(_),
                ..
            }
        ));
        assert!(p.form.as_ref().is_some_and(|f| f.password.is_empty()));

        p.handle(Event::Saved(Ok(profile())));
        assert!(p.form.is_none());
        assert!(matches!(
            p.take_commands().as_slice(),
            [Command::LoadConnections]
        ));
    }

    #[test]
    fn invalid_form_does_not_send_anything() {
        let mut p = presenter();
        p.new_connection_form();
        p.form.as_mut().expect("form").port = "abc".into();
        p.save_form();
        assert!(p.take_commands().is_empty());
        assert!(matches!(p.status, Some(Status::Error(_))));
    }

    #[test]
    fn testing_keeps_the_typed_password_in_the_form() {
        let mut p = presenter();
        p.new_connection_form();
        p.form.as_mut().expect("form").password = "pw".into();
        p.test_form();
        assert!(p.testing);
        assert!(matches!(
            &p.take_commands()[0],
            Command::TestConnection {
                password: Some(_),
                ..
            }
        ));
        assert_eq!(p.form.as_ref().map(|f| f.password.as_str()), Some("pw"));
        p.handle(Event::TestFinished(Err("bad password".into())));
        assert!(!p.testing);
        assert!(matches!(&p.status, Some(Status::Error(m)) if m.contains("bad password")));
    }

    fn schema() -> SchemaTree {
        use domain::{Catalog, Column, DataType, Schema, Table, TableKind};
        let mut customers = Table::new("customers", TableKind::Table);
        customers.columns.push(Column {
            name: "email".into(),
            data_type: DataType::new("text"),
            nullable: true,
            default: None,
        });
        SchemaTree {
            catalogs: vec![Catalog {
                name: "db".into(),
                schemas: vec![Schema {
                    name: "public".into(),
                    tables: vec![customers],
                }],
            }],
        }
    }

    fn with_schema() -> (Presenter, SessionId, TabId) {
        let (mut p, _, session, tab) = connected();
        p.handle(Event::Schema {
            session,
            result: Ok(schema()),
        });
        (p, session, tab)
    }

    #[test]
    fn suggestions_need_the_schema_and_use_it_offline() {
        let (mut p, _, session, tab) = connected();
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM cu".into();
        assert!(p.suggestions(tab, 16).is_empty(), "schema still loading");

        p.handle(Event::Schema {
            session,
            result: Ok(schema()),
        });
        let s = p.suggestions(tab, 16);
        assert_eq!(
            s.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            ["customers"]
        );
        assert!(
            p.take_commands().is_empty(),
            "completion must not send commands"
        );
    }

    #[test]
    fn schema_failure_means_no_suggestions() {
        let (mut p, _, session, tab) = connected();
        p.handle(Event::Schema {
            session,
            result: Err("denied".into()),
        });
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM ".into();
        assert!(p.suggestions(tab, 14).is_empty());
    }

    #[test]
    fn accepting_a_suggestion_replaces_the_prefix_and_returns_the_cursor() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM cu WHERE 1=1".into();
        let s = p.suggestions(tab, 16).remove(0);
        let cursor = p.accept_suggestion(tab, &s);
        assert_eq!(
            p.selected_tab().map(|t| t.text.as_str()),
            Some("SELECT * FROM customers WHERE 1=1")
        );
        assert_eq!(cursor, Some("SELECT * FROM customers".len()));
    }

    #[test]
    fn stale_suggestions_are_rejected_not_applied() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "é".into();
        let bad = Suggestion {
            range: 1..2, // inside the two-byte character
            text: "x".into(),
            kind: application::sql::SuggestionKind::Keyword,
            source: application::sql::SuggestionSource::Local,
            detail: None,
        };
        assert_eq!(p.accept_suggestion(tab, &bad), None);
        assert_eq!(p.selected_tab().map(|t| t.text.as_str()), Some("é"));
    }

    #[test]
    fn ddl_refreshes_the_schema_but_selects_do_not() {
        let (mut p, session, tab) = with_schema();
        for (sql, refreshes) in [("select 1", false), ("CREATE TABLE t (id int)", true)] {
            p.tab_mut(tab).expect("tab").text = sql.into();
            p.run(tab);
            p.take_commands();
            p.handle(Event::QueryFinished {
                tab,
                result: Ok(result(0)),
            });
            let cmds = p.take_commands();
            let refreshed = cmds
                .iter()
                .any(|c| matches!(c, Command::LoadSchema(s) if *s == session));
            assert_eq!(refreshed, refreshes, "{sql}");
        }
        assert!(
            matches!(p.open[0].schema, Load::Loading),
            "schema marked stale while reloading"
        );
    }

    #[test]
    fn failed_ddl_does_not_refresh() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "DROP TABLE nope".into();
        p.run(tab);
        p.take_commands();
        p.handle(Event::QueryFinished {
            tab,
            result: Err("no such table".into()),
        });
        assert!(
            !p.take_commands()
                .iter()
                .any(|c| matches!(c, Command::LoadSchema(_)))
        );
    }

    #[test]
    fn editing_while_a_ddl_runs_does_not_change_what_is_checked() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "DROP TABLE t".into();
        p.run(tab);
        p.take_commands();
        p.tab_mut(tab).expect("tab").text = "select 1".into(); // edited after sending
        p.handle(Event::QueryFinished {
            tab,
            result: Ok(result(0)),
        });
        assert!(
            p.take_commands()
                .iter()
                .any(|c| matches!(c, Command::LoadSchema(_)))
        );
    }

    #[test]
    fn dialect_follows_the_connections_driver() {
        let mut dialect = Dialect::ANSI;
        dialect.name = "special";
        let mut p = Presenter::new(vec![(DriverKind::new("postgres"), dialect)], vec![]);
        let profile = profile();
        let id = profile.id;
        p.handle(Event::Connections(Ok(vec![profile])));
        p.connect(id);
        p.handle(Event::Connected {
            connection: id,
            result: Ok(SessionId(1)),
        });
        let tab = p.selected.expect("tab");
        assert_eq!(p.tab_dialect(tab).name, "special");
        assert_eq!(p.driver_kinds(), vec![DriverKind::new("postgres")]);
    }

    #[test]
    fn popup_shows_while_typing_and_hides_after_escape_until_something_changes() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM cu".into();
        assert_eq!(p.popup(tab, 16).len(), 1);
        p.dismiss_popup(tab, 16);
        assert!(p.popup(tab, 16).is_empty(), "dismissed");
        assert!(p.popup(tab, 15).len() == 1, "moving the cursor reopens it");
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM cuz".into();
        assert!(p.popup(tab, 17).is_empty(), "no table starts with cuz");
    }

    #[test]
    fn popup_needs_a_typed_prefix_unless_requested() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM ".into();
        assert!(p.popup(tab, 14).is_empty(), "nothing typed yet");
        p.request_popup(tab, 14);
        assert_eq!(p.popup(tab, 14).len(), 1, "Ctrl+Space shows it");
        assert!(
            p.popup(tab, 13).is_empty(),
            "request only holds at its cursor"
        );
    }

    #[test]
    fn popup_opens_after_a_dot_and_hides_for_an_exact_single_match() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "SELECT customers. FROM customers".into();
        assert_eq!(p.popup(tab, 17).len(), 1);
        p.tab_mut(tab).expect("tab").text = "SELECT * FROM customers".into();
        assert!(p.popup(tab, 23).is_empty(), "already fully typed");
    }

    #[test]
    fn selection_wraps_resets_on_change_and_stays_in_range() {
        let (mut p, _, tab) = with_schema();
        p.tab_mut(tab).expect("tab").text = "SELECT  FROM customers".into();
        p.request_popup(tab, 7);
        let n = p.popup(tab, 7).len();
        assert!(n >= 2, "columns and functions");
        p.move_selection(tab, -1, n);
        assert_eq!(p.selected_tab().map(|t| t.completion.selected), Some(n - 1));
        p.move_selection(tab, 1, n);
        assert_eq!(p.selected_tab().map(|t| t.completion.selected), Some(0));
        p.move_selection(tab, 1, n);
        p.tab_mut(tab).expect("tab").text.push(' ');
        p.popup(tab, 7);
        assert_eq!(
            p.selected_tab().map(|t| t.completion.selected),
            Some(0),
            "reset on change"
        );
        p.move_selection(tab, 1, 0); // empty list: no panic
    }

    // ---- AI assistant -----------------------------------------------------------------

    use crate::ai::{AiMode, AiRunState, ModelsState};
    use application::ports::ModelInfo;
    use application::usecases::AssistReply;

    fn fake_providers() -> Vec<(ProviderId, bool)> {
        vec![
            (ProviderId::new("local"), false),
            (ProviderId::new("cloud"), true),
        ]
    }

    /// Connected, schema loaded, provider `local` and model `m` chosen.
    fn ai_ready() -> (Presenter, TabId) {
        let mut p = Presenter::new(
            vec![(DriverKind::new("postgres"), Dialect::ANSI)],
            fake_providers(),
        );
        p.take_commands();
        let profile = profile();
        let id = profile.id;
        p.handle(Event::Connections(Ok(vec![profile])));
        p.connect(id);
        p.handle(Event::Connected {
            connection: id,
            result: Ok(SessionId(1)),
        });
        p.handle(Event::Schema {
            session: SessionId(1),
            result: Ok(schema()),
        });
        p.handle(Event::AiSettings(Ok(AiSettings {
            provider: Some(ProviderId::new("local")),
            model: Some(ModelId::new("m")),
        })));
        let tab = p.selected.expect("tab");
        p.take_commands();
        (p, tab)
    }

    fn only_ask(p: &mut Presenter) -> AiAsk {
        let mut cmds = p.take_commands();
        assert_eq!(cmds.len(), 1, "{cmds:?}");
        match cmds.remove(0) {
            Command::AiAsk(ask) => *ask,
            other => panic!("expected AiAsk, got {other:?}"),
        }
    }

    fn rows(n: usize) -> QueryResult {
        QueryResult {
            columns: vec![domain::ResultColumn {
                name: "name".into(),
                data_type: domain::DataType::new("text"),
            }],
            rows: (0..n)
                .map(|i| vec![domain::Value::Text(format!("secret-{i}"))])
                .collect(),
            ..QueryResult::empty()
        }
    }

    fn reply(text: &str, sql: Option<&str>) -> AssistReply {
        AssistReply {
            text: text.into(),
            sql: sql.map(str::to_owned),
            explanation: String::new(),
            safety: None,
            warning: None,
            usage: None,
        }
    }

    #[test]
    fn startup_loads_ai_settings_and_key_status_only_for_keyed_providers() {
        let mut p = Presenter::new(vec![], fake_providers());
        let cmds = p.take_commands();
        assert!(
            matches!(cmds[0], Command::LoadConnections)
                && matches!(cmds[1], Command::LoadAiSettings)
        );
        assert!(matches!(&cmds[2], Command::LoadApiKeyStatus(id) if id.as_str() == "cloud"));
        assert_eq!(cmds.len(), 3);
    }

    #[test]
    fn saved_settings_trigger_a_model_listing() {
        let (mut p, _) = ai_ready();
        p.handle(Event::AiSettings(Ok(AiSettings {
            provider: Some(ProviderId::new("local")),
            model: Some(ModelId::new("m")),
        })));
        assert!(
            matches!(p.take_commands().as_slice(), [Command::ListModels(id)] if id.as_str() == "local")
        );
        // A provider that is no longer registered is ignored instead of selected.
        p.handle(Event::AiSettings(Ok(AiSettings {
            provider: Some(ProviderId::new("gone")),
            model: None,
        })));
        assert_eq!(p.ai.settings.provider, Some(ProviderId::new("local")));
    }

    #[test]
    fn asking_needs_a_provider_a_model_and_a_key() {
        let mut p = Presenter::new(
            vec![(DriverKind::new("postgres"), Dialect::ANSI)],
            fake_providers(),
        );
        p.take_commands();
        p.ai.input = "list customers".into();
        p.ai_ask(AiMode::Generate);
        assert!(matches!(&p.status, Some(Status::Error(m)) if m.contains("provider")));
        assert!(p.take_commands().is_empty());

        p.ai_select_provider(ProviderId::new("cloud"));
        p.take_commands();
        p.ai_ask(AiMode::Generate);
        assert!(matches!(&p.status, Some(Status::Error(m)) if m.contains("model")));
        p.ai_select_model(ModelId::new("big"));
        p.take_commands();
        p.ai_ask(AiMode::Generate);
        assert!(
            matches!(&p.status, Some(Status::Error(m)) if m.contains("API key")),
            "{:?}",
            p.status
        );
        assert!(p.take_commands().is_empty());
    }

    #[test]
    fn generate_sends_schema_and_question_but_no_rows_by_default() {
        let (mut p, tab) = ai_ready();
        p.tab_mut(tab).expect("tab").state = TabState::Done(rows(20));
        p.ai.input = "list customers".into();
        p.ai_ask(AiMode::Generate);

        let ask = only_ask(&mut p);
        assert_eq!((ask.provider.as_str(), ask.model.as_str()), ("local", "m"));
        assert!(
            matches!(&ask.task, AiTask::Generate { question, current_sql: None } if question == "list customers")
        );
        assert_eq!(ask.schema.tables().count(), 1);
        assert!(ask.sample_rows.is_none(), "rows are opt-in");
        assert!(p.ai.is_streaming());
        assert_eq!(p.right_panel, RightPanel::Ai);
        assert!(p.ai.input.is_empty(), "the question is consumed");
    }

    #[test]
    fn sample_rows_are_shared_only_when_ticked_limited_and_for_one_request_only() {
        let (mut p, tab) = ai_ready();
        p.tab_mut(tab).expect("tab").state = TabState::Done(rows(20));
        p.ai.input = "q".into();
        p.ai.include_rows = true;
        p.ai_ask(AiMode::Generate);
        let ask = only_ask(&mut p);
        assert_eq!(ask.sample_rows.expect("rows").rows.len(), 5);
        assert!(!p.ai.include_rows, "the opt-in does not carry over");

        p.handle(Event::AiFinished {
            id: ask.id,
            result: Ok(reply("x", None)),
        });
        p.ai.input = "again".into();
        p.ai_ask(AiMode::Generate);
        assert!(only_ask(&mut p).sample_rows.is_none());
    }

    #[test]
    fn ticking_the_box_without_a_result_sends_no_rows() {
        let (mut p, _) = ai_ready();
        p.ai.input = "q".into();
        p.ai.include_rows = true;
        p.ai_ask(AiMode::Generate);
        assert!(only_ask(&mut p).sample_rows.is_none());
    }

    #[test]
    fn the_editor_sql_travels_with_generate_and_is_required_for_explain_and_optimize() {
        let (mut p, tab) = ai_ready();
        p.ai_ask(AiMode::Explain);
        assert!(matches!(p.status, Some(Status::Error(_))));
        assert!(p.take_commands().is_empty());

        p.tab_mut(tab).expect("tab").text = "select 1".into();
        p.ai_ask(AiMode::Explain);
        assert!(matches!(only_ask(&mut p).task, AiTask::Explain { sql } if sql == "select 1"));
        p.handle(Event::AiFinished {
            id: crate::bridge::AiId(1),
            result: Ok(reply("x", None)),
        });
        p.ai_ask(AiMode::Optimize);
        assert!(matches!(only_ask(&mut p).task, AiTask::Optimize { .. }));
        p.handle(Event::AiFinished {
            id: crate::bridge::AiId(2),
            result: Ok(reply("x", None)),
        });

        p.ai.input = "only top 3".into();
        p.ai_ask(AiMode::Generate);
        assert!(
            matches!(only_ask(&mut p).task, AiTask::Generate { current_sql: Some(s), .. } if s == "select 1")
        );
    }

    #[test]
    fn fix_uses_the_statement_that_failed_not_the_edited_text() {
        let (mut p, tab) = ai_ready();
        p.ai_fix_last_error();
        assert!(matches!(&p.status, Some(Status::Error(m)) if m.contains("Nothing to fix")));

        p.tab_mut(tab).expect("tab").text = "SELECT nam FROM customers".into();
        p.run(tab);
        p.take_commands();
        p.handle(Event::QueryFinished {
            tab,
            result: Err("column nam does not exist".into()),
        });
        p.take_commands();
        p.tab_mut(tab).expect("tab").text = "something else now".into();

        p.ai_fix_last_error();
        let ask = only_ask(&mut p);
        assert!(matches!(&ask.task, AiTask::Fix { sql, error }
            if sql == "SELECT nam FROM customers" && error == "column nam does not exist"));
    }

    #[test]
    fn replies_stream_in_and_stale_events_are_ignored() {
        let (mut p, _) = ai_ready();
        p.ai.input = "q".into();
        p.ai_ask(AiMode::Generate);
        let id = only_ask(&mut p).id;

        p.handle(Event::AiText {
            id,
            text: "SELECT ".into(),
        });
        p.handle(Event::AiText {
            id,
            text: "1".into(),
        });
        p.handle(Event::AiText {
            id: crate::bridge::AiId(999),
            text: "NOISE".into(),
        });
        assert_eq!(p.ai.run.as_ref().map(|r| r.text.as_str()), Some("SELECT 1"));

        p.handle(Event::AiFinished {
            id: crate::bridge::AiId(999),
            result: Err("stale".into()),
        });
        assert!(
            p.ai.is_streaming(),
            "an old request cannot end the current one"
        );
        p.handle(Event::AiFinished {
            id,
            result: Ok(reply("SELECT 1", Some("SELECT 1"))),
        });
        assert!(!p.ai.is_streaming());
        assert!(matches!(
            p.ai.run.as_ref().map(|r| &r.state),
            Some(AiRunState::Done(_))
        ));
    }

    #[test]
    fn a_second_ask_while_streaming_is_ignored_and_stop_only_works_while_streaming() {
        let (mut p, _) = ai_ready();
        p.ai_stop();
        assert!(p.take_commands().is_empty());
        p.ai.input = "q".into();
        p.ai_ask(AiMode::Generate);
        let id = only_ask(&mut p).id;
        p.ai.input = "another".into();
        p.ai_ask(AiMode::Generate);
        assert!(p.take_commands().is_empty());
        p.ai_stop();
        assert!(matches!(p.take_commands().as_slice(), [Command::AiStop(i)] if *i == id));
    }

    #[test]
    fn failures_are_reported_and_keep_the_streamed_text() {
        let (mut p, _) = ai_ready();
        p.ai.input = "q".into();
        p.ai_ask(AiMode::Generate);
        let id = only_ask(&mut p).id;
        p.handle(Event::AiText {
            id,
            text: "partial".into(),
        });
        p.handle(Event::AiFinished {
            id,
            result: Err("rate limited".into()),
        });
        let run = p.ai.run.as_ref().expect("run");
        assert_eq!(run.text, "partial");
        assert!(matches!(&run.state, AiRunState::Failed(m) if m == "rate limited"));
        assert!(matches!(&p.status, Some(Status::Error(m)) if m.contains("rate limited")));
    }

    #[test]
    fn inserting_sql_never_executes_it() {
        let (mut p, tab) = ai_ready();
        p.ai.input = "q".into();
        p.ai_ask(AiMode::Generate);
        let id = only_ask(&mut p).id;
        p.handle(Event::AiFinished {
            id,
            result: Ok(reply("```sql\nDELETE FROM t\n```", Some("DELETE FROM t"))),
        });

        p.ai_insert_sql(false);
        assert_eq!(
            p.selected_tab().map(|t| t.text.as_str()),
            Some("DELETE FROM t")
        );
        p.tab_mut(tab).expect("tab").text = "select 1".into();
        p.ai_insert_sql(false);
        assert_eq!(
            p.selected_tab().map(|t| t.text.as_str()),
            Some("select 1\n\nDELETE FROM t"),
            "appended"
        );
        p.ai_insert_sql(true);
        assert_eq!(
            p.selected_tab().map(|t| t.text.as_str()),
            Some("DELETE FROM t"),
            "replaced"
        );

        let cmds = p.take_commands();
        assert!(
            !cmds.iter().any(|c| matches!(c, Command::Execute { .. })),
            "{cmds:?}"
        );
        assert!(matches!(
            p.selected_tab().map(|t| &t.state),
            Some(TabState::Idle)
        ));
    }

    #[test]
    fn inserting_does_nothing_without_sql_in_the_reply() {
        let (mut p, tab) = ai_ready();
        p.tab_mut(tab).expect("tab").text = "keep".into();
        p.ai.input = "q".into();
        p.ai_ask(AiMode::Generate);
        let id = only_ask(&mut p).id;
        p.handle(Event::AiFinished {
            id,
            result: Ok(reply("just words", None)),
        });
        p.ai_insert_sql(true);
        assert_eq!(p.selected_tab().map(|t| t.text.as_str()), Some("keep"));
    }

    #[test]
    fn chat_keeps_history_across_turns_and_can_be_cleared() {
        let (mut p, _) = ai_ready();
        p.ai.input = "what tables exist?".into();
        p.ai_ask(AiMode::Chat);
        let first = only_ask(&mut p);
        assert!(matches!(&first.task, AiTask::Chat { history, .. } if history.is_empty()));
        p.handle(Event::AiFinished {
            id: first.id,
            result: Ok(reply("customers", None)),
        });
        assert_eq!(p.ai.chat.len(), 2);

        p.ai.input = "and columns?".into();
        p.ai_ask(AiMode::Chat);
        let second = only_ask(&mut p);
        assert!(matches!(&second.task, AiTask::Chat { history, question }
            if history.len() == 2 && question == "and columns?"));
        p.handle(Event::AiFinished {
            id: second.id,
            result: Ok(reply("id, name", None)),
        });
        assert_eq!(p.ai.chat.len(), 4);
        p.ai.clear_chat();
        assert!(p.ai.chat.is_empty());
    }

    #[test]
    fn non_chat_replies_do_not_enter_the_chat_history() {
        let (mut p, _) = ai_ready();
        p.ai.input = "q".into();
        p.ai_ask(AiMode::Generate);
        let id = only_ask(&mut p).id;
        p.handle(Event::AiFinished {
            id,
            result: Ok(reply("x", None)),
        });
        assert!(p.ai.chat.is_empty());
    }

    #[test]
    fn preview_builds_the_same_request_without_starting_a_run() {
        let (mut p, tab) = ai_ready();
        p.tab_mut(tab).expect("tab").text = "select 1".into();
        p.ai_preview(AiMode::Explain);
        assert!(matches!(
            p.take_commands().as_slice(),
            [Command::AiPreview(_)]
        ));
        assert!(!p.ai.is_streaming() && p.ai.run.is_none());
    }

    #[test]
    fn choosing_a_provider_resets_the_model_lists_models_and_saves() {
        let (mut p, _) = ai_ready();
        p.ai_select_provider(ProviderId::new("cloud"));
        assert_eq!(p.ai.settings.model, None);
        assert!(matches!(
            p.ai.provider(&ProviderId::new("cloud")).map(|s| &s.models),
            Some(ModelsState::Loading)
        ));
        let cmds = p.take_commands();
        assert!(matches!(cmds[0], Command::ListModels(_)));
        assert!(matches!(cmds[1], Command::LoadApiKeyStatus(_)));
        assert!(
            matches!(&cmds[2], Command::SaveAiSettings(s) if s.provider == Some(ProviderId::new("cloud")) && s.model.is_none())
        );
        p.ai_select_provider(ProviderId::new("cloud")); // same again: nothing to do
        assert!(p.take_commands().is_empty());

        p.handle(Event::Models {
            provider: ProviderId::new("cloud"),
            result: Ok(vec![ModelInfo {
                id: ModelId::new("big"),
                display_name: "Big".into(),
            }]),
        });
        assert!(
            matches!(p.ai.provider(&ProviderId::new("cloud")).map(|s| &s.models), Some(ModelsState::Ready(m)) if m.len() == 1)
        );
        p.ai_select_model(ModelId::new("big"));
        assert!(
            matches!(&p.take_commands()[0], Command::SaveAiSettings(s) if s.model == Some(ModelId::new("big")))
        );
    }

    #[test]
    fn the_api_key_leaves_the_ui_state_the_moment_it_is_saved() {
        let (mut p, _) = ai_ready();
        p.ai_select_provider(ProviderId::new("cloud"));
        p.take_commands();
        p.ai.api_key_input = "   ".into();
        p.ai_save_key();
        assert!(p.take_commands().is_empty(), "blank input is ignored");

        p.ai.api_key_input = " sk-secret ".into();
        p.ai_save_key();
        assert!(p.ai.api_key_input.is_empty());
        let cmds = p.take_commands();
        assert!(
            matches!(&cmds[0], Command::SaveApiKey { provider, .. } if provider.as_str() == "cloud")
        );
        assert!(
            !format!("{cmds:?}").contains("sk-secret"),
            "commands never print the key"
        );

        p.handle(Event::ApiKeyStatus {
            provider: ProviderId::new("cloud"),
            set: Some(true),
        });
        assert_eq!(
            p.ai.provider(&ProviderId::new("cloud"))
                .and_then(|s| s.key_set),
            Some(true)
        );
        p.ai_clear_key();
        assert!(matches!(
            p.take_commands().as_slice(),
            [Command::ClearApiKey(_)]
        ));
    }

    #[test]
    fn notices_reach_the_status_bar() {
        let (mut p, _) = ai_ready();
        p.handle(Event::Notice(Ok("saved".into())));
        assert_eq!(p.status, Some(Status::Info("saved".into())));
        p.handle(Event::Notice(Err("nope".into())));
        assert!(matches!(p.status, Some(Status::Error(m)) if m == "nope"));
    }

    #[test]
    fn read_only_follows_the_connection_profile() {
        let mut p = Presenter::new(vec![], vec![]);
        let mut ro = profile();
        ro.set_read_only(true);
        let rw = profile();
        let (ro_id, rw_id) = (ro.id, rw.id);
        p.handle(Event::Connections(Ok(vec![ro, rw])));
        assert!(p.is_read_only(ro_id) && !p.is_read_only(rw_id));
        assert!(!p.is_read_only(ConnectionId::new()));
    }
}
