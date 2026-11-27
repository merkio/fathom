use application::export::ExportFormat;
use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use crate::presenter::Presenter;
use crate::theme;

/// A readable name for an adapter kind.
pub fn kind_label(kind: &str) -> String {
    match kind {
        "local" => "Local folder".into(),
        "s3" => "Amazon S3".into(),
        "smb" => "SMB".into(),
        "nfs" => "NFS".into(),
        other => other.to_uppercase(),
    }
}

const FORMATS: &[(ExportFormat, &str)] = &[
    (ExportFormat::Csv, "CSV"),
    (ExportFormat::Json, "JSON"),
    (ExportFormat::Sql, "SQL"),
    (ExportFormat::Parquet, "Parquet"),
];

/// The "Export results" dialog. Draws nothing unless it was opened.
pub fn show(ctx: &egui::Context, p: &mut Presenter) {
    if !p.export.dialog_open {
        return;
    }
    let pal = *theme::palette_for(ctx.theme());
    let escape = p.palette.is_none()
        && p.storage_dialog.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let (mut export, mut cancel, mut manage) = (false, false, false);
    let mut choose: Option<String> = None;
    let mut name = p.export.name.clone();
    let mut folder = p.export.folder.clone();
    let mut format = p.export.format;
    let mut selected_only = p.export.selected_only;
    let ticked = p.selected_row_count();
    let hint = p.export_row_hint();
    let running = p.export.running;
    let current = p
        .export
        .destination
        .as_ref()
        .and_then(|n| p.destinations.iter().find(|d| &d.name == n))
        .cloned();
    let kinds: Vec<String> = {
        let mut k: Vec<String> = Vec::new();
        for d in &p.destinations {
            if !k.contains(&d.kind) {
                k.push(d.kind.clone());
            }
        }
        k
    };

    egui::Window::new("export_results")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([640.0, 0.0])
        .frame(
            Frame::NONE
                .fill(pal.bg)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(egui::CornerRadius::same(12))
                .shadow(ctx.global_style().visuals.popup_shadow)
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.label(RichText::new("Export results").size(20.0).strong());
            ui.label(
                RichText::new("Take your query results wherever you work.").color(pal.text_muted),
            );
            ui.add_space(8.0);

            theme::section_label(ui, "Destination");
            if p.destinations.is_empty() {
                ui.label(
                    RichText::new(
                        "No destinations are configured. Add them to fathom.toml (see the README).",
                    )
                    .color(pal.danger),
                );
            }
            ui.horizontal(|ui| {
                for kind in &kinds {
                    let active = current.as_ref().is_some_and(|c| &c.kind == kind);
                    if ui.selectable_label(active, kind_label(kind)).clicked()
                        && let Some(first) = p.destinations.iter().find(|d| &d.kind == kind)
                    {
                        choose = Some(first.name.clone());
                    }
                }
            });
            if let Some(cur) = &current {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Saved connection")
                            .size(12.0)
                            .color(pal.text_muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.link("Manage connections...").clicked() {
                            manage = true;
                        }
                    });
                });
                egui::ComboBox::from_id_salt("export_destination")
                    .selected_text(format!("{}  ({})", cur.name, kind_label(&cur.kind)))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for d in p.destinations.iter().filter(|d| d.kind == cur.kind) {
                            if ui.selectable_label(d.name == cur.name, &d.name).clicked() {
                                choose = Some(d.name.clone());
                            }
                        }
                    });
                ui.label(
                    RichText::new(format!("Files go to {}", cur.location))
                        .size(11.0)
                        .color(pal.text_muted),
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Folder path")
                            .size(12.0)
                            .color(pal.text_muted),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut folder)
                            .hint_text("exports/customers/2026-11-27/")
                            .desired_width(330.0),
                    );
                });
                ui.vertical(|ui| {
                    ui.label(RichText::new("File name").size(12.0).color(pal.text_muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .hint_text(format!("fathom-export.{}", format.extension()))
                            .desired_width(250.0),
                    );
                });
            });
            if let Some(cur) = &current
                && let Ok(key) = application::usecases::export_key(&folder, &name, format)
            {
                ui.label(
                    RichText::new(format!(
                        "{}{key}",
                        cur.location.trim_end_matches('/').to_owned() + "/"
                    ))
                    .size(11.0)
                    .color(pal.text_muted),
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    theme::section_label(ui, "Format");
                    ui.horizontal(|ui| {
                        for (f, label) in FORMATS {
                            ui.selectable_value(&mut format, *f, *label);
                        }
                    });
                });
                ui.add_space(40.0);
                ui.vertical(|ui| {
                    theme::section_label(ui, "Rows");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut selected_only, false, format!("All ({hint}+)"));
                        ui.add_enabled_ui(ticked > 0, |ui| {
                            ui.selectable_value(
                                &mut selected_only,
                                true,
                                format!("Selected rows ({ticked})"),
                            );
                        });
                    });
                });
            });

            ui.add_space(8.0);
            Frame::NONE
                .fill(pal.surface)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Exports the last executed result.").strong());
                    ui.label(
                        RichText::new(
                            "Pending edits are not included. Existing files are never overwritten.",
                        )
                        .size(11.0)
                        .color(pal.text_muted),
                    );
                    if format == ExportFormat::Sql {
                        ui.label(
                            RichText::new(
                                "SQL files hold INSERT statements; Fathom never runs them.",
                            )
                            .size(11.0)
                            .color(pal.text_muted),
                        );
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = if selected_only { ticked } else { hint };
                    export = ui
                        .add_enabled(
                            !running && current.is_some(),
                            theme::primary_button(format!("Export {n} rows")),
                        )
                        .clicked();
                    cancel = ui
                        .add_enabled(!running, egui::Button::new("Cancel"))
                        .clicked();
                    if running {
                        ui.spinner();
                    }
                });
            });
        });

    p.export.name = name;
    p.export.folder = folder;
    p.export.format = format;
    p.export.selected_only = selected_only && ticked > 0;
    if let Some(name) = choose {
        p.choose_destination(&name);
    }
    if manage {
        p.open_storage_dialog();
    }
    if export {
        p.start_export();
    } else if cancel || escape {
        p.close_export_dialog();
    }
}
