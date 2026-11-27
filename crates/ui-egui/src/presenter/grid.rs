//! Row editing in the result grid: staged cell edits and deletions, the review step, and turning
//! them into `TableChanges`.
//!
//! Nothing here touches the database. Edits are only staged; they are sent when the user
//! confirms in the review dialog, and the result is shown again afterwards from the database.

use std::collections::{BTreeMap, BTreeSet};

use application::sql::changes::{EditableSource, editable_source};
use domain::{QueryResult, RowChange, TableChanges, Value};

use super::{Load, Presenter, Status, TabState};
use crate::bridge::{Command, TabId};

/// Typing exactly this sets a cell to NULL.
pub const NULL_TEXT: &str = "NULL";

/// Everything staged for one result grid. Cleared whenever the tab shows a new result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GridState {
    /// Whether the result was checked for editability yet, and the outcome.
    source: Option<Option<EditableSource>>,
    /// Rows ticked in the first column.
    pub selected: BTreeSet<usize>,
    /// New text per (row, column).
    pub edits: BTreeMap<(usize, usize), String>,
    /// Rows marked for deletion.
    pub deleted: BTreeSet<usize>,
    /// The cell being typed into, and its draft text.
    pub editing: Option<(usize, usize)>,
    pub draft: String,
    pub review_open: bool,
    /// An apply is in flight.
    pub applying: bool,
}

impl GridState {
    pub fn has_pending(&self) -> bool {
        !self.edits.is_empty() || !self.deleted.is_empty()
    }
}

/// One line of the review dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLine {
    pub kind: ReviewKind,
    /// The row's primary key, for display.
    pub row: String,
    /// `(column, old, new)` for updates; empty for deletes.
    pub columns: Vec<(String, String, String)>,
    /// A readable name for a delete (the row's first non-key text).
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    Update,
    Delete,
}

impl Presenter {
    fn grid(&self, tab: TabId) -> Option<&GridState> {
        self.tabs.iter().find(|t| t.id == tab).map(|t| &t.grid)
    }

    fn grid_mut(&mut self, tab: TabId) -> Option<&mut GridState> {
        self.tab_mut(tab).map(|t| &mut t.grid)
    }

    fn result_of(&self, tab: TabId) -> Option<&QueryResult> {
        match &self.tabs.iter().find(|t| t.id == tab)?.state {
            TabState::Done(r) => Some(r),
            _ => None,
        }
    }

    /// The table this tab's result can be edited in, or `None` (joins, aliases, read-only
    /// connections, tables without a primary key, ...). Cached until the next result.
    pub fn grid_source(&mut self, tab: TabId) -> Option<EditableSource> {
        let (session, executed, checked) = {
            let t = self.tabs.iter().find(|t| t.id == tab)?;
            (t.session, t.executed.clone(), t.grid.source.clone())
        };
        if let Some(known) = checked {
            return known;
        }
        // No result yet: say "no" without remembering it.
        self.result_of(tab)?;
        let connection = self.tab_connection(tab)?;
        let open = self.open.iter().find(|o| o.session == session)?;
        // Wait for the schema instead of caching a "no" that only means "not loaded yet".
        let Load::Ready(tree) = &open.schema else {
            return None;
        };
        let source = if self.is_read_only(connection) {
            None
        } else {
            editable_source(&executed, tree).filter(|s| {
                // The result must carry every key column under its own name.
                self.result_of(tab).is_some_and(|r| {
                    s.primary_key
                        .iter()
                        .all(|k| r.columns.iter().any(|c| c.name.eq_ignore_ascii_case(k)))
                })
            })
        };
        if let Some(g) = self.grid_mut(tab) {
            g.source = Some(source.clone());
        }
        source
    }

    fn is_key_column(source: &EditableSource, column: &str) -> bool {
        source
            .primary_key
            .iter()
            .any(|k| k.eq_ignore_ascii_case(column))
    }

    /// Starts typing in a cell. Key columns cannot be edited (rows are identified by them).
    pub fn begin_cell_edit(&mut self, tab: TabId, row: usize, col: usize) {
        let Some(source) = self.grid_source(tab) else {
            return;
        };
        let Some(result) = self.result_of(tab) else {
            return;
        };
        let (Some(column), Some(values)) = (result.columns.get(col), result.rows.get(row)) else {
            return;
        };
        if Self::is_key_column(&source, &column.name) {
            return;
        }
        let current = values.get(col).map(Value::display).unwrap_or_default();
        let original = values.get(col).cloned().unwrap_or(Value::Null);
        let Some(g) = self.grid_mut(tab) else { return };
        if g.deleted.contains(&row) {
            return;
        }
        g.draft = g
            .edits
            .get(&(row, col))
            .cloned()
            .unwrap_or(if original.is_null() {
                NULL_TEXT.into()
            } else {
                current
            });
        g.editing = Some((row, col));
    }

