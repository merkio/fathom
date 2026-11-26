//! The AI side panel: ask, preview what is sent, read the streamed reply, insert SQL into the
//! editor. SQL is never run from here; the user reviews it in the editor and runs it.

use application::llm::StatementKind;
use application::usecases::AssistReply;
use eframe::egui::{self, Color32, RichText};

use crate::ai::{AiMode, AiRunState, ModelsState};
use crate::presenter::Presenter;

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    egui::ScrollArea::vertical()
        .id_salt("ai_panel")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            selection(ui, p);
            ui.separator();
            modes(ui, p);
            ask_form(ui, p);
            ui.separator();
            reply(ui, p);
            ui.separator();
            inline_settings(ui, p);
        });
    preview_window(ui.ctx(), p);
}

fn selection(ui: &mut egui::Ui, p: &mut Presenter) {
    let providers: Vec<_> = p.ai.providers.iter().map(|s| s.id.clone()).collect();
    if providers.is_empty() {
        ui.weak("No AI providers are configured.");
        return;
    }
    let mut chosen_provider = None;
    let mut chosen_model = None;
    let mut refresh = false;

    ui.horizontal_wrapped(|ui| {
        let current =
            p.ai.settings
                .provider
                .as_ref()
                .map_or("provider", |id| id.as_str());
        egui::ComboBox::from_id_salt("ai_provider")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for id in &providers {
                    let selected = p.ai.settings.provider.as_ref() == Some(id);
                    if ui.selectable_label(selected, id.as_str()).clicked() {
                        chosen_provider = Some(id.clone());
                    }
                }
            });

        let model = p.ai.settings.model.as_ref().map_or("model", |m| m.as_str());
        egui::ComboBox::from_id_salt("ai_model")
            .selected_text(model)
            .show_ui(ui, |ui| match p.ai.selected_provider().map(|s| &s.models) {
                Some(ModelsState::Ready(models)) => {
                    for m in models {
                        let selected = p.ai.settings.model.as_ref() == Some(&m.id);
                        if ui.selectable_label(selected, &m.display_name).clicked() {
                            chosen_model = Some(m.id.clone());
                        }
                    }
                }
                Some(ModelsState::Loading) => {
                    ui.spinner();
                }
                Some(ModelsState::Failed(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                _ => {
                    ui.weak("Pick a provider first");
                }
            });
        if ui
            .small_button("Refresh")
            .on_hover_text("Reload the model list")
            .clicked()
        {
            refresh = true;
        }
    });
    if let Some(id) = chosen_provider {
        p.ai_select_provider(id);
    }
    if let Some(id) = chosen_model {
        p.ai_select_model(id);
    }
    if refresh {
        p.ai_refresh_models();
    }

    // API key, for providers that need one. The key is never displayed again once saved.
    let Some(state) = p.ai.selected_provider() else {
        return;
    };
    if let ModelsState::Failed(e) = &state.models {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    if state.needs_key {
        let set = state.key_set == Some(true);
        let (mut save, mut clear) = (false, false);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut p.ai.api_key_input)
                    .password(true)
                    .hint_text(if set {
                        "API key saved (enter a new one to replace)"
                    } else {
                        "API key"
                    })
                    .desired_width(180.0),
            );
            save = ui.button("Save key").clicked();
            if set {
                clear = ui.button("Remove").clicked();
            }
        });
        if save {
            p.ai_save_key();
        }
        if clear {
            p.ai_clear_key();
        }
    }
}

fn modes(ui: &mut egui::Ui, p: &mut Presenter) {
    ui.horizontal_wrapped(|ui| {
        for mode in AiMode::ALL {
            if ui
                .selectable_label(p.ai.mode == mode, mode.label())
                .clicked()
            {
                p.ai.mode = mode;
            }
        }
    });
}

