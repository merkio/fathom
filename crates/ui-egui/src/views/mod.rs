//! Views: pure egui drawing. They read presenter state and call presenter methods; they contain
//! no business logic.

mod ai;
mod code_editor;
mod connections;
mod editor;
mod history;
mod results;
mod schema;

use eframe::egui::{self, Panel};

use crate::presenter::{Presenter, RightPanel, Status};

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    Panel::bottom("status").show(ui, |ui| status_bar(ui, p));
    Panel::left("sidebar")
        .default_size(300.0)
        .min_size(220.0)
        .show(ui, |ui| {
            connections::show(ui, p);
            ui.separator();
            schema::show(ui, p);
        });
    Panel::right("right_panel")
        .default_size(340.0)
        .min_size(220.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut p.right_panel, RightPanel::History, "History");
                ui.selectable_value(&mut p.right_panel, RightPanel::Ai, "AI");
            });
            ui.separator();
            match p.right_panel {
                RightPanel::History => history::show(ui, p),
                RightPanel::Ai => ai::show(ui, p),
            }
        });
    egui::CentralPanel::default().show(ui, |ui| editor::show(ui, p));
    connections::show_form(ui.ctx(), p);
}

fn status_bar(ui: &mut egui::Ui, p: &Presenter) {
    ui.horizontal(|ui| match &p.status {
        Some(Status::Info(m)) => {
            ui.label(m);
        }
        Some(Status::Error(m)) => {
            ui.colored_label(ui.visuals().error_fg_color, m);
        }
        None => {
            ui.weak("Ready");
        }
    });
}