    /// Keeps the typed text as a staged edit (or drops it when it equals the stored value).
    pub fn commit_cell_edit(&mut self, tab: TabId) {
        let Some((row, col)) = self.grid(tab).and_then(|g| g.editing) else {
            return;
        };
        let original = self
            .result_of(tab)
            .and_then(|r| r.rows.get(row))
            .and_then(|v| v.get(col))
            .map(|v| {
                if v.is_null() {
                    NULL_TEXT.to_owned()
                } else {
                    v.display()
                }
            });
        let Some(g) = self.grid_mut(tab) else { return };
        let draft = std::mem::take(&mut g.draft);
        g.editing = None;
        if Some(&draft) == original.as_ref() {
            g.edits.remove(&(row, col));
        } else {
            g.edits.insert((row, col), draft);
        }
    }

    pub fn cancel_cell_edit(&mut self, tab: TabId) {
        if let Some(g) = self.grid_mut(tab) {
            g.editing = None;
            g.draft.clear();
        }
    }

    pub fn toggle_row_selected(&mut self, tab: TabId, row: usize) {
        if let Some(g) = self.grid_mut(tab)
            && !g.selected.remove(&row)
        {
            g.selected.insert(row);
        }
    }

    /// Marks the ticked rows for deletion (staged only).
    pub fn stage_delete_selected(&mut self, tab: TabId) {
        if let Some(g) = self.grid_mut(tab) {
            let rows: Vec<usize> = g.selected.iter().copied().collect();
            for row in rows {
                g.edits.retain(|(r, _), _| *r != row);
                g.deleted.insert(row);
            }
            g.selected.clear();
        }
    }

    /// Takes a staged deletion back.
    pub fn unstage_delete(&mut self, tab: TabId, row: usize) {
        if let Some(g) = self.grid_mut(tab) {
            g.deleted.remove(&row);
        }
    }

    pub fn discard_changes(&mut self, tab: TabId) {
        if let Some(g) = self.grid_mut(tab) {
            g.edits.clear();
            g.deleted.clear();
            g.selected.clear();
            g.editing = None;
            g.draft.clear();
            g.review_open = false;
        }
    }

    pub fn open_review(&mut self, tab: TabId) {
        if let Some(g) = self.grid_mut(tab)
            && g.has_pending()
        {
            g.review_open = true;
        }
    }

    pub fn close_review(&mut self, tab: TabId) {
        if let Some(g) = self.grid_mut(tab)
            && !g.applying
        {
            g.review_open = false;
        }
    }

    /// Staged cell edits and rows marked for deletion (edits on deleted rows are dropped when
    /// the deletion is staged, so the two never overlap).
    pub fn pending_counts(&self, tab: TabId) -> (usize, usize) {
        self.grid(tab)
            .map_or((0, 0), |g| (g.edits.len(), g.deleted.len()))
    }

