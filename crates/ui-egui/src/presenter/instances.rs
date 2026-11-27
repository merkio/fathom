//! The "Database instances" dialog: pick which databases of each instance the explorer shows.
//!
//! Hiding a database only changes the explorer; it never disconnects or alters anything.

use std::collections::BTreeSet;

use domain::ConnectionId;

use super::{Load, Presenter};
use crate::bridge::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstanceTab {
    #[default]
    Databases,
    Connection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstancesState {
    pub selected: Option<ConnectionId>,
    pub tab: InstanceTab,
    pub search: String,
    /// The databases ticked in the dialog (not saved until "Save visibility").
    pub checked: BTreeSet<String>,
}

/// A database of an open instance, for the checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRow {
    pub name: String,
    pub tables: usize,
}

impl Presenter {
    pub fn open_instances(&mut self, select: Option<ConnectionId>) {
        let first = select.or_else(|| self.connections.first().map(|c| c.id));
        self.instances = Some(InstancesState::default());
        if let Some(id) = first {
            self.select_instance(id);
        }
    }

    pub fn close_instances(&mut self) {
        self.instances = None;
    }

    pub fn select_instance(&mut self, id: ConnectionId) {
        let checked = self.initial_checked(id);
        if let Some(state) = self.instances.as_mut() {
            state.selected = Some(id);
            state.search.clear();
            state.checked = checked;
        }
    }

    /// What starts ticked: the saved choice, or everything when nothing was chosen yet.
    fn initial_checked(&self, id: ConnectionId) -> BTreeSet<String> {
        match self.visibility.get(&id) {
            Some(saved) => saved.iter().cloned().collect(),
            None => self.databases_of(id).into_iter().map(|d| d.name).collect(),
        }
    }

    /// The databases of an instance that is open and whose schema has loaded.
    pub fn databases_of(&self, id: ConnectionId) -> Vec<DatabaseRow> {
        let Some(open) = self.open.iter().find(|o| o.connection == id) else {
            return Vec::new();
        };
        let Load::Ready(tree) = &open.schema else {
            return Vec::new();
        };
        tree.catalogs
            .iter()
            .flat_map(|c| &c.schemas)
            .map(|s| DatabaseRow {
                name: s.name.clone(),
                tables: s.tables.len(),
            })
            .collect()
    }

    /// All databases of the instance selected in the dialog.
    pub fn databases_of_selected(&self) -> Vec<DatabaseRow> {
        self.instances
            .as_ref()
            .and_then(|s| s.selected)
            .map(|id| self.databases_of(id))
            .unwrap_or_default()
    }

    /// The checklist rows that match the dialog's search box.
    pub fn visible_database_rows(&self) -> Vec<DatabaseRow> {
        let Some(state) = &self.instances else {
            return Vec::new();
        };
        let Some(id) = state.selected else {
            return Vec::new();
        };
        let needle = state.search.trim().to_lowercase();
        self.databases_of(id)
            .into_iter()
            .filter(|d| needle.is_empty() || d.name.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn toggle_database(&mut self, name: &str) {
        if let Some(state) = self.instances.as_mut()
            && !state.checked.remove(name)
        {
            state.checked.insert(name.to_owned());
        }
    }

    /// Ticks every database that matches the search.
    pub fn select_all_databases(&mut self) {
        let names: Vec<String> = self
            .visible_database_rows()
            .into_iter()
            .map(|d| d.name)
            .collect();
        if let Some(state) = self.instances.as_mut() {
            state.checked.extend(names);
        }
    }

    /// Reloads the database list of the selected instance (when it is open).
    pub fn refresh_instance_databases(&mut self) {
        let session = self
            .instances
            .as_ref()
            .and_then(|s| s.selected)
            .and_then(|id| self.open.iter().find(|o| o.connection == id))
            .map(|o| o.session);
        if let Some(session) = session {
            self.refresh_schema(session);
        }
    }

    /// Saves the ticked databases for the selected instance and closes the dialog.
    pub fn save_visibility(&mut self) {
        let Some(state) = self.instances.take() else {
            return;
        };
        let Some(id) = state.selected else {
            return;
        };
        // Keep the server's order, and keep ticks for databases that are not listed right now
        // (an instance that is not connected cannot list them).
        let listed: Vec<String> = self.databases_of(id).into_iter().map(|d| d.name).collect();
        let mut visible: Vec<String> = listed
            .iter()
            .filter(|n| state.checked.contains(*n))
            .cloned()
            .collect();
        visible.extend(
            state
                .checked
                .iter()
                .filter(|n| !listed.contains(*n))
                .cloned(),
        );
        self.visibility.insert(id, visible.clone());
        self.outbox.push(Command::SaveVisibility {
            connection: id,
            visible,
        });
    }

    /// Whether the explorer shows `database` of `connection`.
    pub fn is_database_visible(&self, connection: ConnectionId, database: &str) -> bool {
        self.visibility
            .get(&connection)
            .is_none_or(|v| v.iter().any(|n| n == database))
    }

    /// How many databases of an open instance are shown, and how many there are.
    pub fn shown_databases(&self, connection: ConnectionId) -> Option<(usize, usize)> {
        let all = self.databases_of(connection);
        if all.is_empty() {
            return None;
        }
        let shown = all
            .iter()
            .filter(|d| self.is_database_visible(connection, &d.name))
            .count();
        Some((shown, all.len()))
    }
}
