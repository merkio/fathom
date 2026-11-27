use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use crate::presenter::{Presenter, StorageForm, TestState};
use crate::theme;

use super::export::kind_label;

/// The "Storage connections" dialog: the destinations from the configuration file, with a
/// connection test. They are edited in the file, not here (credentials stay out of the app).
pub fn show(ctx: &egui::Context, p: &mut Presenter) {
    let Some(selected) = p.storage_dialog.clone() else {
        return;
    };
    let pal = *theme::palette_for(ctx.theme());
    let escape = p.palette.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut pick: Option<String> = None;
    let mut test: Option<String> = None;
    let mut close = false;
    let mut actions = FormActions::default();

    egui::Window::new("storage_connections")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([820.0, 0.0])
        .frame(
            Frame::NONE
                .fill(pal.bg)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(egui::CornerRadius::same(12))
                .shadow(ctx.global_style().visuals.popup_shadow)
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.label(RichText::new("Storage connections").size(20.0).strong());
            ui.label(
                RichText::new("Reusable destinations for file exports. Separate from your database connections.")
                    .color(pal.text_muted),
            );
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    ui.set_min_height(260.0);
                    ui.horizontal(|ui| {
                        theme::section_label(ui, &format!("{} destinations", p.destinations.len()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.menu_button("+ Add", |ui| {
                                for kind in &p.storage_kinds {
                                    if ui.button(application::usecases::form_for(kind).0).clicked() {
                                        actions.add = Some(kind.clone());
                                        ui.close();
                                    }
                                }
                                if p.storage_kinds.is_empty() {
                                    ui.label("No adapters are enabled.");
                                }
                            });
                        });
                    });
                    for d in &p.destinations {
                        let active = selected.as_deref() == Some(d.name.as_str());
                        let fill = if active { pal.accent_soft } else { egui::Color32::TRANSPARENT };
                        let r = Frame::NONE
                            .fill(fill)
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(&d.name).color(if active { pal.accent } else { pal.text }));
                                ui.label(RichText::new(kind_label(&d.kind)).size(11.0).color(pal.text_muted));
                            })
                            .response
                            .interact(egui::Sense::click());
                        if r.clicked() {
                            pick = Some(d.name.clone());
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    if let Some(form) = p.storage_form.as_mut() {
                        form_ui(ui, form, &pal, &mut actions);
                        return;
                    }
                    let Some(d) = selected
                        .as_ref()
                        .and_then(|n| p.destinations.iter().find(|d| &d.name == n))
                    else {
                        ui.label(RichText::new("No storage connections yet. Use “+ Add” to create one.").color(pal.text_muted));
                        return;
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&d.name).size(16.0).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (text, color) = match p.destination_tests.get(&d.name) {
                                None => ("Not tested".to_owned(), pal.text_muted),
                                Some(TestState::Testing) => ("Testing...".to_owned(), pal.text_muted),
                                Some(TestState::Ok) => ("Connected".to_owned(), pal.success),
                                Some(TestState::Failed(_)) => ("Failed".to_owned(), pal.danger),
                            };
                            ui.label(RichText::new(text).color(color));
                        });
                    });
                    ui.add_space(6.0);
                    let source = if d.editable { "Saved in this app" } else { "fathom.toml (read-only)" };
                    for (label, value) in [("Type", kind_label(&d.kind)), ("Location", d.location.clone()), ("Source", source.to_owned())] {
                        ui.horizontal(|ui| {
                            ui.add_sized([90.0, 18.0], egui::Label::new(RichText::new(label).color(pal.text_muted)));
                            ui.label(value);
                        });
                    }
                    if let Some(TestState::Failed(message)) = p.destination_tests.get(&d.name) {
                        ui.add_space(6.0);
                        ui.label(RichText::new(message).color(pal.danger));
                    }
                    ui.add_space(10.0);
                    let testing = matches!(p.destination_tests.get(&d.name), Some(TestState::Testing));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!testing, egui::Button::new("Test connection"))
                            .on_hover_text("Writes and removes a tiny probe file")
                            .clicked()
                        {
                            test = Some(d.name.clone());
                        }
                        if d.editable {
                            if ui.button("Edit").clicked() {
                                actions.edit = Some(d.name.clone());
                            }
                            if ui.button(RichText::new("Delete").color(pal.danger)).clicked() {
                                actions.delete = Some(d.name.clone());
                            }
                        }
                    });
                });
            });
            if !p.destination_problems.is_empty() {
                ui.add_space(6.0);
                Frame::NONE
                    .fill(pal.warning_soft)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new("Problems in the configuration").color(pal.warning).strong());
                        for problem in &p.destination_problems {
                            ui.label(RichText::new(problem).size(11.0).color(pal.warning));
                        }
                    });
            }
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Secrets are kept in your system keychain and never shown. Connections from fathom.toml are read-only.")
                        .size(11.0)
                        .color(pal.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button("Close").clicked();
                });
            });
        });

    if let Some(kind) = actions.add {
        p.new_storage_form(&kind);
    }
    if let Some(name) = actions.edit {
        p.edit_storage_form(&name);
    }
    if let Some(name) = actions.delete {
        p.delete_storage(&name);
    }
    if actions.test {
        p.test_storage_form();
    }
    if actions.save {
        p.submit_storage_form();
    }
    if actions.cancel {
        p.cancel_storage_form();
    }
    if let Some(name) = pick {
        p.select_storage(&name);
    }
    if let Some(name) = test {
        p.test_destination(&name);
    }
    if escape && p.storage_form.is_some() {
        p.cancel_storage_form();
    } else if close || escape {
        p.close_storage_dialog();
    }
}