    fn key_of(
        source: &EditableSource,
        result: &QueryResult,
        row: usize,
    ) -> Option<Vec<(String, Value)>> {
        let values = result.rows.get(row)?;
        source
            .primary_key
            .iter()
            .map(|k| {
                let i = result
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(k))?;
                Some((k.clone(), values.get(i)?.clone()))
            })
            .collect()
    }

    fn value_of_draft(text: &str) -> Value {
        if text == NULL_TEXT {
            Value::Null
        } else {
            Value::Text(text.to_owned())
        }
    }

    /// The staged changes in a form the database layer accepts: keyed by primary key values
    /// read from the result the user is looking at.
    pub fn staged_changes(&mut self, tab: TabId) -> Option<TableChanges> {
        let source = self.grid_source(tab)?;
        let session = self.tabs.iter().find(|t| t.id == tab)?.session;
        let result = self.result_of(tab)?.clone();
        let g = self.grid(tab)?.clone();
        let tree = self
            .open
            .iter()
            .find(|o| o.session == session)
            .and_then(|o| match &o.schema {
                Load::Ready(t) => Some(t.clone()),
                _ => None,
            })?;
        let table = tree
            .tables()
            .find(|(s, t)| *s == source.schema && t.name == source.table)
            .map(|(_, t)| t.clone())?;

        let mut changes = Vec::new();
        let rows: BTreeSet<usize> = g.edits.keys().map(|(r, _)| *r).collect();
        for row in rows {
            if g.deleted.contains(&row) {
                continue;
            }
            let key = Self::key_of(&source, &result, row)?;
            let set: Vec<(String, Value)> = g
                .edits
                .iter()
                .filter(|((r, _), _)| *r == row)
                .filter_map(|((_, c), text)| {
                    Some((
                        result.columns.get(*c)?.name.clone(),
                        Self::value_of_draft(text),
                    ))
                })
                .collect();
            changes.push(RowChange::Update { key, set });
        }
        for row in &g.deleted {
            changes.push(RowChange::Delete {
                key: Self::key_of(&source, &result, *row)?,
            });
        }
        Some(TableChanges {
            schema: Some(source.schema.clone()),
            table: source.table.clone(),
            column_types: table
                .columns
                .iter()
                .map(|c| (c.name.clone(), c.data_type.0.clone()))
                .collect(),
            changes,
        })
    }

    /// What the review dialog lists, in grid order.
    pub fn review_lines(&mut self, tab: TabId) -> Vec<ReviewLine> {
        let Some(source) = self.grid_source(tab) else {
            return Vec::new();
        };
        let (Some(result), Some(g)) = (self.result_of(tab), self.grid(tab)) else {
            return Vec::new();
        };
        let show = |v: &Value| {
            if v.is_null() {
                NULL_TEXT.to_owned()
            } else {
                v.display()
            }
        };
        let row_label = |row: usize| {
            Self::key_of(&source, result, row)
                .map(|k| {
                    k.iter()
                        .map(|(_, v)| v.display())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        let mut lines = Vec::new();
        let rows: BTreeSet<usize> = g
            .edits
            .keys()
            .map(|(r, _)| *r)
            .chain(g.deleted.iter().copied())
            .collect();
        for row in rows {
            if g.deleted.contains(&row) {
                let label = result
                    .rows
                    .get(row)
                    .and_then(|v| {
                        v.iter().zip(&result.columns).find_map(|(v, c)| {
                            (matches!(v, Value::Text(_)) && !Self::is_key_column(&source, &c.name))
                                .then(|| v.display())
                        })
                    })
                    .unwrap_or_default();
                lines.push(ReviewLine {
                    kind: ReviewKind::Delete,
                    row: row_label(row),
                    columns: Vec::new(),
                    label,
                });
            } else {
                let columns = g
                    .edits
                    .iter()
                    .filter(|((r, _), _)| *r == row)
                    .filter_map(|((_, c), new)| {
                        let name = result.columns.get(*c)?.name.clone();
                        let old = result.rows.get(row)?.get(*c).map(show).unwrap_or_default();
                        Some((name, old, new.clone()))
                    })
                    .collect();
                lines.push(ReviewLine {
                    kind: ReviewKind::Update,
                    row: row_label(row),
                    columns,
                    label: String::new(),
                });
            }
        }
        lines
    }

    /// Whether a failure part-way leaves nothing applied for this tab's database.
    pub fn applies_atomically(&self, tab: TabId) -> bool {
        self.tabs
            .iter()
            .find(|t| t.id == tab)
            .and_then(|t| self.open.iter().find(|o| o.session == t.session))
            .is_none_or(|o| o.dialect.name != "clickhouse")
    }

    /// A copy of the staged state for the view to read while it holds other borrows.
    pub fn grid_snapshot(&self, tab: TabId) -> GridState {
        self.grid(tab).cloned().unwrap_or_default()
    }

    pub fn set_draft(&mut self, tab: TabId, text: String) {
        if let Some(g) = self.grid_mut(tab)
            && g.editing.is_some()
        {
            g.draft = text;
        }
    }

    /// Sends the reviewed changes. Only called from the review dialog's Apply button.
    pub fn apply_staged(&mut self, tab: TabId) {
        if self
            .grid(tab)
            .is_none_or(|g| g.applying || !g.has_pending())
        {
            return;
        }
        let Some(changes) = self.staged_changes(tab) else {
            self.status = Some(Status::Error(
                "These results can no longer be edited; nothing was changed.".into(),
            ));
            return;
        };
        let Some(session) = self.tabs.iter().find(|t| t.id == tab).map(|t| t.session) else {
            return;
        };
        if let Some(g) = self.grid_mut(tab) {
            g.applying = true;
        }
        self.outbox.push(Command::ApplyChanges {
            tab,
            session,
            changes,
        });
    }

    pub(super) fn on_changes_applied(
        &mut self,
        tab: TabId,
        result: Result<domain::ApplyOutcome, String>,
    ) {
        match result {
            Ok(outcome) => {
                let note = if outcome.atomic {
                    format!("Applied {} change(s) in one transaction", outcome.applied)
                } else {
                    format!("Applied {} change(s)", outcome.applied)
                };
                self.status = Some(Status::Info(note));
                if let Some(g) = self.grid_mut(tab) {
                    *g = GridState::default();
                }
                // Show what the database has now: run the same statement again.
                self.rerun_executed(tab);
            }
            Err(message) => {
                if let Some(g) = self.grid_mut(tab) {
                    g.applying = false;
                }
                // The staged edits stay so nothing the user typed is lost.
                self.status = Some(Status::Error(message));
            }
        }
    }

    /// Runs the statement the tab last ran, regardless of what the editor holds now.
    fn rerun_executed(&mut self, id: TabId) {
        let Some(connection) = self.tab_connection(id) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if tab.executed.trim().is_empty() {
            return;
        }
        tab.state = TabState::Running;
        self.outbox.push(Command::Execute {
            tab: id,
            session: tab.session,
            connection,
            text: tab.executed.clone(),
        });
    }
}
