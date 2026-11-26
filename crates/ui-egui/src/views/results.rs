use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::presenter::{Presenter, TabState};

const ROW_HEIGHT: f32 = 20.0;

pub fn show(ui: &mut egui::Ui, p: &Presenter) {
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
                summary.push_str(" (more rows exist; result limited)");
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
}
