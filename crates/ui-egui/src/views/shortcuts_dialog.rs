use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use crate::keymap::{Action, Keymap, format_shortcut};
use crate::presenter::Presenter;
use crate::theme;

/// The "Keyboard shortcuts" dialog: every action, its current binding and the id to use in the
/// `[keys]` table of `fathom.toml`. Read-only; the bindings are changed in the file.
pub fn show(ctx: &egui::Context, p: &mut Presenter) {
    if !p.shortcuts_dialog {
        return;
    }
    let pal = *theme::palette_for(ctx.theme());
    let escape = p.palette.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut close = false;
    egui::Window::new("keyboard_shortcuts")
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
            ui.label(RichText::new("Keyboard shortcuts").size(20.0).strong());
            ui.label(
                RichText::new(
                    "Change them in the [keys] table of fathom.toml (see fathom.example.toml). Restart Fathom to apply.",
                )
                .color(pal.text_muted),
            );
            ui.add_space(8.0);
            egui::Grid::new("shortcut_rows")
                .num_columns(3)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for h in ["Action", "Shortcut", "Name in fathom.toml"] {
                        theme::section_label(ui, h);
                    }
                    ui.end_row();
                    for action in Action::ALL {
                        ui.label(action.label());
                        let bound: Vec<String> =
                            p.keymap.bindings(action).iter().map(format_shortcut).collect();
                        let changed = p.keymap.bindings(action) != Keymap::defaults(action);
                        let text = if bound.is_empty() {
                            "not assigned".to_owned()
                        } else {
                            bound.join("  or  ")
                        };
                        let color = if changed { pal.accent } else { pal.text };
                        ui.label(RichText::new(text).color(color));
                        ui.label(RichText::new(action.id()).monospace().color(pal.text_muted));
                        ui.end_row();
                    }
                });
            if !p.keymap.problems().is_empty() {
                ui.add_space(8.0);
                Frame::NONE
                    .fill(pal.warning_soft)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new("Problems in [keys]").color(pal.warning).strong());
                        for problem in p.keymap.problems() {
                            ui.label(RichText::new(problem).size(11.0).color(pal.warning));
                        }
                    });
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("Highlighted shortcuts differ from the defaults. Copy, paste and text editing keys belong to your system.")
                    .size(11.0)
                    .color(pal.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui.button("Close").clicked();
            });
        });
    if close || escape {
        p.shortcuts_dialog = false;
    }
}