fn ask_form(ui: &mut egui::Ui, p: &mut Presenter) {
    let mode = p.ai.mode;
    match mode {
        AiMode::Generate => {
            ui.weak("Describe the query you want. SQL already in the editor is sent as context.");
        }
        AiMode::Explain => {
            ui.weak("Explains the SQL in the editor.");
        }
        AiMode::Optimize => {
            ui.weak("Suggests a faster or clearer version of the SQL in the editor.");
        }
        AiMode::Fix => {
            ui.weak("Fixes the last query of this tab if it failed.");
        }
        AiMode::Chat => {
            ui.weak("Ask anything about this database.");
        }
    }
    if mode.takes_input() {
        ui.add(
            egui::TextEdit::multiline(&mut p.ai.input)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text(if mode == AiMode::Chat {
                    "Message"
                } else {
                    "e.g. revenue per customer in 2024"
                }),
        );
    }

    ui.checkbox(&mut p.ai.include_rows, "Also share up to 5 result rows with this request")
        .on_hover_text("Off by default. Rows are real data and are sent to the provider. Applies to this request only.");
    if p.ai.include_rows {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Rows from the current result will be sent to the provider.",
        );
    }

    let streaming = p.ai.is_streaming();
    let (mut ask, mut preview, mut stop) = (false, false, false);
    ui.horizontal(|ui| {
        ask = ui
            .add_enabled(!streaming, egui::Button::new("Ask"))
            .clicked();
        preview = ui.button("Show what is sent").clicked();
        if streaming {
            stop = ui.button("Stop").clicked();
            ui.spinner();
        }
    });
    ui.weak("Sent: the dialect, your schema (names and types only) and your text.");
    if ask {
        p.ai_ask(mode);
    }
    if preview {
        p.ai_preview(mode);
    }
    if stop {
        p.ai_stop();
    }
}

fn badge(ui: &mut egui::Ui, kind: StatementKind) {
    let (text, color) = match kind {
        StatementKind::Read => ("READ", Color32::from_rgb(46, 160, 67)),
        StatementKind::Write => ("WRITE", Color32::from_rgb(210, 140, 30)),
        StatementKind::Ddl => ("DDL", Color32::from_rgb(210, 60, 60)),
        StatementKind::Admin => ("ADMIN", Color32::from_rgb(210, 60, 60)),
        StatementKind::Unknown => ("UNPARSEABLE", Color32::from_rgb(140, 140, 140)),
    };
    ui.label(
        RichText::new(text)
            .strong()
            .color(Color32::WHITE)
            .background_color(color),
    );
}

fn reply(ui: &mut egui::Ui, p: &mut Presenter) {
    if p.ai.mode == AiMode::Chat && !p.ai.chat.is_empty() {
        ui.horizontal(|ui| {
            ui.strong("Conversation");
            if ui.small_button("Clear").clicked() {
                p.ai.clear_chat();
            }
        });
        for m in &p.ai.chat {
            let who = if m.role == domain::Role::User {
                "You"
            } else {
                "Assistant"
            };
            ui.label(RichText::new(who).weak());
            ui.label(&m.content);
            ui.add_space(4.0);
        }
        ui.separator();
    }

    let (mut insert, mut replace, mut copy) = (false, false, None::<String>);
    let Some(run) = &p.ai.run else {
        ui.weak("Replies appear here.");
        return;
    };
    ui.label(RichText::new(&run.prompt).weak());
    match &run.state {
        AiRunState::Streaming => {
            if run.text.is_empty() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Waiting for the model...");
                });
            } else {
                ui.label(&run.text);
            }
        }
        AiRunState::Failed(message) => {
            if !run.text.is_empty() {
                ui.label(&run.text);
            }
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        AiRunState::Done(reply) => {
            sql_block(ui, reply, &mut insert, &mut replace, &mut copy);
        }
    }
    if insert {
        p.ai_insert_sql(false);
    }
    if replace {
        p.ai_insert_sql(true);
    }
    if let Some(text) = copy {
        ui.ctx().copy_text(text);
    }
}

fn sql_block(
    ui: &mut egui::Ui,
    reply: &AssistReply,
    insert: &mut bool,
    replace: &mut bool,
    copy: &mut Option<String>,
) {
    if let Some(warning) = &reply.warning {
        ui.colored_label(ui.visuals().warn_fg_color, warning);
    }
    if let Some(sql) = &reply.sql {
        ui.horizontal(|ui| {
            if let Some(kind) = reply.safety.as_ref().and_then(|s| s.overall()) {
                badge(ui, kind);
            }
            ui.weak("Review it before running");
        });
        if let Some(error) = reply.safety.as_ref().and_then(|s| s.parse_error.as_ref()) {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Could not parse this SQL: {error}"),
            );
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(sql).monospace()).wrap());
        });
        ui.horizontal(|ui| {
            *insert = ui.button("Insert into editor").clicked();
            *replace = ui.button("Replace editor").clicked();
            if ui.button("Copy").clicked() {
                *copy = Some(sql.clone());
            }
        });
        if !reply.explanation.is_empty() {
            ui.label(&reply.explanation);
        }
    } else {
        ui.label(&reply.text);
    }
    if let Some(usage) = reply.usage {
        ui.weak(format!(
            "{} input / {} output tokens",
            usage.input_tokens, usage.output_tokens
        ));
    }
}

