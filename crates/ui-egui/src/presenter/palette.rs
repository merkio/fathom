//! The command palette: a searchable list of commands, panel toggles, themes and tables.
//!
//! Pure state and logic; the view only draws `items()` and forwards keys. Choosing a table opens
//! a new tab with a `SELECT` for it but never runs it.

use application::usecases::ThemePreference;

use super::Presenter;
use crate::bridge::SessionId;

/// Tables shown for a query, so a large schema does not flood the list.
const MAX_TABLE_RESULTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    FocusMode,
    ToggleExplorer,
    ToggleResults,
    ToggleInlineAi,
    ToggleSidePanel,
    Theme(ThemePreference),
    NewQuery,
    ManageInstances,
    ExportResults,
    StorageConnections,
    OpenTable {
        session: SessionId,
        schema: String,
        table: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub action: PaletteAction,
    pub section: &'static str,
    pub title: String,
    pub subtitle: String,
    pub shortcut: Option<&'static str>,
    /// `Some(on)` for items that are switches.
    pub toggle: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}

fn item(
    action: PaletteAction,
    section: &'static str,
    title: &str,
    subtitle: &str,
    shortcut: Option<&'static str>,
    toggle: Option<bool>,
) -> PaletteItem {
    PaletteItem {
        action,
        section,
        title: title.to_owned(),
        subtitle: subtitle.to_owned(),
        shortcut,
        toggle,
    }
}

/// Every word of `query` must occur (case-insensitively) in the item's title or subtitle.
fn matches(query: &str, it: &PaletteItem) -> bool {
    let hay = format!("{} {}", it.title, it.subtitle).to_lowercase();
    query
        .split_whitespace()
        .all(|word| hay.contains(&word.to_lowercase()))
}

fn title_matches(query: &str, it: &PaletteItem) -> bool {
    let title = it.title.to_lowercase();
    query
        .split_whitespace()
        .all(|word| title.contains(&word.to_lowercase()))
}

impl Presenter {
    pub fn open_palette(&mut self) {
        self.palette = Some(PaletteState::default());
    }

    pub fn close_palette(&mut self) {
        self.palette = None;
    }

    pub fn set_palette_query(&mut self, query: String) {
        if let Some(state) = self.palette.as_mut()
            && state.query != query
        {
            state.query = query;
            state.selected = 0;
        }
    }

    /// The commands (and, for a non-empty query, tables) that match the palette's query.
    pub fn palette_items(&self) -> Vec<PaletteItem> {
        let Some(state) = &self.palette else {
            return Vec::new();
        };
        let inline_on = self.inline.settings.enabled;
        let mut items = vec![
            item(
                PaletteAction::FocusMode,
                "Workspace",
                "Enter focus mode",
                "Hide distractions. Keep your query and results.",
                Some("Ctrl+Shift+F"),
                None,
            ),
            item(
                PaletteAction::ToggleExplorer,
                "Workspace",
                "Explorer",
                "Instances and schema tree",
                Some("Ctrl+1"),
                Some(self.prefs.explorer),
            ),
            item(
                PaletteAction::ToggleResults,
                "Workspace",
                "Results panel",
                "The grid under the editor",
                Some("Ctrl+2"),
                Some(self.prefs.results),
            ),
            item(
                PaletteAction::ToggleInlineAi,
                "Workspace",
                "Inline AI",
                "Suggestions while you type (needs a provider and model)",
                None,
                Some(inline_on),
            ),
            item(
                PaletteAction::ToggleSidePanel,
                "Workspace",
                "Query history",
                "The panel with history and the AI assistant",
                Some("Ctrl+H"),
                Some(self.prefs.side_panel),
            ),
            item(
                PaletteAction::NewQuery,
                "Workspace",
                "New query",
                "Open an empty tab on the current connection",
                None,
                None,
            ),
            item(
                PaletteAction::ManageInstances,
                "Workspace",
                "Database instances",
                "Choose which databases the explorer shows",
                None,
                None,
            ),
            item(
                PaletteAction::ExportResults,
                "Workspace",
                "Export results...",
                "Save the results as a file in an export destination",
                Some("Ctrl+Shift+E"),
                None,
            ),
            item(
                PaletteAction::StorageConnections,
                "Workspace",
                "Storage connections",
                "Where exports can be written",
                None,
                None,
            ),
            item(
                PaletteAction::Theme(ThemePreference::Light),
                "Appearance",
                "Theme: light",
                "",
                None,
                Some(self.prefs.theme == ThemePreference::Light),
            ),
            item(
                PaletteAction::Theme(ThemePreference::Dark),
                "Appearance",
                "Theme: dark",
                "",
                None,
                Some(self.prefs.theme == ThemePreference::Dark),
            ),
            item(
                PaletteAction::Theme(ThemePreference::System),
                "Appearance",
                "Theme: follow the system",
                "",
                None,
                Some(self.prefs.theme == ThemePreference::System),
            ),
        ];
        items.retain(|it| matches(&state.query, it));
        // Matches in the title come before matches that are only in the description.
        items.sort_by_key(|it| !title_matches(&state.query, it));

        if !state.query.trim().is_empty() {
            let tables = self.open.iter().flat_map(|o| {
                let schema = match &o.schema {
                    super::Load::Ready(tree) => Some(tree),
                    _ => None,
                };
                schema.into_iter().flat_map(move |tree| {
                    tree.tables().map(move |(schema, t)| {
                        item(
                            PaletteAction::OpenTable {
                                session: o.session,
                                schema: schema.to_owned(),
                                table: t.name.clone(),
                            },
                            "Tables",
                            &t.name,
                            &format!("{schema} \u{00B7} {}", o.name),
                            None,
                            None,
                        )
                    })
                })
            });
            items.extend(
                tables
                    .filter(|it| matches(&state.query, it))
                    .take(MAX_TABLE_RESULTS),
            );
        }
        items
    }

