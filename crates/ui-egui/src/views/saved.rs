use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use crate::presenter::Presenter;
use crate::theme;

/// The "Saved queries" list at the bottom of the sidebar.
pub fn sidebar(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);
    ui.add_space(4.0);
    theme::section_label(ui, "Saved queries");
    if p.saved.is_empty() {
        ui.label(
            RichText::new("Press Ctrl+S to keep a query")
                .size(11.0)
                .color(pal.text_muted),
        );
        return;
    }
    let mut open = None;
    let mut ask = None;
    let (mut confirm, mut cancel) = (false, false);
    egui::ScrollArea::vertical()
        .id_salt("saved_queries")
        .max_height(160.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for q in &p.saved {
                let open_here = p.tabs.iter().any(|t| t.saved == Some(q.id));
                ui.horizontal(|ui| {
                    if p.confirm_delete == Some(q.id) {
                        ui.label(RichText::new(format!("Delete `{}`?", q.name)).color(pal.danger));
                        confirm = ui.small_button("Delete").clicked();
                        cancel = ui.small_button("Keep").clicked();
                        return;
                    }
                    let label =
                        RichText::new(&q.name).color(if open_here { pal.accent } else { pal.text });
                    let response = ui
                        .add(egui::Button::new(label).frame(false).truncate())
                        .on_hover_text(q.text.lines().next().unwrap_or_default());
                    if response.clicked() {
                        open = Some(q.id);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("x").color(pal.text_faint))
                                    .frame(false),
                            )
                            .on_hover_text("Delete this saved query")
                            .clicked()
                        {
                            ask = Some(q.id);
                        }
                    });
                });
            }
        });
    if let Some(id) = open {
        p.open_saved(id);
    }
    if let Some(id) = ask {
        p.request_delete_saved(id);
    }
    if confirm {
        p.confirm_delete_saved();
    }
    if cancel {
        p.cancel_delete_saved();
    }
}

/// The "Save query" dialog. Draws nothing unless it was opened.
pub fn save_dialog(ctx: &egui::Context, p: &mut Presenter) {
    let Some(dialog) = p.save_dialog.clone() else {
        return;
    };
    let pal = *theme::palette_for(ctx.theme());
    let escape = p.palette.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut name = dialog.name.clone();
    let (mut save, mut cancel) = (false, false);

    egui::Window::new("save_query")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([440.0, 0.0])
        .frame(
            Frame::NONE
                .fill(pal.bg)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(egui::CornerRadius::same(12))
                .shadow(ctx.global_style().visuals.popup_shadow)
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            let title = if dialog.as_new {
                "Save query as"
            } else {
                "Save query"
            };
            ui.label(RichText::new(title).size(20.0).strong());
            ui.label(
                RichText::new("Keep this SQL under a name, to open it again from the sidebar.")
                    .color(pal.text_muted),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("Name").size(12.0).color(pal.text_muted));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .hint_text("customers.sql")
                    .desired_width(f32::INFINITY),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                save = true;
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    save |= ui.add(theme::primary_button("Save")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        });

    p.set_save_name(name);
    if save {
        p.confirm_save();
    } else if cancel || escape {
        p.cancel_save_dialog();
    }
}
