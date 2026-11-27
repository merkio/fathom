//! Saved queries: the sidebar list, the Save dialog and opening a saved query in a tab.
//!
//! Saving never runs anything. A tab linked to a saved query shows a marker when its text
//! differs from what was saved, and Save (Ctrl+S) updates the saved query in place.

use application::usecases::SaveDraft;
use domain::{SavedQuery, SavedQueryId};

use super::{Presenter, Status};
use crate::bridge::{Command, TabId};

/// The "Save query" dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDialog {
    pub tab: TabId,
    pub name: String,
    /// Save a new copy even if the tab is linked to a saved query ("Save as...").
    pub as_new: bool,
}

impl Presenter {
    fn saved_by_id(&self, id: SavedQueryId) -> Option<&SavedQuery> {
        self.saved.iter().find(|q| q.id == id)
    }

    /// Whether the tab holds a saved query with unsaved changes.
    pub fn is_dirty(&self, tab: TabId) -> bool {
        self.tabs
            .iter()
            .find(|t| t.id == tab)
            .is_some_and(|t| t.saved.is_some() && t.text != t.saved_text)
    }

    /// Ctrl+S: update the linked saved query, or ask for a name.
    pub fn save_current(&mut self) {
        let Some(tab) = self.selected else { return };
        let Some(t) = self.tabs.iter().find(|t| t.id == tab) else {
            return;
        };
        if t.text.trim().is_empty() {
            self.status = Some(Status::Error("There is no SQL to save.".into()));
            return;
        }
        let linked = t.saved.and_then(|id| self.saved_by_id(id).cloned());
        match linked {
            Some(q) => self.send_save(tab, Some(q.id), q.name),
            None => self.open_save_dialog(false),
        }
    }

    /// Opens the Save dialog ("Save as..." when `as_new`).
    pub fn open_save_dialog(&mut self, as_new: bool) {
        let Some(tab) = self.selected else { return };
        let Some(t) = self.tabs.iter().find(|t| t.id == tab) else {
            return;
        };
        if t.text.trim().is_empty() {
            self.status = Some(Status::Error("There is no SQL to save.".into()));
            return;
        }
        let name = t
            .saved
            .and_then(|id| self.saved_by_id(id))
            .map(|q| q.name.clone())
            .unwrap_or_default();
        self.save_dialog = Some(SaveDialog { tab, name, as_new });
    }

    pub fn set_save_name(&mut self, name: String) {
        if let Some(d) = self.save_dialog.as_mut() {
            d.name = name;
        }
    }

    pub fn cancel_save_dialog(&mut self) {
        self.save_dialog = None;
    }

    /// Sends the dialog's choice. The dialog closes when the save succeeds.
    pub fn confirm_save(&mut self) {
        let Some(d) = self.save_dialog.clone() else {
            return;
        };
        if d.name.trim().is_empty() {
            self.status = Some(Status::Error("Give the query a name.".into()));
            return;
        }
        let id = if d.as_new {
            None
        } else {
            self.tabs
                .iter()
                .find(|t| t.id == d.tab)
                .and_then(|t| t.saved)
        };
        self.send_save(d.tab, id, d.name);
    }

    fn send_save(&mut self, tab: TabId, id: Option<SavedQueryId>, name: String) {
        let Some(text) = self
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .map(|t| t.text.clone())
        else {
            return;
        };
        let connection = self.tab_connection(tab);
        self.outbox.push(Command::SaveQuery {
            tab,
            draft: SaveDraft {
                id,
                name,
                text,
                connection,
            },
        });
    }

    pub(super) fn on_query_saved(&mut self, tab: TabId, result: Result<SavedQuery, String>) {
        match result {
            Ok(q) => {
                self.saved.retain(|s| s.id != q.id);
                self.saved.push(q.clone());
                self.saved.sort_by_key(|s| s.name.to_lowercase());
                self.save_dialog = None;
                if let Some(t) = self.tab_mut(tab) {
                    t.saved = Some(q.id);
                    t.saved_text = q.text.clone();
                    t.title = q.name.clone();
                }
                self.status = Some(Status::Info(format!("Saved `{}`", q.name)));
            }
            // The dialog stays open so the name can be fixed.
            Err(message) => self.status = Some(Status::Error(message)),
        }
    }

    pub(super) fn on_saved_queries(&mut self, result: Result<Vec<SavedQuery>, String>) {
        match result {
            Ok(list) => {
                // A deleted query leaves its tabs open, just no longer linked.
                for t in &mut self.tabs {
                    if t.saved.is_some_and(|id| !list.iter().any(|q| q.id == id)) {
                        t.saved = None;
                    }
                }
                self.saved = list;
            }
            Err(e) => self.error(format!("Could not load saved queries: {e}")),
        }
    }

    /// Opens a saved query in a tab: the one already showing it, or a new tab on the connection
    /// it was written for (if open), else the current one. It is not run.
    pub fn open_saved(&mut self, id: SavedQueryId) {
        let Some(q) = self.saved_by_id(id).cloned() else {
            return;
        };
        if let Some(existing) = self.tabs.iter().find(|t| t.saved == Some(id)).map(|t| t.id) {
            self.select_tab(existing);
            return;
        }
        let session = q
            .connection
            .and_then(|c| self.open.iter().find(|o| o.connection == c))
            .map(|o| o.session)
            .or_else(|| self.selected_tab().map(|t| t.session))
            .or_else(|| self.open.first().map(|o| o.session));
        let Some(session) = session else {
            self.status = Some(Status::Error(
                "Connect to a database first, then open the query.".into(),
            ));
            return;
        };
        if let Some(tab) = self.new_tab(session)
            && let Some(t) = self.tab_mut(tab)
        {
            t.text = q.text.clone();
            t.saved = Some(q.id);
            t.saved_text = q.text;
            t.title = q.name;
        }
    }

    /// Asks for confirmation before deleting (the sidebar shows "Delete?").
    pub fn request_delete_saved(&mut self, id: SavedQueryId) {
        if self.saved_by_id(id).is_some() {
            self.confirm_delete = Some(id);
        }
    }

    pub fn cancel_delete_saved(&mut self) {
        self.confirm_delete = None;
    }

    pub fn confirm_delete_saved(&mut self) {
        if let Some(id) = self.confirm_delete.take() {
            self.outbox.push(Command::DeleteSavedQuery(id));
        }
    }
}