    /// Moves the highlight (wrapping) within the current results.
    pub fn palette_move(&mut self, delta: isize) {
        let n = self.palette_items().len();
        if let Some(state) = self.palette.as_mut()
            && n > 0
        {
            state.selected = (state.selected as isize + delta).rem_euclid(n as isize) as usize;
        }
    }

    /// Runs the highlighted item and closes the palette.
    pub fn palette_accept(&mut self) {
        let items = self.palette_items();
        let Some(selected) = self.palette.as_ref().map(|s| s.selected) else {
            return;
        };
        if let Some(it) = items.get(selected) {
            let action = it.action.clone();
            self.run_palette(action);
        }
    }

    pub fn run_palette(&mut self, action: PaletteAction) {
        self.palette = None;
        match action {
            PaletteAction::FocusMode => self.focus_mode(),
            PaletteAction::ToggleExplorer => self.toggle_explorer(),
            PaletteAction::ToggleResults => self.toggle_results(),
            PaletteAction::ToggleInlineAi => {
                let on = !self.inline.settings.enabled;
                self.inline_set_enabled(on);
            }
            PaletteAction::ToggleSidePanel => self.toggle_side_panel(),
            PaletteAction::Theme(theme) => self.set_theme(theme),
            PaletteAction::ManageInstances => self.open_instances(None),
            PaletteAction::ExportResults => self.open_export_dialog(),
            PaletteAction::StorageConnections => self.open_storage_dialog(),
            PaletteAction::NewQuery => {
                let session = self
                    .selected_tab()
                    .map(|t| t.session)
                    .or_else(|| self.open.first().map(|o| o.session));
                if let Some(session) = session {
                    self.new_tab(session);
                }
            }
            PaletteAction::OpenTable {
                session,
                schema,
                table,
            } => self.open_table(session, &schema, &table),
        }
    }

    /// Hides the explorer and the side panel; the query and results stay.
    pub fn focus_mode(&mut self) {
        if self.prefs.explorer || self.prefs.side_panel {
            self.prefs.explorer = false;
            self.prefs.side_panel = false;
            self.outbox
                .push(crate::bridge::Command::SaveUiPrefs(self.prefs));
        }
    }

    /// Opens a tab holding `SELECT * FROM <table> LIMIT 100`. It is not executed.
    pub fn open_table(&mut self, session: SessionId, schema: &str, table: &str) {
        let Some(dialect) = self
            .open
            .iter()
            .find(|o| o.session == session)
            .map(|o| o.dialect)
        else {
            return;
        };
        let quoted = format!(
            "{}.{}",
            dialect.quote_identifier(schema),
            dialect.quote_identifier(table)
        );
        if let Some(id) = self.new_tab(session)
            && let Some(tab) = self.tab_mut(id)
        {
            tab.text = format!("SELECT * FROM {quoted} LIMIT 100;");
        }
    }
}
