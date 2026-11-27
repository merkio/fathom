use eframe::egui;

use crate::presenter::Presenter;
use crate::theme;

enum Action {
    Connect(domain::ConnectionId),
    Edit(domain::ConnectionId),
    Delete(domain::ConnectionId),
}

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);
    ui.horizontal(|ui| {
        theme::section_label(ui, "Instances");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("+ New").clicked() {
                p.new_connection_form();
            }
        });
    });

    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("connections")
        .max_height(220.0)
        .show(ui, |ui| {
            if p.connections.is_empty() {
                ui.label(egui::RichText::new("No saved connections").color(pal.text_muted));
            }
            for c in &p.connections {
                let connecting = p.connecting.contains(&c.id);
                let is_open = p.open.iter().any(|o| o.connection == c.id);
                let port = c
                    .port
                    .map_or_else(|| "default".to_owned(), |p| p.to_string());
                let mut title = c.name.clone();
                if c.read_only() {
                    title.push_str(" (read-only)");
                }
                ui.horizontal(|ui| {
                    let dot = if is_open { pal.success } else { pal.text_faint };
                    ui.label(egui::RichText::new("\u{25CF}").color(dot).size(9.0));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(title).strong());
                        ui.label(
                            egui::RichText::new(format!(
                                "{} \u{00B7} {}:{port}",
                                c.driver_kind.as_str(),
                                c.host
                            ))
                            .size(11.0)
                            .color(pal.text_muted),
                        );
                        ui.horizontal(|ui| {
                            if connecting {
                                ui.spinner();
                            } else if !is_open && ui.small_button("Connect").clicked() {
                                action = Some(Action::Connect(c.id));
                            }
                            if ui.small_button("Edit").clicked() {
                                action = Some(Action::Edit(c.id));
                            }
                            if ui.small_button("Delete").clicked() {
                                action = Some(Action::Delete(c.id));
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });
    match action {
        Some(Action::Connect(id)) => p.connect(id),
        Some(Action::Edit(id)) => p.edit_connection(id),
        Some(Action::Delete(id)) => p.delete_connection(id),
        None => {}
    }
}

/// The create/edit dialog.
pub fn show_form(ctx: &egui::Context, p: &mut Presenter) {
    if p.form.is_none() {
        return;
    }
    let kinds: Vec<String> = p
        .driver_kinds()
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect();
    let testing = p.testing;
    let (mut save, mut test, mut cancel) = (false, false, false);

    if let Some(form) = p.form.as_mut() {
        let title = if form.is_editing() {
            "Edit connection"
        } else {
            "New connection"
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("connection_form")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut form.name);
                        ui.end_row();

                        ui.label("Database type");
                        egui::ComboBox::from_id_salt("driver_kind")
                            .selected_text(form.driver_kind.clone())
                            .show_ui(ui, |ui| {
                                for kind in &kinds {
                                    ui.selectable_value(&mut form.driver_kind, kind.clone(), kind);
                                }
                            });
                        ui.end_row();

                        ui.label("Host");
                        ui.text_edit_singleline(&mut form.host);
                        ui.end_row();

                        ui.label("Port");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.port).hint_text("driver default"),
                        );
                        ui.end_row();

                        ui.label("Database");
                        ui.text_edit_singleline(&mut form.database);
                        ui.end_row();

                        ui.label("User");
                        ui.text_edit_singleline(&mut form.username);
                        ui.end_row();

                        ui.label("Read-only");
                        ui.checkbox(
                            &mut form.read_only,
                            "Only allow plain reads (blocks writes and DDL)",
                        );
                        ui.end_row();

                        ui.label("AI inline");
                        egui::ComboBox::from_id_salt("inline_override")
                            .selected_text(match form.inline_ai {
                                None => "Use the global setting",
                                Some(true) => "On for this connection",
                                Some(false) => "Off for this connection",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut form.inline_ai,
                                    None,
                                    "Use the global setting",
                                );
                                ui.selectable_value(
                                    &mut form.inline_ai,
                                    Some(true),
                                    "On for this connection",
                                );
                                ui.selectable_value(
                                    &mut form.inline_ai,
                                    Some(false),
                                    "Off for this connection",
                                );
                            });
                        ui.end_row();

                        ui.label("Advanced");
                ui.add(
                    egui::TextEdit::multiline(&mut form.extra_options)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("key=value per line, e.g. sslmode=verify-full\nsslrootcert=/path/to/ca.pem"),
                );
                ui.end_row();

                ui.label("Password");
                        let hint = if form.is_editing() { "unchanged" } else { "" };
                        ui.add(
                            egui::TextEdit::singleline(&mut form.password)
                                .password(true)
                                .hint_text(hint),
                        );
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    save = ui.button("Save").clicked();
                    test = ui
                        .add_enabled(!testing, egui::Button::new("Test connection"))
                        .clicked();
                    if testing {
                        ui.spinner();
                    }
                    cancel = ui.button("Cancel").clicked();
                });
            });
    }
    if save {
        p.save_form();
    } else if test {
        p.test_form();
    } else if cancel {
        p.cancel_form();
    }
}