#[derive(Default)]
struct FormActions {
    add: Option<String>,
    edit: Option<String>,
    delete: Option<String>,
    test: bool,
    save: bool,
    cancel: bool,
}

fn form_ui(
    ui: &mut egui::Ui,
    form: &mut StorageForm,
    pal: &theme::Palette,
    actions: &mut FormActions,
) {
    let (kind_title, fields) = application::usecases::form_for(&form.kind);
    ui.label(
        RichText::new(if form.original.is_some() {
            "Edit connection"
        } else {
            "New connection"
        })
        .size(16.0)
        .strong(),
    );
    ui.label(RichText::new(kind_title).color(pal.text_muted));
    ui.add_space(6.0);
    egui::Grid::new("storage_form_fields")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Connection name");
            ui.add(
                egui::TextEdit::singleline(&mut form.name)
                    .desired_width(360.0)
                    .hint_text("Analytics archive"),
            );
            ui.end_row();
            for f in &fields {
                ui.label(if f.required {
                    format!("{} *", f.label)
                } else {
                    f.label.to_owned()
                });
                if f.secret {
                    let value = form.secrets.entry(f.key.to_owned()).or_default();
                    let hint = if form.stored_secrets {
                        "unchanged (type to replace)"
                    } else {
                        f.hint
                    };
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .password(true)
                            .desired_width(360.0)
                            .hint_text(hint),
                    );
                } else {
                    let value = form.settings.entry(f.key.to_owned()).or_default();
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .desired_width(360.0)
                            .hint_text(f.hint),
                    );
                }
                ui.end_row();
            }
        });
    if let Some(error) = &form.error {
        ui.add_space(6.0);
        ui.label(RichText::new(error).color(pal.danger));
    }
    match &form.test {
        Some(TestState::Ok) => {
            ui.label(
                RichText::new("Connected: a probe file was written and removed.")
                    .color(pal.success),
            );
        }
        Some(TestState::Failed(m)) => {
            ui.label(RichText::new(m).color(pal.danger));
        }
        Some(TestState::Testing) => {
            ui.label(RichText::new("Testing...").color(pal.text_muted));
        }
        None => {}
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let testing = matches!(form.test, Some(TestState::Testing));
        if ui
            .add_enabled(!testing, egui::Button::new("Test connection"))
            .clicked()
        {
            actions.test = true;
        }
        if ui
            .add_enabled(!form.busy, theme::primary_button("Save"))
            .clicked()
        {
            actions.save = true;
        }
        if ui.button("Cancel").clicked() {
            actions.cancel = true;
        }
    });
}
