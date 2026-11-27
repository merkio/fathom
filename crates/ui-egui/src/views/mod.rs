//! Views: pure egui drawing. They read presenter state and call presenter methods; they contain
//! no business logic.

mod ai;
mod ai_card;
mod code_editor;
mod connections;
mod editor;
mod export;
mod history;
mod instances;
mod palette;
mod results;
mod saved;
mod sidebar;
mod storage;

use application::usecases::ThemePreference;
use eframe::egui::{
    self, Frame, Key, KeyboardShortcut, Margin, Modifiers, Panel, RichText, Stroke,
};

use crate::presenter::{Presenter, RightPanel, Status};
use crate::theme;

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    shortcuts(ui, p);
    let pal = *theme::palette(ui);
    let chrome = Frame::NONE
        .fill(pal.chrome)
        .stroke(Stroke::new(1.0, pal.border))
        .inner_margin(Margin::symmetric(14, 6));
    let surface = Frame::NONE
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.border))
        .inner_margin(Margin::same(10));

    Panel::top("header")
        .frame(chrome)
        .show(ui, |ui| header(ui, p));
    Panel::bottom("status")
        .frame(chrome)
        .show(ui, |ui| status_bar(ui, p));
    if p.prefs.explorer {
        Panel::left("sidebar")
            .frame(surface)
            .default_size(250.0)
            .min_size(200.0)
            .show(ui, |ui| sidebar::show(ui, p));
    }
    if p.prefs.side_panel {
        Panel::right("right_panel")
            .frame(surface)
            .default_size(340.0)
            .min_size(220.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut p.right_panel, RightPanel::History, "History");
                    ui.selectable_value(&mut p.right_panel, RightPanel::Ai, "AI assistant");
                });
                ui.separator();
                match p.right_panel {
                    RightPanel::History => history::show(ui, p),
                    RightPanel::Ai => ai::show(ui, p),
                }
            });
    }
    egui::CentralPanel::default()
        .frame(Frame::NONE.fill(pal.bg))
        .show(ui, |ui| editor::show(ui, p));
    connections::show_form(ui.ctx(), p);
    results::review_dialog(ui.ctx(), p);
    storage::show(ui.ctx(), p);
    export::show(ui.ctx(), p);
    saved::save_dialog(ui.ctx(), p);
    instances::show(ui.ctx(), p);
    palette::show(ui.ctx(), p);
}

/// Window-wide shortcuts. The palette owns the keyboard while it is open.
fn shortcuts(ui: &egui::Ui, p: &mut Presenter) {
    let cmd = Modifiers::COMMAND;
    let (mut open, mut focus, mut explorer, mut results, mut side, mut export, mut save) =
        (false, false, false, false, false, false, false);
    let mut review = false;
    ui.input_mut(|i| {
        // The shifted shortcut first: `Cmd+F` must not swallow `Cmd+Shift+F`.
        // Shifted shortcuts first, so `Cmd+Enter` (run) cannot swallow `Cmd+Shift+Enter`.
        review = i.consume_shortcut(&KeyboardShortcut::new(cmd | Modifiers::SHIFT, Key::Enter));
        focus = i.consume_shortcut(&KeyboardShortcut::new(cmd | Modifiers::SHIFT, Key::F));
        export = i.consume_shortcut(&KeyboardShortcut::new(cmd | Modifiers::SHIFT, Key::E));
        save = i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::S));
        open = i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::K));
        explorer = i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num1));
        results = i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num2));
        side = i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::H));
    });
    if open {
        if p.palette.is_some() {
            p.close_palette();
        } else {
            p.open_palette();
        }
    }
    if focus {
        p.focus_mode();
    }
    if export {
        p.open_export_dialog();
    }
    if save {
        p.save_current();
    }
    if review && let Some(tab) = p.selected {
        p.open_review(tab);
    }
    if explorer {
        p.toggle_explorer();
    }
    if results {
        p.toggle_results();
    }
    if side {
        p.toggle_side_panel();
    }
}

/// App name, the open query's breadcrumb, the layout menu, the theme switch and the palette.
fn header(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);
    let cmd_k = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
    ui.horizontal(|ui| {
        ui.set_min_height(36.0);
        ui.label(RichText::new("Fathom").strong().size(14.0));
        let crumb = p.selected_tab().map(|t| {
            let connection = p
                .open
                .iter()
                .find(|o| o.session == t.session)
                .map_or("", |o| o.name.as_str());
            format!("{connection} / {}", t.title)
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(ui.ctx().format_shortcut(&cmd_k)).color(pal.text_muted),
                    )
                    .min_size(egui::vec2(64.0, 28.0)),
                )
                .on_hover_text("Search commands, tables and queries")
                .clicked()
            {
                p.open_palette();
            }
            let (glyph, next, hint) = match p.prefs.theme {
                ThemePreference::System => (
                    "\u{25D0}",
                    ThemePreference::Light,
                    "Theme: follows the system",
                ),
                ThemePreference::Light => ("\u{2600}", ThemePreference::Dark, "Theme: light"),
                ThemePreference::Dark => ("\u{263E}", ThemePreference::System, "Theme: dark"),
            };
            if ui
                .add(egui::Button::new(RichText::new(glyph).size(16.0)).frame(false))
                .on_hover_text(format!("{hint} (click to change)"))
                .clicked()
            {
                p.set_theme(next);
            }
            ui.separator();
            let layout = RichText::new(format!(
                "\u{25A4}  Layout   {}",
                ui.ctx().format_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::L
                ))
            ))
            .color(pal.text_muted);
            ui.menu_button(layout, |ui| {
                let (mut explorer, mut results, mut side) =
                    (p.prefs.explorer, p.prefs.results, p.prefs.side_panel);
                if ui.checkbox(&mut explorer, "Explorer").changed() {
                    p.toggle_explorer();
                }
                if ui.checkbox(&mut results, "Results panel").changed() {
                    p.toggle_results();
                }
                if ui.checkbox(&mut side, "History and AI panel").changed() {
                    p.toggle_side_panel();
                }
            });
            if let Some(crumb) = crumb {
                ui.add_space((ui.available_width() / 2.0 - 140.0).max(8.0));
                ui.label(RichText::new(crumb).color(pal.text_muted));
            }
        });
    });
}

