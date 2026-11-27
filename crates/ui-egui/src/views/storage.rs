use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use crate::presenter::{Presenter, TestState};
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
                    theme::section_label(ui, &format!("{} destinations", p.destinations.len()));
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
                    let Some(d) = selected
                        .as_ref()
                        .and_then(|n| p.destinations.iter().find(|d| &d.name == n))
                    else {
                        ui.label(RichText::new("No destinations are configured.").color(pal.text_muted));
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
                    for (label, value) in [("Type", kind_label(&d.kind)), ("Location", d.location.clone())] {
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
                    if ui
                        .add_enabled(!testing, egui::Button::new("Test connection"))
                        .on_hover_text("Writes and removes a tiny probe file")
                        .clicked()
                    {
                        test = Some(d.name.clone());
                    }
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
                    RichText::new("Destinations and credentials are set in fathom.toml and environment variables (see the README). Secrets are never shown here.")
                        .size(11.0)
                        .color(pal.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button("Close").clicked();
                });
            });
        });

    if let Some(name) = pick {
        p.select_storage(&name);
    }
    if let Some(name) = test {
        p.test_destination(&name);
    }
    if close || escape {
        p.close_storage_dialog();
    }
}
