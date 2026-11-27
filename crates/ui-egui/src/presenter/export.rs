//! Exporting results: the Export dialog's state, choosing rows and a destination, and the
//! Storage connections dialog.
//!
//! An export writes a new file in one of the destinations from the configuration file; it never
//! overwrites. The rows are either the statement run again (up to the export cap) or exactly the
//! rows the user ticked.

use application::export::ExportFormat;
use application::registry::DestinationInfo;
use domain::QueryResult;

use super::{Presenter, Status, TabState};
use crate::bridge::{Command, ExportDone, ExportSource, TabId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportUi {
    pub format: ExportFormat,
    /// File name typed by the user (empty: a default name).
    pub name: String,
    /// Folder inside the destination.
    pub folder: String,
    /// The chosen destination's name.
    pub destination: Option<String>,
    /// Export only the rows ticked in the grid.
    pub selected_only: bool,
    pub running: bool,
    pub dialog_open: bool,
}

impl Default for ExportUi {
    fn default() -> Self {
        Self {
            format: ExportFormat::Csv,
            name: String::new(),
            folder: String::new(),
            destination: None,
            selected_only: false,
            running: false,
            dialog_open: false,
        }
    }
}

/// The outcome of "Test connection" for a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestState {
    Testing,
    Ok,
    Failed(String),
}

/// The add/edit form of a storage connection. Typed secrets live here only until submitted.
#[derive(Debug, Clone, Default)]
pub struct StorageForm {
    /// The connection being edited (`None` for a new one).
    pub original: Option<String>,
    pub name: String,
    pub kind: String,
    pub settings: std::collections::BTreeMap<String, String>,
    pub secrets: std::collections::BTreeMap<String, String>,
    /// Secrets already stored for the connection (the form shows "unchanged").
    pub stored_secrets: bool,
    pub test: Option<TestState>,
    pub error: Option<String>,
    pub busy: bool,
    pub confirm_delete: bool,
}

impl StorageForm {
    fn draft(&self) -> application::usecases::ConnectionDraft {
        application::usecases::ConnectionDraft {
            original_name: self.original.clone(),
            name: self.name.clone(),
            kind: self.kind.clone(),
            settings: self.settings.clone(),
            secrets: self
                .secrets
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), secrecy::SecretString::from(v.clone())))
                .collect(),
        }
    }
}

impl Presenter {
    /// Opens an empty form for a new connection of `kind`.
    pub fn new_storage_form(&mut self, kind: &str) {
        self.storage_form = Some(StorageForm {
            kind: kind.to_owned(),
            ..StorageForm::default()
        });
    }

    pub fn edit_storage_form(&mut self, name: &str) {
        let Some(d) = self
            .destinations
            .iter()
            .find(|d| d.name == name && d.editable)
        else {
            return;
        };
        self.storage_form = Some(StorageForm {
            original: Some(d.name.clone()),
            name: d.name.clone(),
            kind: d.kind.clone(),
            settings: d.settings.clone(),
            stored_secrets: true,
            ..StorageForm::default()
        });
    }

    pub fn cancel_storage_form(&mut self) {
        self.storage_form = None;
    }

    pub fn submit_storage_form(&mut self) {
        if let Some(form) = self.storage_form.as_mut().filter(|f| !f.busy) {
            form.busy = true;
            form.error = None;
            self.outbox.push(Command::SaveStorage(form.draft()));
        }
    }

    pub fn test_storage_form(&mut self) {
        if let Some(form) = self
            .storage_form
            .as_mut()
            .filter(|f| !matches!(f.test, Some(TestState::Testing)))
        {
            form.test = Some(TestState::Testing);
            self.outbox.push(Command::TestStorageDraft(form.draft()));
        }
    }

    pub fn delete_storage(&mut self, name: &str) {
        if self
            .destinations
            .iter()
            .any(|d| d.name == name && d.editable)
        {
            self.outbox.push(Command::DeleteStorage(name.to_owned()));
            self.storage_form = None;
            self.destination_tests.remove(name);
            self.storage_dialog = Some(None);
        }
    }

    pub(super) fn on_storage_saved(&mut self, result: Result<(), String>) {
        let Some(form) = self.storage_form.as_mut() else {
            return;
        };
        form.busy = false;
        match result {
            Ok(()) => {
                let name = form.name.trim().to_owned();
                self.storage_form = None;
                self.destination_tests.remove(&name);
                self.storage_dialog = Some(Some(name));
            }
            Err(message) => form.error = Some(message),
        }
    }

    /// Whether the selected tab shows a result that can be exported.
    pub fn can_export(&self) -> bool {
        self.selected_tab().is_some_and(|t| {
            matches!(&t.state, TabState::Done(r) if !r.columns.is_empty())
                && !t.executed_text().trim().is_empty()
        })
    }

    pub fn open_export_dialog(&mut self) {
        if !self.can_export() {
            return;
        }
        if self
            .export
            .destination
            .as_ref()
            .is_none_or(|d| !self.destinations.iter().any(|i| &i.name == d))
        {
            self.export.destination = self.destinations.first().map(|d| d.name.clone());
        }
        self.export.dialog_open = true;
    }

