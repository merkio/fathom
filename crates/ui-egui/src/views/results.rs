use eframe::egui;
use egui_extras::{Column, TableBuilder};

use application::export::ExportFormat;

use crate::presenter::{Presenter, TabState};

const ROW_HEIGHT: f32 = 20.0;

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let mut fix = false;
    let Some(tab) = p.selected_tab() else { return };
    match &tab.state {
        TabState::Idle => {
            ui.weak("Run a query to see results");
        }
        TabState::Running => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running...");
            });
        }
        TabState::Failed(message) => {
            ui.colored_label(ui.visuals().error_fg_color, message);
            fix = ui
                .button("Fix with AI")
                .on_hover_text(
                    "Sends the query, the error and your schema (no data) to your AI provider",
                )
                .clicked();
        }
        TabState::Done(result) => {
            if result.columns.is_empty() {
                let n = result.rows_affected.unwrap_or(0);
                ui.label(format!(
                    "{n} row(s) affected in {} ms",
                    result.elapsed.as_millis()
                ));
                return;
            }
            let mut summary = format!(
                "{} row(s) in {} ms",
                result.rows.len(),
                result.elapsed.as_millis()
            );
            if result.truncated {
                summary.push_str(" (limited: more rows exist; Export saves more)");
            }
            ui.weak(summary);

            // Only the visible rows are laid out, so large results stay responsive.
            TableBuilder::new(ui)
                .id_salt(("results", tab.id.0))
                .striped(true)
                .resizable(true)
                .max_scroll_height(f32::INFINITY)
                .columns(
                    Column::initial(140.0).at_least(40.0).clip(true),
                    result.columns.len(),
                )
                .header(ROW_HEIGHT, |mut header| {
                    for col in &result.columns {
                        header.col(|ui| {
                            ui.strong(&col.name).on_hover_text(&col.data_type.0);
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_HEIGHT, result.rows.len(), |mut row| {
                        for value in &result.rows[row.index()] {
                            row.col(|ui| {
                                if value.is_null() {
                                    ui.weak("NULL");
                                } else {
                                    ui.add(egui::Label::new(value.display()).truncate());
                                }
                            });
                        }
                    });
                });
        }
    }
    if fix {
        p.ai_fix_last_error();
    }
}

/// Export controls above the grid: format, file name and the button. Shown only for a result
/// with columns. The export re-runs the statement, so it can hold more rows than the grid.
pub fn export_controls(ui: &mut egui::Ui, p: &mut Presenter) {
    let exportable = p
        .selected_tab()
        .is_some_and(|t| matches!(&t.state, TabState::Done(r) if !r.columns.is_empty()));
    if !exportable {
        return;
    }
    let mut export = false;
    ui.horizontal(|ui| {
        ui.label("Export");
        egui::ComboBox::from_id_salt("export_format")
            .width(70.0)
            .selected_text(match p.export.format {
                ExportFormat::Csv => "CSV",
                ExportFormat::Json => "JSON",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut p.export.format, ExportFormat::Csv, "CSV");
                ui.selectable_value(&mut p.export.format, ExportFormat::Json, "JSON");
            });
        ui.add(
            egui::TextEdit::singleline(&mut p.export.name)
                .hint_text("file name (optional)")
                .desired_width(170.0),
        );
        export = ui
            .add_enabled(!p.export.running, egui::Button::new("Save"))
            .on_hover_text(
                "Re-runs the query and writes a new file. Existing files are never overwritten.",
            )
            .clicked();
        if p.export.running {
            ui.spinner();
        }
    });
    if export {
        p.export_selected();
    }
}
