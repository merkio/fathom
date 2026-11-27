use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Panel, RichText, Stroke};

use crate::bridge::TabId;
use crate::presenter::{Presenter, TabState};
use crate::theme;

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    tab_bar(ui, p);
    let Some(id) = p.selected else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.weak("Open a connection to start writing queries");
        });
        return;
    };

    let running = matches!(p.selected_tab().map(|t| &t.state), Some(TabState::Running));
    let (mut run, mut cancel, mut save) = (false, false, false);
    let shortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let pal = *theme::palette(ui);
        if let Some(tab) = p.selected_tab()
            && let Some(open) = p.open.iter().find(|o| o.session == tab.session)
        {
            let database = p
                .connections
                .iter()
                .find(|c| c.id == open.connection)
                .map(super::database_label)
                .filter(|d| !d.is_empty());
            match database {
                Some(db) => {
                    ui.label(RichText::new(&open.name).strong());
                    ui.label(RichText::new("/").color(pal.text_faint));
                    ui.label(RichText::new(db).color(pal.text_muted));
                }
                None => {
                    ui.label(RichText::new(&open.name).strong());
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let label = format!("Run query   {}", ui.ctx().format_shortcut(&shortcut));
            run = ui
                .add_enabled(!running, theme::primary_button(label))
                .clicked();
            save = ui
                .button("Save")
                .on_hover_text("Keep this query under a name (Ctrl+S)")
                .clicked();
            if running {
                cancel = ui.button("Cancel").clicked();
                ui.spinner();
                ui.label(RichText::new("Running...").color(pal.text_muted));
            }
        });
    });
    ui.add_space(6.0);
    if !running && ui.input_mut(|i| i.consume_shortcut(&shortcut)) {
        run = true;
    }

    if p.prefs.results {
        Panel::bottom("results_panel")
            .resizable(true)
            .default_size((ui.available_height() * 0.55).max(260.0))
            .min_size(120.0)
            .show(ui, |ui| {
                if p.selected_tab().is_some_and(|t| t.grid.has_pending()) {
                    Panel::bottom("pending_bar_panel")
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| super::results::pending_bar(ui, p));
                }
                Panel::bottom("results_footer_panel")
                    .frame(egui::Frame::NONE)
                    .show_separator_line(false)
                    .show(ui, |ui| super::results::footer(ui, p));
                super::results::show(ui, p);
            });
    }

    Panel::bottom("ai_card_panel")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 8)))
        .show_separator_line(false)
        .show(ui, |ui| super::ai_card::show(ui, p));

    let height = ui.available_height();
    egui::ScrollArea::vertical()
        .id_salt(("editor", id.0))
        .auto_shrink([false, false])
        .show(ui, |ui| super::code_editor::show(ui, p, id, height));

    if save {
        p.save_current();
    }
    if run {
        p.run(id);
    }
    if cancel {
        p.cancel(id);
    }
}

fn tab_bar(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);
    let mut select: Option<TabId> = None;
    let mut close: Option<TabId> = None;
    let mut add = false;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(8.0);
        for tab in &p.tabs {
            let selected = p.selected == Some(tab.id);
            let running = matches!(tab.state, TabState::Running);
            let mut title = if running {
                format!("{} ...", tab.title)
            } else {
                tab.title.clone()
            };
            if p.is_dirty(tab.id) {
                title = format!("\u{25CF} {title}");
            }
            ui.label(RichText::new("</>").monospace().color(if selected {
                pal.accent
            } else {
                pal.text_faint
            }));
            let text = RichText::new(title).color(if selected { pal.text } else { pal.text_muted });
            let response = ui.add(egui::Button::new(text).frame(false));
            if response.clicked() {
                select = Some(tab.id);
            }
            if selected {
                let r = response.rect;
                ui.painter().line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    Stroke::new(2.0, pal.accent),
                );
            }
            if ui
                .add(egui::Button::new(RichText::new("x").color(pal.text_faint)).frame(false))
                .on_hover_text("Close tab")
                .clicked()
            {
                close = Some(tab.id);
            }
            ui.add_space(6.0);
        }
        let session = p
            .selected_tab()
            .map(|t| t.session)
            .or_else(|| p.open.first().map(|o| o.session));
        if session.is_some()
            && ui
                .add(egui::Button::new("+").frame(false))
                .on_hover_text("New query tab")
                .clicked()
        {
            add = true;
        }
    });
    ui.separator();
    if let Some(id) = select {
        p.select_tab(id);
    }
    if let Some(id) = close {
        p.close_tab(id);
    }
    if add
        && let Some(session) = p
            .selected_tab()
            .map(|t| t.session)
            .or_else(|| p.open.first().map(|o| o.session))
    {
        p.new_tab(session);
    }
}
