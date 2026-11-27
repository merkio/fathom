use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use domain::ConnectionId;

use crate::presenter::{InstanceTab, Presenter};
use crate::theme;

/// The "Database instances" dialog. Draws nothing while closed.
pub fn show(ctx: &egui::Context, p: &mut Presenter) {
    let Some(state) = p.instances.clone() else {
        return;
    };
    let pal = *theme::palette_for(ctx.theme());
    // Esc closes the topmost layer: the palette and the connection form sit above this dialog.
    let escape = p.palette.is_none()
        && p.form.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

    let mut select: Option<ConnectionId> = None;
    let (mut save, mut cancel, mut add, mut edit) = (false, false, false, None);
    let (mut refresh, mut select_all) = (false, false);
    let mut tab = state.tab;
    let mut search = state.search.clone();
    let mut toggled: Option<String> = None;

    egui::Window::new("Database instances")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([860.0, 540.0])
        .frame(
            Frame::NONE
                .fill(pal.bg)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(egui::CornerRadius::same(12))
                .shadow(ctx.global_style().visuals.popup_shadow)
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.label(RichText::new("Database instances").size(20.0).strong());
            ui.label(
                RichText::new("Connect once. Choose exactly what appears in your explorer.")
                    .color(pal.text_muted),
            );
            ui.add_space(10.0);
            ui.separator();
            ui.horizontal_top(|ui| {
                // Left: the instances.
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    ui.set_min_height(360.0);
                    theme::section_label(ui, &format!("{} instances", p.connections.len()));
                    for c in &p.connections {
                        let selected = state.selected == Some(c.id);
                        let connected = p.open.iter().any(|o| o.connection == c.id);
                        let fill = if selected {
                            pal.accent_soft
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        let r = Frame::NONE
                            .fill(fill)
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let name = RichText::new(&c.name).color(if selected {
                                    pal.accent
                                } else {
                                    pal.text
                                });
                                ui.label(name);
                                let status = if connected {
                                    "Connected"
                                } else {
                                    "Disconnected"
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "{} \u{00B7} {status}",
                                        c.driver_kind.as_str()
                                    ))
                                    .size(11.0)
                                    .color(pal.text_muted),
                                );
                            })
                            .response
                            .interact(egui::Sense::click());
                        if r.clicked() {
                            select = Some(c.id);
                        }
                    }
                    ui.add_space(8.0);
                    if ui.button("+ Add instance").clicked() {
                        add = true;
                    }
                });
                ui.separator();
                // Right: the selected instance.
                ui.vertical(|ui| {
                    let Some(profile) = state
                        .selected
                        .and_then(|id| p.connections.iter().find(|c| c.id == id))
                    else {
                        ui.label(
                            RichText::new("Add an instance to get started").color(pal.text_muted),
                        );
                        return;
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&profile.name).size(16.0).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.link("Edit connection...").clicked() {
                                edit = Some(profile.id);
                            }
                        });
                    });
                    let port = profile
                        .port
                        .map_or_else(|| "default".to_owned(), |p| p.to_string());
                    ui.label(
                        RichText::new(format!("{}:{port}", profile.host)).color(pal.text_muted),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut tab, InstanceTab::Databases, "Databases");
                        ui.selectable_value(&mut tab, InstanceTab::Connection, "Connection");
                    });
                    ui.separator();
                    match tab {
                        InstanceTab::Databases => {
                            databases_tab(
                                ui,
                                p,
                                &state,
                                &pal,
                                &mut search,
                                &mut toggled,
                                &mut refresh,
                                &mut select_all,
                            );
                        }
                        InstanceTab::Connection => {
                            for (label, value) in [
                                ("Engine", profile.driver_kind.as_str().to_owned()),
                                ("Host", profile.host.clone()),
                                ("Port", port.clone()),
                                (
                                    "Database",
                                    profile.database.clone().unwrap_or_else(|| "-".into()),
                                ),
                                ("User", profile.username.clone()),
                            ] {
                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        [90.0, 18.0],
                                        egui::Label::new(
                                            RichText::new(label).color(pal.text_muted),
                                        ),
                                    );
                                    ui.label(value);
                                });
                            }
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(
                                    "Passwords are kept in the system keychain, never shown here.",
                                )
                                .size(11.0)
                                .color(pal.text_muted),
                            );
                        }
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Visibility is saved per instance in this workspace.")
                        .size(11.0)
                        .color(pal.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    save = ui
                        .add_enabled(
                            state.selected.is_some(),
                            theme::primary_button("Save visibility"),
                        )
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        });

    if let Some(open) = p.instances.as_mut() {
        open.tab = tab;
        open.search = search;
    }
    if let Some(name) = toggled {
        p.toggle_database(&name);
    }
    if select_all {
        p.select_all_databases();
    }
    if refresh {
        p.refresh_instance_databases();
    }
    if let Some(id) = select {
        p.select_instance(id);
    }
    if let Some(id) = edit {
        p.close_instances();
        p.edit_connection(id);
    } else if add {
        p.close_instances();
        p.new_connection_form();
    } else if save {
        p.save_visibility();
    } else if cancel || escape {
        p.close_instances();
    }
}

#[allow(clippy::too_many_arguments)]
fn databases_tab(
    ui: &mut egui::Ui,
    p: &Presenter,
    state: &crate::presenter::InstancesState,
    pal: &theme::Palette,
    search: &mut String,
    toggled: &mut Option<String>,
    refresh: &mut bool,
    select_all: &mut bool,
) {
    theme::section_label(ui, "Show in explorer");
    ui.add(
        egui::TextEdit::singleline(search)
            .hint_text("Search databases...")
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        if ui.link("Select all").clicked() {
            *select_all = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.link("Refresh list").clicked() {
                *refresh = true;
            }
        });
    });
    let rows = p.visible_database_rows();
    let connected = state
        .selected
        .is_some_and(|id| p.open.iter().any(|o| o.connection == id));
    if rows.is_empty() {
        let message = if connected {
            "No databases match."
        } else {
            "Connect to this instance to list its databases."
        };
        ui.label(RichText::new(message).color(pal.text_muted));
    }
    egui::ScrollArea::vertical()
        .max_height(250.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in rows {
                ui.horizontal(|ui| {
                    let mut on = state.checked.contains(&row.name);
                    if ui.checkbox(&mut on, &row.name).changed() {
                        *toggled = Some(row.name.clone());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} tables", row.tables)).color(pal.text_muted),
                        );
                    });
                });
            }
        });
    let total = p.databases_of_selected().len();
    ui.label(
        RichText::new(format!(
            "{} of {total} databases selected \u{00B7} Hiding does not disconnect a database.",
            state.checked.len()
        ))
        .size(11.0)
        .color(pal.text_muted),
    );
}
