use eframe::egui::{self, Key, Modifiers, Panel};

use crate::bridge::TabId;
use crate::presenter::{Presenter, TabState};

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
    let (mut run, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        run = ui
            .add_enabled(!running, egui::Button::new("Run  (Ctrl+Enter)"))
            .clicked();
        if running {
            cancel = ui.button("Cancel").clicked();
            ui.spinner();
        }
    });
    if !running && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
        run = true;
    }

    Panel::bottom("results_panel")
        .resizable(true)
        .default_size(300.0)
        .min_size(120.0)
        .show(ui, |ui| super::results::show(ui, p));

    let height = ui.available_height();
    egui::ScrollArea::vertical()
        .id_salt(("editor", id.0))
        .auto_shrink([false, false])
        .show(ui, |ui| super::code_editor::show(ui, p, id, height));

    if run {
        p.run(id);
    }
    if cancel {
        p.cancel(id);
    }
}

fn tab_bar(ui: &mut egui::Ui, p: &mut Presenter) {
    let mut select: Option<TabId> = None;
    let mut close: Option<TabId> = None;
    ui.horizontal_wrapped(|ui| {
        for tab in &p.tabs {
            let selected = p.selected == Some(tab.id);
            let running = matches!(tab.state, TabState::Running);
            let label = if running {
                format!("{} ...", tab.title)
            } else {
                tab.title.clone()
            };
            if ui.selectable_label(selected, label).clicked() {
                select = Some(tab.id);
            }
            if ui.small_button("x").on_hover_text("Close tab").clicked() {
                close = Some(tab.id);
            }
            ui.add_space(6.0);
        }
    });
    if let Some(id) = select {
        p.select_tab(id);
    }
    if let Some(id) = close {
        p.close_tab(id);
    }
}
