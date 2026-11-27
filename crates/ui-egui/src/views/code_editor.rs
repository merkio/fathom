//! SQL editor widget: `egui::TextEdit` with syntax highlighting and a completion popup.
//!
//! All decisions (what to suggest, when the popup shows, what accepting does) live in the
//! presenter and `application`; this file only draws and translates keys.

use application::sql::{Suggestion, SuggestionKind, SyntaxClass, highlight};
use domain::Dialect;
use eframe::egui::{
    self, Color32, FontId, Id, Key, Modifiers, Order, TextFormat,
    text::{CCursor, CCursorRange, LayoutJob},
    text_edit::TextEditState,
};
use std::time::Instant;

use crate::bridge::TabId;
use crate::keymap::Action;
use crate::presenter::Presenter;

const POPUP_MAX_HEIGHT: f32 = 220.0;

fn color(class: SyntaxClass, dark: bool) -> Option<Color32> {
    // `None` keeps the default text colour.
    let (d, l) = match class {
        SyntaxClass::Keyword => (
            Color32::from_rgb(86, 156, 214),
            Color32::from_rgb(0, 0, 200),
        ),
        SyntaxClass::Function => (
            Color32::from_rgb(220, 220, 170),
            Color32::from_rgb(120, 80, 0),
        ),
        SyntaxClass::String => (
            Color32::from_rgb(206, 145, 120),
            Color32::from_rgb(163, 21, 21),
        ),
        SyntaxClass::Number => (
            Color32::from_rgb(181, 206, 168),
            Color32::from_rgb(9, 134, 88),
        ),
        SyntaxClass::Comment => (
            Color32::from_rgb(106, 153, 85),
            Color32::from_rgb(0, 128, 0),
        ),
        SyntaxClass::Identifier | SyntaxClass::Punctuation | SyntaxClass::Whitespace => {
            return None;
        }
    };
    Some(if dark { d } else { l })
}

fn highlight_job(ui: &egui::Ui, text: &str, dialect: &Dialect, wrap_width: f32) -> LayoutJob {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let default = ui.visuals().text_color();
    let dark = ui.visuals().dark_mode;
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    for span in highlight(text, dialect) {
        let color = color(span.class, dark).unwrap_or(default);
        job.append(
            &text[span.range],
            0.0,
            TextFormat::simple(FontId { ..font.clone() }, color),
        );
    }
    job
}

fn byte_to_char(text: &str, byte: usize) -> usize {
    text.get(..byte)
        .map_or_else(|| text.chars().count(), |s| s.chars().count())
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(b, _)| b)
}

fn set_cursor(ctx: &egui::Context, id: Id, text: &str, byte: usize) {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    let at = CCursor::new(byte_to_char(text, byte));
    state.cursor.set_char_range(Some(CCursorRange::one(at)));
    state.store(ctx, id);
}

fn kind_tag(kind: SuggestionKind) -> &'static str {
    match kind {
        SuggestionKind::Keyword => "kw",
        SuggestionKind::Function => "fn",
        SuggestionKind::Schema => "sch",
        SuggestionKind::Table => "tbl",
        SuggestionKind::View => "view",
        SuggestionKind::Column => "col",
        SuggestionKind::Join => "join",
        SuggestionKind::Inline => "ai",
    }
}

/// Width reserved left of the text for line numbers.
const GUTTER_WIDTH: f32 = 48.0;

/// Line numbers in the left margin, one per logical line (a wrapped line is numbered once).
fn draw_line_numbers(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput) {
    let color = ui.visuals().weak_text_color();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let x = output.galley_pos.x - 14.0;
    let mut number = 1;
    let mut starts_line = true;
    for placed in &output.galley.rows {
        if starts_line {
            ui.painter().text(
                egui::pos2(x, output.galley_pos.y + placed.pos.y),
                egui::Align2::RIGHT_TOP,
                number.to_string(),
                font.clone(),
                color,
            );
            number += 1;
        }
        starts_line = placed.ends_with_newline;
    }
}

/// Cursor of the editor as of the last frame, as a byte offset.
fn cursor_byte(ctx: &egui::Context, id: Id, text: &str) -> Option<usize> {
    let state = TextEditState::load(ctx, id)?;
    let range = state.cursor.char_range()?;
    Some(char_to_byte(text, range.primary.index.0))
}

