//! Views: pure egui drawing. They read presenter state and call presenter methods; they contain
//! no business logic.

mod ai;
mod code_editor;
mod connections;
mod editor;
mod export;
mod history;
mod instances;
mod palette;
mod results;
mod schema;
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
            .default_size(280.0)
            .min_size(220.0)
            .show(ui, |ui| {
                connections::show(ui, p);
                ui.add_space(6.0);
                schema::show(ui, p);
            });
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
    instances::show(ui.ctx(), p);
    palette::show(ui.ctx(), p);
}

/// Window-wide shortcuts. The palette owns the keyboard while it is open.
fn shortcuts(ui: &egui::Ui, p: &mut Presenter) {
    let cmd = Modifiers::COMMAND;
    let (mut open, mut focus, mut explorer, mut results, mut side, mut export) =
        (false, false, false, false, false, false);
    ui.input_mut(|i| {
        // The shifted shortcut first: `Cmd+F` must not swallow `Cmd+Shift+F`.
        focus = i.consume_shortcut(&KeyboardShortcut::new(cmd | Modifiers::SHIFT, Key::F));
        export = i.consume_shortcut(&KeyboardShortcut::new(cmd | Modifiers::SHIFT, Key::E));
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

/// App name, the open query's breadcrumb, the theme switch and the layout toggles.
fn header(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);
    ui.horizontal(|ui| {
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
            let next = match p.prefs.theme {
                ThemePreference::System => ThemePreference::Light,
                ThemePreference::Light => ThemePreference::Dark,
                ThemePreference::Dark => ThemePreference::System,
            };
            let name = match p.prefs.theme {
                ThemePreference::System => "Theme: auto",
                ThemePreference::Light => "Theme: light",
                ThemePreference::Dark => "Theme: dark",
            };
            if ui
                .button(name)
                .on_hover_text("Switch between light, dark and the system theme")
                .clicked()
            {
                p.set_theme(next);
            }
            if ui
                .button(format!(
                    "Search  {}",
                    ui.ctx()
                        .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K))
                ))
                .on_hover_text("Open the command palette")
                .clicked()
            {
                p.open_palette();
            }
            ui.menu_button("Layout", |ui| {
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
                ui.add_space(ui.available_width() / 2.0 - 120.0);
                ui.label(RichText::new(crumb).color(pal.text_muted));
            }
        });
    });
}

fn status_bar(ui: &mut egui::Ui, p: &Presenter) {
    let pal = *theme::palette(ui);
    ui.horizontal(|ui| {
        match &p.status {
            Some(Status::Info(m)) => {
                ui.label(RichText::new(m).color(pal.text_muted));
            }
            Some(Status::Error(m)) => {
                ui.label(RichText::new(m).color(pal.danger));
            }
            None => {
                let connected = p
                    .selected_tab()
                    .and_then(|t| p.open.iter().find(|o| o.session == t.session));
                match connected {
                    Some(o) => {
                        ui.label(RichText::new("Connected").color(pal.success));
                        ui.label(RichText::new(&o.name).color(pal.text_muted));
                    }
                    None => {
                        ui.label(RichText::new("Ready").color(pal.text_muted));
                    }
                }
            }
        }
        if let Some(tab) = p.selected_tab() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (line, col) = cursor_position(&tab.text, p.cursor_of(tab.id));
                ui.label(RichText::new(format!("Ln {line}, Col {col}")).color(pal.text_muted));
                ui.label(RichText::new("UTF-8").color(pal.text_muted));
            });
        }
    });
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