/// Shows exactly what a request would send.
fn preview_window(ctx: &egui::Context, p: &mut Presenter) {
    let Some(preview) = p.ai.preview.clone() else {
        return;
    };
    let mut open = true;
    egui::Window::new("What will be sent")
        .open(&mut open)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.label(format!(
                "To {} ({}), template {}",
                preview.provider.as_str(),
                preview.model.as_str(),
                preview.template
            ));
            ui.label(format!(
                "Schema: {} of {} tables. Row data: {}.",
                preview.schema_tables_sent,
                preview.schema_tables_total,
                if preview.includes_sample_rows {
                    "INCLUDED (you ticked the box)"
                } else {
                    "not included"
                }
            ));
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.strong("System");
                    ui.add(egui::Label::new(RichText::new(&preview.system).monospace()).wrap());
                    for m in &preview.messages {
                        ui.add_space(6.0);
                        ui.strong(format!("{:?}", m.role));
                        ui.add(egui::Label::new(RichText::new(&m.content).monospace()).wrap());
                    }
                });
        });
    if !open {
        p.ai.preview = None;
    }
}

/// Inline (ghost text) suggestions. Off by default; turning them on sends typing context to the
/// chosen provider, so the warning is shown right where the switch is.
fn inline_settings(ui: &mut egui::Ui, p: &mut Presenter) {
    egui::CollapsingHeader::new("Inline suggestions").default_open(false).show(ui, |ui| {
        let mut enabled = p.inline.settings.enabled;
        if ui.checkbox(&mut enabled, "Suggest completions as I type (off by default)").changed() {
            p.inline_set_enabled(enabled);
        }
        let provider_name = p
            .inline
            .settings
            .provider
            .as_ref()
            .map_or("the selected provider".to_owned(), |id| id.as_str().to_owned());
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "While this is on, after each pause in typing the text around your cursor and \
                 your schema (names and types, never row data) are sent to {provider_name}. \
                 With a local Ollama model they stay on this machine. A small, fast model works best."
            ),
        );
        ui.weak("Tab accepts a suggestion. Each connection can override this in its settings.");

        let providers: Vec<_> = p.ai.providers.iter().map(|s| s.id.clone()).collect();
        let mut chosen_provider = None;
        let mut chosen_model = None;
        ui.horizontal_wrapped(|ui| {
            let current = p.inline.settings.provider.as_ref().map_or("provider", |id| id.as_str());
            egui::ComboBox::from_id_salt("inline_provider").selected_text(current).show_ui(ui, |ui| {
                for id in &providers {
                    let selected = p.inline.settings.provider.as_ref() == Some(id);
                    if ui.selectable_label(selected, id.as_str()).clicked() {
                        chosen_provider = Some(id.clone());
                    }
                }
            });
            let model = p.inline.settings.model.as_ref().map_or("model", |m| m.as_str());
            egui::ComboBox::from_id_salt("inline_model").selected_text(model).show_ui(ui, |ui| {
                let models = p
                    .inline
                    .settings
                    .provider
                    .as_ref()
                    .and_then(|id| p.ai.provider(id))
                    .map(|s| &s.models);
                match models {
                    Some(ModelsState::Ready(models)) => {
                        for m in models {
                            let selected = p.inline.settings.model.as_ref() == Some(&m.id);
                            if ui.selectable_label(selected, &m.display_name).clicked() {
                                chosen_model = Some(m.id.clone());
                            }
                        }
                    }
                    Some(ModelsState::Failed(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Some(ModelsState::Loading) => {
                        ui.spinner();
                    }
                    _ => {
                        ui.weak("Pick a provider first");
                    }
                }
            });
        });
        if let Some(id) = chosen_provider {
            p.inline_select_provider(id);
        }
        if let Some(id) = chosen_model {
            p.inline_select_model(id);
        }
        if p.inline.settings.enabled && (p.inline.settings.provider.is_none() || p.inline.settings.model.is_none()) {
            ui.colored_label(ui.visuals().warn_fg_color, "Choose a provider and model for it to work.");
        }
        if let Some(error) = &p.inline.error {
            ui.colored_label(ui.visuals().error_fg_color, format!("Last inline request failed: {error}"));
        }
    });
}