    pub fn close_export_dialog(&mut self) {
        if !self.export.running {
            self.export.dialog_open = false;
        }
    }

    pub fn choose_destination(&mut self, name: &str) {
        if self.destinations.iter().any(|d| d.name == name) {
            self.export.destination = Some(name.to_owned());
        }
    }

    /// Rows ticked in the grid of the selected tab.
    pub fn selected_row_count(&self) -> usize {
        self.selected
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
            .map_or(0, |t| t.grid.selected.len())
    }

    /// Rows an export would hold: the ticked ones, or all the grid has (the real export may hold
    /// more, up to the cap, because the statement is run again).
    pub fn export_row_hint(&self) -> usize {
        self.selected_tab()
            .and_then(|t| match &t.state {
                TabState::Done(r) => Some(r.rows.len()),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// The tab's ticked rows as a result of their own.
    fn selected_rows(&self, tab: TabId) -> Option<QueryResult> {
        let t = self.tabs.iter().find(|t| t.id == tab)?;
        let TabState::Done(result) = &t.state else {
            return None;
        };
        let rows: Vec<_> = t
            .grid
            .selected
            .iter()
            .filter_map(|i| result.rows.get(*i).cloned())
            .collect();
        (!rows.is_empty()).then(|| QueryResult {
            columns: result.columns.clone(),
            rows,
            rows_affected: None,
            elapsed: result.elapsed,
            truncated: false,
        })
    }

    /// Starts the export with the dialog's choices.
    pub fn start_export(&mut self) {
        if self.export.running {
            return;
        }
        let Some(id) = self.selected else { return };
        let Some(connection) = self.tab_connection(id) else {
            return;
        };
        if !self.can_export() {
            self.status = Some(Status::Error(
                "Run a query first; the export needs its result.".into(),
            ));
            return;
        }
        let Some(destination) = self.export.destination.clone() else {
            self.status = Some(Status::Error(
                "Choose a destination. Configure destinations in fathom.toml (see the README)."
                    .into(),
            ));
            return;
        };
        let source = if self.export.selected_only {
            match self.selected_rows(id) {
                Some(rows) => ExportSource::Rows(rows),
                None => {
                    self.status = Some(Status::Error("No rows are ticked.".into()));
                    return;
                }
            }
        } else {
            ExportSource::Query(
                self.tabs
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.executed_text().to_owned())
                    .unwrap_or_default(),
            )
        };
        let table = self.grid_source(id).map(|s| s.table);
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return;
        };
        self.export.running = true;
        self.outbox.push(Command::Export {
            tab: id,
            session: tab.session,
            connection,
            source,
            destination,
            folder: self.export.folder.clone(),
            name: self.export.name.clone(),
            format: self.export.format,
            table,
        });
    }

    pub(super) fn on_exported(&mut self, result: Result<ExportDone, String>) {
        self.export.running = false;
        self.status = Some(match result {
            Ok(done) => {
                self.export.dialog_open = false;
                // The next export is a different file; keep destination, folder and format.
                self.export.name.clear();
                let more = if done.truncated {
                    " (stopped at the row limit; more rows exist)"
                } else {
                    ""
                };
                Status::Info(format!(
                    "Exported {} row(s) to {}{more}",
                    done.rows, done.location
                ))
            }
            // The dialog stays open so the name or folder can be fixed.
            Err(message) => Status::Error(format!("Export failed: {message}")),
        });
    }

    pub(super) fn on_destinations(
        &mut self,
        items: Vec<DestinationInfo>,
        problems: Vec<String>,
        kinds: Vec<String>,
    ) {
        self.destinations = items;
        self.destination_problems = problems;
        self.storage_kinds = kinds;
        if self.storage_dialog == Some(None) {
            self.storage_dialog = Some(self.destinations.first().map(|d| d.name.clone()));
        }
        if self
            .export
            .destination
            .as_ref()
            .is_some_and(|d| !self.destinations.iter().any(|i| &i.name == d))
        {
            self.export.destination = None;
        }
    }

    // ---- Storage connections dialog ----------------------------------------------------------

    pub fn open_storage_dialog(&mut self) {
        self.storage_dialog = Some(self.destinations.first().map(|d| d.name.clone()));
    }

    pub fn close_storage_dialog(&mut self) {
        self.storage_dialog = None;
    }

    pub fn select_storage(&mut self, name: &str) {
        if self.destinations.iter().any(|d| d.name == name) {
            self.storage_dialog = Some(Some(name.to_owned()));
        }
    }

    pub fn test_destination(&mut self, name: &str) {
        if matches!(self.destination_tests.get(name), Some(TestState::Testing)) {
            return;
        }
        if self.destinations.iter().any(|d| d.name == name) {
            self.destination_tests
                .insert(name.to_owned(), TestState::Testing);
            self.outbox.push(Command::TestDestination(name.to_owned()));
        }
    }
}