/// Connection state on the left (with the latest message after it), engine and cursor on the
/// right.
fn status_bar(ui: &mut egui::Ui, p: &Presenter) {
    let pal = *theme::palette(ui);
    ui.horizontal(|ui| {
        let tab = p.selected_tab();
        let open = tab.and_then(|t| p.open.iter().find(|o| o.session == t.session));
        let profile = open.and_then(|o| p.connections.iter().find(|c| c.id == o.connection));
        match (open, profile) {
            (Some(o), Some(c)) => {
                ui.label(RichText::new("\u{2713} Connected").color(pal.success));
                let place = place_label(c);
                let db = database_label(c);
                let text = if db.is_empty() || c.driver_kind.as_str() == "sqlite" {
                    place
                } else {
                    format!("{place} / {db}")
                };
                ui.label(
                    RichText::new(if text.is_empty() {
                        o.name.clone()
                    } else {
                        text
                    })
                    .color(pal.text_muted),
                );
            }
            _ => {
                ui.label(RichText::new("Not connected").color(pal.text_muted));
            }
        }
        match &p.status {
            Some(Status::Info(m)) => {
                ui.separator();
                ui.label(RichText::new(m).color(pal.text_muted));
            }
            Some(Status::Error(m)) => {
                ui.separator();
                ui.label(RichText::new(m).color(pal.danger));
            }
            None => {}
        }
        if let Some(tab) = tab {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (line, col) = cursor_position(&tab.text, p.cursor_of(tab.id));
                ui.label(RichText::new(format!("Ln {line}, Col {col}")).color(pal.text_muted));
                ui.label(RichText::new("UTF-8").color(pal.text_muted));
                if let Some(c) = profile {
                    ui.label(
                        RichText::new(engine_name(c.driver_kind.as_str())).color(pal.text_muted),
                    );
                }
            });
        }
    });
}

/// The database to show next to an instance's name: a SQLite file by its file name, other
/// databases by their database name (empty when there is none).
pub(super) fn database_label(c: &domain::ConnectionProfile) -> String {
    let db = c.database.as_deref().unwrap_or("").trim();
    if c.driver_kind.as_str() == "sqlite" {
        std::path::Path::new(db)
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    } else {
        db.to_owned()
    }
}

/// Where an instance lives, for subtitles and the status bar: `host:port`, or the file name for
/// a SQLite file.
pub(super) fn place_label(c: &domain::ConnectionProfile) -> String {
    if c.driver_kind.as_str() == "sqlite" {
        return database_label(c);
    }
    match c.port {
        Some(port) => format!("{}:{port}", c.host),
        None => c.host.clone(),
    }
}

/// A readable engine name for the status bar.
fn engine_name(kind: &str) -> String {
    match kind {
        "postgres" => "PostgreSQL".into(),
        "mysql" => "MySQL".into(),
        "sqlite" => "SQLite".into(),
        "clickhouse" => "ClickHouse".into(),
        other => other.to_owned(),
    }
}

/// 1-based line and column of the byte offset `cursor` in `text` (columns count characters).
fn cursor_position(text: &str, cursor: usize) -> (usize, usize) {
    let end = cursor.min(text.len());
    let end = (0..=end)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    let before = &text[..end];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::cursor_position;

    #[test]
    fn places_and_databases_are_labelled_readably() {
        use domain::{ConnectionProfile, DriverKind};
        let mut pg = ConnectionProfile::new("x", DriverKind::new("postgres"), "db.local");
        pg.port = Some(5432);
        pg.database = Some("commerce".into());
        assert_eq!(super::place_label(&pg), "db.local:5432");
        assert_eq!(super::database_label(&pg), "commerce");
        pg.port = None;
        assert_eq!(super::place_label(&pg), "db.local");
        let mut lite = ConnectionProfile::new("y", DriverKind::new("sqlite"), "local");
        lite.database = Some("/data/shop/app.db".into());
        assert_eq!(
            super::place_label(&lite),
            "app.db",
            "a file is shown by its name"
        );
        assert_eq!(super::database_label(&lite), "app.db");
        lite.database = None;
        assert_eq!(super::place_label(&lite), "");
    }

    #[test]
    fn engine_names_are_readable_and_unknown_kinds_pass_through() {
        assert_eq!(super::engine_name("postgres"), "PostgreSQL");
        assert_eq!(super::engine_name("clickhouse"), "ClickHouse");
        assert_eq!(super::engine_name("duckdb"), "duckdb");
    }

    #[test]
    fn cursor_position_is_one_based() {
        assert_eq!(cursor_position("", 0), (1, 1));
        assert_eq!(cursor_position("select 1", 6), (1, 7));
        assert_eq!(cursor_position("a\nbc", 3), (2, 2));
    }

    #[test]
    fn cursor_position_counts_characters_not_bytes() {
        // "é" is two bytes; the cursor after it is column 2.
        assert_eq!(cursor_position("éx", 2), (1, 2));
    }

    #[test]
    fn cursor_position_tolerates_out_of_range_and_mid_character_offsets() {
        assert_eq!(cursor_position("ab", 99), (1, 3));
        assert_eq!(cursor_position("é", 1), (1, 1));
    }
}