/// `height` is the space the editor should fill, so the whole area is clickable.
pub fn show(ui: &mut egui::Ui, p: &mut Presenter, tab: TabId, height: f32) {
    let ctx = ui.ctx().clone();
    let edit_id = ui.make_persistent_id(("editor_text", tab.0));
    let dialect = p.tab_dialect(tab);
    let focused = ctx.memory(|m| m.has_focus(edit_id));

    // 1. Keys that drive the popup are consumed before the text edit sees them.
    if focused {
        let text = p
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .map(|t| t.text.clone())
            .unwrap_or_default();
        if let Some(cursor) = cursor_byte(&ctx, edit_id, &text) {
            if p.fired(Action::Autocomplete) {
                p.request_popup(tab, cursor);
            }
            let shown = p.popup(tab, cursor);
            if !shown.is_empty() {
                let n = shown.len();
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                    p.move_selection(tab, 1, n);
                }
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                    p.move_selection(tab, -1, n);
                }
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    p.dismiss_popup(tab, cursor);
                }
                let accept = ctx.input_mut(|i| {
                    i.consume_key(Modifiers::NONE, Key::Enter)
                        || i.consume_key(Modifiers::NONE, Key::Tab)
                });
                if accept {
                    let chosen = p
                        .selected_tab()
                        .map_or(0, |t| t.completion.selected)
                        .min(n - 1);
                    if let Some(new_cursor) = p.accept_suggestion(tab, &shown[chosen]) {
                        let text = p.tabs.iter().find(|t| t.id == tab).map(|t| t.text.clone());
                        set_cursor(&ctx, edit_id, &text.unwrap_or_default(), new_cursor);
                    }
                }
            } else if p.inline_ghost(tab, cursor).is_some()
                && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab))
                && let Some(new_cursor) = p.inline_accept(tab, cursor)
            {
                // LLM ghost text: Tab inserts it. Local completions take priority (above).
                let text = p.tabs.iter().find(|t| t.id == tab).map(|t| t.text.clone());
                set_cursor(&ctx, edit_id, &text.unwrap_or_default(), new_cursor);
            }
        }
    }

    // 2. The editor itself.
    let line = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let rows = ((height / line).floor() as usize).max(8);
    let Some(tab_state) = p.tab_mut(tab) else {
        return;
    };
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let job = highlight_job(ui, text.as_str(), &dialect, wrap_width);
        ui.painter().layout_job(job)
    };
    let output = egui::TextEdit::multiline(&mut tab_state.text)
        .id(edit_id)
        .code_editor()
        .hint_text("SELECT ...")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: GUTTER_WIDTH as i8,
            right: 12,
            top: 8,
            bottom: 8,
        }))
        .desired_width(f32::INFINITY)
        .desired_rows(rows)
        .layouter(&mut layouter)
        .show(ui);
    draw_line_numbers(ui, &output);

    // 3. Inline ghost text and the popup, from the state after this frame's edits.
    let now = Instant::now();
    let range = match output.cursor_range {
        Some(range) if output.response.has_focus() => range,
        _ => {
            // Not editing: stop any inline request and hide ghost text.
            p.inline_tick(now, tab, 0, false, false);
            return;
        }
    };
    let text = p
        .tabs
        .iter()
        .find(|t| t.id == tab)
        .map(|t| t.text.clone())
        .unwrap_or_default();
    let cursor = char_to_byte(&text, range.primary.index.0);
    p.note_cursor(tab, cursor);
    let shown = p.popup(tab, cursor);
    if let Some(wake) = p.inline_tick(now, tab, cursor, true, !shown.is_empty()) {
        // egui only repaints on events; ask for a frame when the debounce timer is due.
        ctx.request_repaint_after(wake);
    }
    if shown.is_empty() {
        draw_ghost(ui, p, tab, cursor, &output, range);
        return;
    }
    let selected = p.selected_tab().map_or(0, |t| t.completion.selected);
    let anchor =
        output.galley.pos_from_cursor(range.primary).left_bottom() + output.galley_pos.to_vec2();

    let mut clicked: Option<usize> = None;
    egui::Area::new(ui.make_persistent_id(("completion_popup", tab.0)))
        .order(Order::Foreground)
        .fixed_pos(anchor + egui::vec2(0.0, 2.0))
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(460.0);
                egui::ScrollArea::vertical()
                    .max_height(POPUP_MAX_HEIGHT)
                    .min_scrolled_height(POPUP_MAX_HEIGHT)
                    .show(ui, |ui| {
                        for (i, s) in shown.iter().enumerate() {
                            let row = row(ui, s, i == selected);
                            if i == selected
                                && ctx.input(|i| {
                                    i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowUp)
                                })
                            {
                                row.scroll_to_me(None);
                            }
                            if row.clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
            });
        });
    if let Some(i) = clicked
        && let Some(new_cursor) = p.accept_suggestion(tab, &shown[i])
    {
        let text = p
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .map(|t| t.text.clone())
            .unwrap_or_default();
        set_cursor(&ctx, edit_id, &text, new_cursor);
        ctx.memory_mut(|m| m.request_focus(edit_id));
    }
}

fn row(ui: &mut egui::Ui, s: &Suggestion, selected: bool) -> egui::Response {
    let text = egui::RichText::new(format!("{:<5}{}", kind_tag(s.kind), s.text)).monospace();
    let mut response = ui.selectable_label(selected, text);
    if let Some(detail) = &s.detail {
        response = response.on_hover_text(detail);
    }
    response
}

/// LLM ghost text, drawn in a muted colour right after the cursor (a warning colour when the
/// statement it would complete is not a plain read). It is not part of the text until accepted.
fn draw_ghost(
    ui: &egui::Ui,
    p: &Presenter,
    tab: TabId,
    cursor: usize,
    output: &egui::text_edit::TextEditOutput,
    range: CCursorRange,
) {
    let Some(ghost) = p.inline_ghost(tab, cursor) else {
        return;
    };
    let pos = output.galley.pos_from_cursor(range.primary).left_top() + output.galley_pos.to_vec2();
    let color = match ghost.safety {
        Some(application::llm::StatementKind::Read) | None => ui.visuals().weak_text_color(),
        Some(_) => ui.visuals().warn_fg_color,
    };
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let galley = ui
        .painter()
        .layout(ghost.text.clone(), font, color, f32::INFINITY);
    ui.painter().galley(pos, galley, color);
}
