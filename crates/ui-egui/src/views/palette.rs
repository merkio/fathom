use eframe::egui::{self, Color32, CornerRadius, Frame, Key, Margin, RichText, Sense, Stroke};

use crate::presenter::{PaletteAction, PaletteItem, Presenter};
use crate::theme;

const WIDTH: f32 = 640.0;

/// The command palette overlay. Draws nothing while closed.
pub fn show(ctx: &egui::Context, p: &mut Presenter) {
    let Some(state) = p.palette.clone() else {
        return;
    };
    let pal = *theme::palette_for(ctx.theme());
    let screen = ctx.content_rect();

    // A click outside the box closes it.
    let mut close = false;
    egui::Area::new(egui::Id::unique("palette_backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let response = ui.allocate_rect(screen, Sense::click());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(90));
            close = response.clicked();
        });

    let items = p.palette_items();
    let (mut accept, mut chosen): (bool, Option<PaletteAction>) = (false, None);
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
            p.palette_move(1);
        }
        if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
            p.palette_move(-1);
        }
        if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
            accept = true;
        }
        if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
            close = true;
        }
    });

    let mut query = state.query.clone();
    egui::Area::new(egui::Id::unique("palette"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 110.0])
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.bg)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(CornerRadius::same(12))
                .shadow(ui.style().visuals.popup_shadow)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(WIDTH);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Search commands, tables, or settings...")
                            .desired_width(f32::INFINITY),
                    );
                    edit.request_focus();
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            if items.is_empty() {
                                ui.label(
                                    RichText::new("No matching commands").color(pal.text_muted),
                                );
                            }
                            let mut section = "";
                            for (index, item) in items.iter().enumerate() {
                                if item.section != section {
                                    section = item.section;
                                    ui.add_space(6.0);
                                    theme::section_label(ui, section);
                                }
                                if row(ui, item, index == state.selected, &pal).clicked() {
                                    chosen = Some(item.action.clone());
                                }
                            }
                        });
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Up/Down navigate \u{00B7} Enter select \u{00B7} Esc close")
                            .size(11.0)
                            .color(pal.text_faint),
                    );
                });
        });

    p.set_palette_query(query);
    if let Some(action) = chosen {
        p.run_palette(action);
    } else if accept {
        p.palette_accept();
    } else if close {
        p.close_palette();
    }
}

fn row(
    ui: &mut egui::Ui,
    item: &PaletteItem,
    selected: bool,
    pal: &theme::Palette,
) -> egui::Response {
    let fill = if selected {
        pal.accent_soft
    } else {
        Color32::TRANSPARENT
    };
    let inner = Frame::NONE
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    let title = RichText::new(&item.title).color(if selected {
                        pal.accent
                    } else {
                        pal.text
                    });
                    ui.label(title);
                    if !item.subtitle.is_empty() {
                        ui.label(
                            RichText::new(&item.subtitle)
                                .size(11.0)
                                .color(pal.text_muted),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(on) = item.toggle {
                        let (text, color) = if on {
                            ("On", pal.success)
                        } else {
                            ("Off", pal.text_faint)
                        };
                        ui.label(RichText::new(text).color(color).strong());
                    }
                    if let Some(shortcut) = &item.shortcut {
                        ui.label(RichText::new(shortcut).size(11.0).color(pal.text_muted));
                    }
                });
            });
        });
    inner.response.interact(Sense::click())
}
