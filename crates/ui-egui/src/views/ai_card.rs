//! The inline AI bar under the editor ("Ask in plain language, stay in your query").
//!
//! It drives the same assistant as the side panel: only schema metadata is sent (row data only
//! when asked for in the panel), the reply is a draft that lands in the editor when the user
//! presses "Insert draft", and nothing is ever run from here.

use eframe::egui::{self, Frame, Key, Margin, RichText, Stroke};

use crate::ai::{AiMode, AiRunState};
use crate::presenter::Presenter;
use crate::theme;

const INPUT_ID: &str = "ai_card_input";

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);
    let input_id = ui.make_persistent_id(INPUT_ID);
    // Ctrl+I jumps here.
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::I)) {
        ui.memory_mut(|m| m.request_focus(input_id));
    }

    let ready = p.ai.ready().is_ok();
    let streaming = p.ai.is_streaming();
    let mut ask = false;
    let (mut stop, mut insert, mut dismiss, mut setup) = (false, false, false, false);

    Frame::NONE
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{2726}").color(pal.accent).size(16.0));
                ui.vertical(|ui| {
                    match p.ai.run.as_ref().map(|r| (&r.prompt, &r.state)) {
                        None => {
                            let hint = if ready {
                                "Ask in plain language, stay in your query."
                            } else {
                                "Choose an AI provider and model to ask questions."
                            };
                            let edit = ui.add_enabled(
                                ready,
                                egui::TextEdit::singleline(&mut p.ai.input)
                                    .id(input_id)
                                    .hint_text(hint)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY),
                            );
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                ask = true;
                            }
                            ui.label(
                                RichText::new(
                                    "e.g. \u{201C}Show customers who haven\u{2019}t ordered in 30 days\u{201D}",
                                )
                                .color(pal.text_muted),
                            );
                        }
                        Some((prompt, AiRunState::Streaming)) => {
                            ui.label(RichText::new(prompt).strong());
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new("Drafting...").color(pal.text_muted));
                            });
                        }
                        Some((prompt, AiRunState::Done(reply))) => {
                            ui.label(RichText::new(prompt).strong());
                            let first = reply
                                .explanation
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or("");
                            let summary = match (&reply.sql, &reply.warning) {
                                (_, Some(w)) => format!("Draft ready \u{00B7} {w}"),
                                (Some(_), None) if first.is_empty() => "Draft ready.".to_owned(),
                                (Some(_), None) => format!("Draft ready \u{00B7} {first}"),
                                (None, None) => first.to_owned(),
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{summary}  Your query stays unchanged until you insert."
                                ))
                                .color(pal.text_muted),
                            );
                        }
                        Some((prompt, AiRunState::Failed(message))) => {
                            ui.label(RichText::new(prompt).strong());
                            ui.label(RichText::new(message).color(pal.danger));
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    ui.vertical(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            match p.ai.run.as_ref().map(|r| &r.state) {
                                None if ready => {
                                    ui.label(
                                        RichText::new("Ctrl+I")
                                            .size(11.0)
                                            .background_color(pal.bg)
                                            .color(pal.text_muted),
                                    );
                                    ui.label(
                                        RichText::new("Schema only \u{00B7} no row data")
                                            .size(11.0)
                                            .color(pal.text_muted),
                                    );
                                }
                                None => setup = ui.button("Set up AI").clicked(),
                                Some(AiRunState::Streaming) => stop = ui.button("Stop").clicked(),
                                Some(AiRunState::Done(reply)) => {
                                    if reply.sql.is_some() {
                                        insert = ui
                                            .add(theme::primary_button("Insert draft  \u{21E5}"))
                                            .clicked();
                                    }
                                }
                                Some(AiRunState::Failed(_)) => {}
                            }
                        });
                        if p.ai.run.is_some() && !streaming {
                            dismiss = ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Dismiss  Esc").size(11.0).color(pal.text_muted),
                                    )
                                    .frame(false),
                                )
                                .clicked();
                        }
                    });
                });
            });
        });

    if ask && !p.ai.input.trim().is_empty() {
        p.ai_ask(AiMode::Generate);
    }
    if stop {
        p.ai_stop();
    }
    if insert {
        p.ai_insert_sql(false);
        p.ai_dismiss();
    }
    if setup {
        p.open_ai_panel();
    }
    // Esc dismisses a finished draft (never while the editor's popup owns Esc: it consumes first).
    if (dismiss || (p.ai.run.is_some() && !streaming && ui.input(|i| i.key_pressed(Key::Escape))))
        && !streaming
    {
        p.ai_dismiss();
    }
}
