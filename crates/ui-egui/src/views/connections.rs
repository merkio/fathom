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
            if ui
                .small_button("Manage...")
                .on_hover_text("Choose which databases each instance shows")
                .clicked()
            {
                p.open_instances(None);
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
                        if let Some((shown, total)) = p.shown_databases(c.id)
                            && shown < total
                        {
                            ui.label(
                                egui::RichText::new(format!("{shown} of {total} databases shown"))
                                    .size(11.0)
                                    .color(pal.text_muted),
                            );
                        }
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

/// The create/edit dialog: a Connection tab and an Advanced / TLS tab.
pub fn show_form(ctx: &egui::Context, p: &mut Presenter) {
    if p.form.is_none() {
        return;
    }
    let pal = *theme::palette_for(ctx.theme());
    let kinds: Vec<String> = p
        .driver_kinds()
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect();
    let testing = p.testing;
    let (mut save, mut test, mut cancel, mut choose) = (false, false, false, false);
    if p.palette.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        cancel = true;
    }

    if let Some(form) = p.form.as_mut() {
        let editing = form.is_editing();
        egui::Window::new("connection_form_window")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([620.0, 0.0])
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg)
                    .stroke(egui::Stroke::new(1.0, pal.border))
                    .corner_radius(egui::CornerRadius::same(12))
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                let title = if editing {
                    "Edit database instance"
                } else {
                    "Add database instance"
                };
                ui.label(egui::RichText::new(title).size(20.0).strong());
                ui.label(
                    egui::RichText::new(
                        "Save a connection, then select the databases you want to see.",
                    )
                    .color(pal.text_muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut form.show_advanced, false, "Connection");
                    ui.selectable_value(&mut form.show_advanced, true, "TLS / SSL and options");
                });
                ui.separator();
                if form.show_advanced {
                    advanced_tab(ui, form, &pal);
                } else {
                    connection_tab(ui, form, &kinds, editing, &pal);
                }
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    test = ui
                        .add_enabled(!testing, egui::Button::new("Test connection"))
                        .clicked();
                    if testing {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if editing {
                            save = ui.add(theme::primary_button("Save")).clicked();
                        } else {
                            choose = ui
                                .add(theme::primary_button("Connect & choose databases"))
                                .clicked();
                            save = ui.button("Save only").clicked();
                        }
                        cancel |= ui.button("Cancel").clicked();
                    });
                });
            });
    }
    if choose {
        p.save_form_and_choose();
    } else if save {
        p.save_form();
    } else if test {
        p.test_form();
    } else if cancel {
        p.cancel_form();
    }
}

fn field(ui: &mut egui::Ui, label: &str, pal: &theme::Palette) {
    ui.label(egui::RichText::new(label).size(12.0).color(pal.text_muted));
}

fn connection_tab(
    ui: &mut egui::Ui,
    form: &mut crate::form::ConnectionForm,
    kinds: &[String],
    editing: bool,
    pal: &theme::Palette,
) {
    field(ui, "Database engine", pal);
    egui::ComboBox::from_id_salt("driver_kind")
        .selected_text(form.driver_kind.clone())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for kind in kinds {
                ui.selectable_value(&mut form.driver_kind, kind.clone(), kind);
            }
        });
    field(ui, "Instance name", pal);
    ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(f32::INFINITY));
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            field(ui, "Host", pal);
            ui.add(egui::TextEdit::singleline(&mut form.host).desired_width(380.0));
        });
        ui.vertical(|ui| {
            field(ui, "Port", pal);
            ui.add(
                egui::TextEdit::singleline(&mut form.port)
                    .hint_text("driver default")
                    .desired_width(150.0),
            );
        });
    });
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            field(ui, "Database (or file for SQLite)", pal);
            ui.add(egui::TextEdit::singleline(&mut form.database).desired_width(280.0));
        });
        ui.vertical(|ui| {
            field(ui, "Username", pal);
            ui.add(egui::TextEdit::singleline(&mut form.username).desired_width(250.0));
        });
    });
    field(ui, "Password", pal);
    let hint = if editing { "unchanged" } else { "" };
    ui.add(
        egui::TextEdit::singleline(&mut form.password)
            .password(true)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
    ui.label(
        egui::RichText::new("Saved in the system keychain, never in the settings file.")
            .size(11.0)
            .color(pal.text_muted),
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut form.read_only,
        "Read-only: only allow plain reads (blocks writes and DDL)",
    );
    ui.horizontal(|ui| {
        ui.label("AI inline suggestions");
        egui::ComboBox::from_id_salt("inline_override")
            .selected_text(match form.inline_ai {
                None => "Use the global setting",
                Some(true) => "On for this connection",
                Some(false) => "Off for this connection",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut form.inline_ai, None, "Use the global setting");
                ui.selectable_value(&mut form.inline_ai, Some(true), "On for this connection");
                ui.selectable_value(&mut form.inline_ai, Some(false), "Off for this connection");
            });
    });
}

fn advanced_tab(ui: &mut egui::Ui, form: &mut crate::form::ConnectionForm, pal: &theme::Palette) {
    field(ui, "Driver options (one key=value per line)", pal);
    ui.add(
        egui::TextEdit::multiline(&mut form.extra_options)
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .hint_text("sslmode=verify-full\nsslrootcert=/path/to/ca.pem"),
    );
    ui.label(
        egui::RichText::new(
            "TLS: sslmode (disable, prefer, require, verify-ca, verify-full), sslrootcert, sslcert, \
             sslkey. ClickHouse: secure=true. SQLite: mode=ro, create=true.",
        )
        .size(11.0)
        .color(pal.text_muted),
    );
}
