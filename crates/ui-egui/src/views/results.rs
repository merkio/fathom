use eframe::egui::{self, Frame, Margin, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::presenter::{NULL_TEXT, Presenter, ReviewKind, TabState};
use crate::theme;

const ROW_HEIGHT: f32 = 30.0;

/// What the grid asked for this frame. Applied after drawing, because drawing borrows the result.
enum GridAction {
    SetDraft(String),
    BeginEdit(usize, usize),
    CommitEdit,
    CancelEdit,
    ToggleRow(usize),
    SelectAll,
    Unstage(usize),
    Copy,
    EditRow,
    DeleteRows,
    ToggleFilter,
    SetFilter(String),
    Refresh,
    Export,
}

/// A toolbar button: an icon and a label, outlined like the design's.
fn tool_button(ui: &mut egui::Ui, icon: &str, label: &str, enabled: bool) -> egui::Response {
    let pal = *theme::palette(ui);
    let text = RichText::new(format!("{icon}  {label}")).color(if enabled {
        pal.text
    } else {
        pal.text_faint
    });
    ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(egui::vec2(0.0, 30.0)),
    )
}

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let mut fix = false;
    let Some(tab_id) = p.selected else { return };
    let done = matches!(p.selected_tab().map(|t| &t.state), Some(TabState::Done(_)));
    let source = if done { p.grid_source(tab_id) } else { None };
    let grid = p.grid_snapshot(tab_id);
    let visible = p.visible_rows(tab_id);
    let mut actions: Vec<GridAction> = Vec::new();
    let pal = *theme::palette(ui);
    let Some(tab) = p.selected_tab() else { return };
    match &tab.state {
        TabState::Idle => {
            ui.add_space(8.0);
            ui.label(RichText::new("Run a query to see results").color(pal.text_muted));
        }
        TabState::Running => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running...");
            });
        }
        TabState::Failed(message) => {
            ui.add_space(8.0);
            ui.colored_label(ui.visuals().error_fg_color, message);
            fix = ui
                .button("Fix with AI")
                .on_hover_text(
                    "Sends the query, the error and your schema (no data) to your AI provider",
                )
                .clicked();
        }
        TabState::Done(result) => {
            if result.columns.is_empty() {
                let n = result.rows_affected.unwrap_or(0);
                ui.add_space(8.0);
                ui.label(format!(
                    "{n} row(s) affected in {} ms",
                    result.elapsed.as_millis()
                ));
                return;
            }
            let editable = source.is_some();
            // ---- toolbar -------------------------------------------------------------------
            ui.horizontal(|ui| {
                ui.set_min_height(36.0);
                ui.label(RichText::new("Results").strong());
                ui.label(
                    RichText::new(format!(" {} ", result.rows.len()))
                        .background_color(pal.surface)
                        .color(pal.text_muted),
                );
                ui.label(
                    RichText::new(format!("\u{2713}  {} ms", result.elapsed.as_millis()))
                        .color(pal.success),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if tool_button(ui, "\u{2B06}", "Export...", true).clicked() {
                        actions.push(GridAction::Export);
                    }
                    if tool_button(ui, "\u{21BB}", "Refresh", true).clicked() {
                        actions.push(GridAction::Refresh);
                    }
                    if tool_button(ui, "\u{25BE}", "Filter", true).clicked() {
                        actions.push(GridAction::ToggleFilter);
                    }
                    ui.separator();
                    let any = !grid.selected.is_empty();
                    if tool_button(ui, "\u{232B}", "Delete", any && editable)
                        .on_hover_text(if editable {
                            "Mark the ticked rows for deletion; nothing is deleted until you apply"
                        } else {
                            "These results cannot be edited (see the README)"
                        })
                        .clicked()
                    {
                        actions.push(GridAction::DeleteRows);
                    }
                    if tool_button(ui, "\u{270E}", "Edit", any && editable).clicked() {
                        actions.push(GridAction::EditRow);
                    }
                    if tool_button(ui, "\u{29C9}", "Copy", any)
                        .on_hover_text("Copy the ticked rows (Ctrl+C)")
                        .clicked()
                    {
                        actions.push(GridAction::Copy);
                    }
                    ui.label(
                        RichText::new(format!("{} selected", grid.selected.len()))
                            .size(11.0)
                            .color(pal.text_muted),
                    );
                });
            });
            if grid.filter_open {
                let mut text = grid.filter.clone();
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("Filter rows (any column)")
                        .desired_width(f32::INFINITY),
                );
                if !edit.has_focus() && text.is_empty() {
                    edit.request_focus();
                }
                if text != grid.filter {
                    actions.push(GridAction::SetFilter(text));
                }
            }
            let key_columns: Vec<bool> = result
                .columns
                .iter()
                .map(|c| {
                    source.as_ref().is_some_and(|s| {
                        s.primary_key
                            .iter()
                            .any(|k| k.eq_ignore_ascii_case(&c.name))
                    })
                })
                .collect();

            // Only the visible rows are laid out, so large results stay responsive. Cells touch
            // each other (no gap) so backgrounds and the grid lines are continuous.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let last = result.columns.len().saturating_sub(1);
                TableBuilder::new(ui)
                    .id_salt(("results", tab.id.0))
                    .striped(false)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .max_scroll_height(f32::INFINITY)
                    .column(Column::exact(36.0))
                    .columns(Column::initial(150.0).at_least(40.0).clip(true), last)
                    .column(Column::remainder().at_least(80.0).clip(true))
                    .header(ROW_HEIGHT, |mut header| {
                        header.col(|ui| {
                            paint_cell(ui, Some(pal.surface), &pal);
                            let all = !visible.is_empty()
                                && visible.iter().all(|r| grid.selected.contains(r));
                            let mut on = all;
                            if ui.checkbox(&mut on, "").changed() {
                                actions.push(GridAction::SelectAll);
                            }
                        });
                        for col in &result.columns {
                            header.col(|ui| {
                                paint_cell(ui, Some(pal.surface), &pal);
                                ui.label(
                                    RichText::new(&col.name).color(theme::palette(ui).text_muted),
                                )
                                .on_hover_text(&col.data_type.0);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, visible.len(), |mut row| {
                            let r = visible[row.index()];
                            let deleted = grid.deleted.contains(&r);
                            let fill = if deleted {
                                Some(pal.danger_soft)
                            } else if grid.selected.contains(&r) {
                                Some(pal.accent_soft)
                            } else {
                                None
                            };
                            row.col(|ui| {
                                paint_cell(ui, fill, &pal);
                                if grid.selected.contains(&r) && !deleted {
                                    // The design marks a ticked row with an accent bar on its left edge.
                                    let rect = ui.max_rect();
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_size(
                                            rect.left_top() - egui::vec2(4.0, 1.0),
                                            egui::vec2(3.0, rect.height() + 2.0),
                                        ),
                                        0.0,
                                        pal.accent,
                                    );
                                }
                                let mut on = grid.selected.contains(&r);
                                if ui.checkbox(&mut on, "").changed() {
                                    actions.push(GridAction::ToggleRow(r));
                                }
                            });
                            for (c, value) in result.rows[r].iter().enumerate() {
                                row.col(|ui| {
                                    paint_cell(ui, fill, &pal);
                                    if grid.editing == Some((r, c)) {
                                        let mut draft = grid.draft.clone();
                                        let edit = ui.add(
                                            egui::TextEdit::singleline(&mut draft)
                                                .desired_width(f32::INFINITY),
                                        );
                                        if !edit.has_focus() && !edit.lost_focus() {
                                            edit.request_focus();
                                        }
                                        if draft != grid.draft {
                                            actions.push(GridAction::SetDraft(draft));
                                        }
                                        if edit.lost_focus() {
                                            let escaped =
                                                ui.input(|i| i.key_pressed(egui::Key::Escape));
                                            actions.push(if escaped {
                                                GridAction::CancelEdit
                                            } else {
                                                GridAction::CommitEdit
                                            });
                                        }
                                        return;
                                    }
                                    ui.add_space(10.0);
                                    let staged = grid.edits.get(&(r, c));
                                    let mut text = match (staged, value.is_null()) {
                                        (Some(s), _) => RichText::new(s.clone()),
                                        (None, true) => RichText::new(NULL_TEXT)
                                            .italics()
                                            .color(ui.visuals().weak_text_color()),
                                        (None, false) => RichText::new(value.display()),
                                    };
                                    if staged.is_some() {
                                        paint_cell(ui, Some(pal.warning_soft), &pal);
                                        text = text.color(pal.text);
                                    }
                                    if deleted {
                                        text = text.strikethrough().color(pal.danger);
                                    }
                                    let response = ui.add(
                                        egui::Label::new(text).truncate().sense(Sense::click()),
                                    );
                                    if editable && !key_columns[c] && response.double_clicked() {
                                        actions.push(GridAction::BeginEdit(r, c));
                                    }
                                    if deleted && response.double_clicked() {
                                        actions.push(GridAction::Unstage(r));
                                    }
                                });
                            }
                        });
                    });
            });
        }
    }
    if fix {
        p.ai_fix_last_error();
    }
    // Keys that act on the grid, unless a text box has the keyboard.
    if done && !ui.ctx().egui_wants_keyboard_input() {
        let (copy, delete) = ui.input(|i| {
            (
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if copy {
            actions.push(GridAction::Copy);
        }
        if delete {
            actions.push(GridAction::DeleteRows);
        }
    }
    for action in actions {
        match action {
            GridAction::SetDraft(text) => p.set_draft(tab_id, text),
            GridAction::BeginEdit(r, c) => p.begin_cell_edit(tab_id, r, c),
            GridAction::CommitEdit => p.commit_cell_edit(tab_id),
            GridAction::CancelEdit => p.cancel_cell_edit(tab_id),
            GridAction::ToggleRow(r) => p.toggle_row_selected(tab_id, r),
            GridAction::SelectAll => p.toggle_select_all(tab_id),
            GridAction::Unstage(r) => p.unstage_delete(tab_id, r),
            GridAction::Copy => {
                if let Some(text) = p.copy_selected_text(tab_id) {
                    ui.ctx().copy_text(text);
                    p.status_info("Copied the ticked rows");
                }
            }
            GridAction::EditRow => p.edit_selected_row(tab_id),
            GridAction::DeleteRows => {
                if p.grid_source(tab_id).is_some() {
                    p.stage_delete_selected(tab_id);
                }
            }
            GridAction::ToggleFilter => p.toggle_filter(tab_id),
            GridAction::SetFilter(text) => p.set_filter(tab_id, text),
            GridAction::Refresh => p.refresh_results(tab_id),
            GridAction::Export => p.open_export_dialog(),
        }
    }
}

/// A cell's background and its grid lines (bottom and right edge).
fn paint_cell(ui: &egui::Ui, fill: Option<egui::Color32>, pal: &theme::Palette) {
    let rect = ui.max_rect();
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 0.0, fill);
    }
    let stroke = egui::Stroke::new(1.0, pal.border);
    ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
    ui.painter().vline(rect.right(), rect.y_range(), stroke);
}

/// "Showing 1-N of M rows" and the hints for what the keyboard does in the grid.
pub fn footer(ui: &mut egui::Ui, p: &mut Presenter) {
    let Some(tab) = p.selected else { return };
    let Some(total) = p.selected_tab().and_then(|t| match &t.state {
        TabState::Done(r) if !r.columns.is_empty() => Some((r.rows.len(), r.truncated)),
        _ => None,
    }) else {
        return;
    };
    let pal = *theme::palette(ui);
    let shown = p.visible_rows(tab).len();
    let editable = p.grid_source(tab).is_some();
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        let range = if shown == 0 {
            "0".to_owned()
        } else {
            format!("1\u{2013}{shown}")
        };
        ui.label(
            RichText::new(format!("Showing {range} of {} rows", total.0))
                .size(11.0)
                .color(pal.text_muted),
        );
        if total.1 {
            ui.label(
                RichText::new("limited: more rows exist, Export saves more")
                    .size(11.0)
                    .color(pal.warning),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let hint = if editable {
                "Double-click to edit \u{00B7} Ctrl+C copy \u{00B7} Del stage deletion"
            } else {
                "Ctrl+C copy"
            };
            ui.label(RichText::new(hint).size(11.0).color(pal.text_muted));
        });
    });
}

/// The pending-changes bar at the bottom of the results: drawn only while edits are staged.
pub fn pending_bar(ui: &mut egui::Ui, p: &mut Presenter) {
    let Some(tab) = p.selected else { return };
    let pal = *theme::palette(ui);
    let grid = p.grid_snapshot(tab);
    if !grid.has_pending() {
        return;
    }
    let (edits, deletions) = p.pending_counts(tab);
    let (mut discard, mut review) = (false, false);
    ui.horizontal(|ui| {
        ui.set_min_height(44.0);
        ui.label(
            RichText::new(format!(
                "\u{25CF} {} pending change{}",
                edits + deletions,
                if edits + deletions == 1 { "" } else { "s" }
            ))
            .color(pal.warning)
            .strong(),
        );
        ui.label(
            RichText::new(format!(
                "{edits} edit{} \u{00B7} {deletions} deletion{} \u{00B7} not applied to database",
                if edits == 1 { "" } else { "s" },
                if deletions == 1 { "" } else { "s" }
            ))
            .size(11.0)
            .color(pal.text_muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Enter,
            ));
            review = ui
                .add(theme::primary_button(format!(
                    "Review & apply   {shortcut}"
                )))
                .clicked();
            discard = ui
                .add(egui::Button::new(RichText::new("Discard").color(pal.text_muted)).frame(false))
                .clicked();
        });
    });
    if discard {
        p.discard_changes(tab);
    }
    if review {
        p.open_review(tab);
    }
}

/// The "Review changes" dialog. Draws nothing unless the user opened it.
pub fn review_dialog(ctx: &egui::Context, p: &mut Presenter) {
    let Some(tab) = p.selected else { return };
    let grid = p.grid_snapshot(tab);
    if !grid.review_open {
        return;
    }
    let pal = *theme::palette_for(ctx.theme());
    let lines = p.review_lines(tab);
    let atomic = p.applies_atomically(tab);
    let table = p
        .grid_source(tab)
        .map(|s| format!("{}.{}", s.schema, s.table))
        .unwrap_or_default();
    let deletions = lines
        .iter()
        .filter(|l| l.kind == ReviewKind::Delete)
        .count();
    let connection = p
        .selected_tab()
        .and_then(|t| p.open.iter().find(|o| o.session == t.session))
        .map(|o| o.name.clone())
        .unwrap_or_default();
    let (mut apply, mut cancel) = (false, false);
    let mut unstage_pick: Option<usize> = None;
    let escape = p.palette.is_none()
        && p.instances.is_none()
        && p.form.is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

    egui::Window::new("review_changes")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([740.0, 0.0])
        .frame(
            Frame::NONE
                .fill(pal.bg)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(egui::CornerRadius::same(12))
                .shadow(ctx.global_style().visuals.popup_shadow)
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.label(RichText::new("Review changes").size(20.0).strong());
            ui.label(
                RichText::new(format!(
                    "{} change{} to {table}",
                    lines.len(),
                    if lines.len() == 1 { "" } else { "s" }
                ))
                .color(pal.text_muted),
            );
            ui.add_space(8.0);
            Frame::NONE
                .fill(pal.surface)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(&connection).strong());
                });
            ui.add_space(8.0);
            // Room for every line (up to the cap): the window itself has no fixed height.
            let wanted = (44.0 + lines.len() as f32 * 56.0).min(320.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .min_scrolled_height(wanted)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (label, width) in [("OPERATION", 76.0), ("ROW", 96.0), ("CHANGE", 0.0)] {
                            let text = RichText::new(label).size(10.5).color(pal.text_muted).strong();
                            if width > 0.0 {
                                ui.add_sized([width, 16.0], egui::Label::new(text));
                            } else {
                                ui.label(text);
                            }
                        }
                    });
                    ui.separator();
                    for line in &lines {
                        ui.horizontal_top(|ui| {
                            let (label, color) = match line.kind {
                                ReviewKind::Update => ("Update", pal.warning),
                                ReviewKind::Delete => ("Delete", pal.danger),
                            };
                            ui.add_sized(
                                [70.0, 18.0],
                                egui::Label::new(RichText::new(label).color(color)),
                            );
                            ui.add_sized(
                                [90.0, 18.0],
                                egui::Label::new(RichText::new(&line.row).color(pal.text_muted)),
                            );
                            ui.vertical(|ui| match line.kind {
                                ReviewKind::Update => {
                                    for (column, old, new) in &line.columns {
                                        ui.label(RichText::new(column).strong());
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(old)
                                                    .strikethrough()
                                                    .color(pal.text_muted),
                                            );
                                            ui.label("\u{2192}");
                                            ui.label(RichText::new(new).color(pal.success));
                                        });
                                    }
                                }
                                ReviewKind::Delete => {
                                    ui.label(RichText::new(&line.label).strong());
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                if ui
                                    .add(egui::Button::new(RichText::new("\u{2715}").color(pal.text_faint)).frame(false))
                                    .on_hover_text("Take this change back")
                                    .clicked()
                                {
                                    unstage_pick = Some(line.index);
                                }
                            });
                        });
                        ui.separator();
                    }
                });
            ui.add_space(6.0);
            Frame::NONE
                .fill(pal.warning_soft)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if deletions > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{deletions} row{} will be permanently deleted.",
                                if deletions == 1 { "" } else { "s" }
                            ))
                            .color(pal.warning)
                            .strong(),
                        );
                    }
                    let note = if atomic {
                        "All changes run in one transaction. If any change fails, none are applied."
                    } else {
                        "This database has no transactions: changes are applied one by one, and a failure leaves the earlier ones applied."
                    };
                    ui.label(RichText::new(note).size(11.0).color(pal.warning));
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Nothing has been applied yet.")
                        .size(11.0)
                        .color(pal.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = format!(
                        "Apply {} change{}",
                        lines.len(),
                        if lines.len() == 1 { "" } else { "s" }
                    );
                    apply = ui
                        .add_enabled(!grid.applying, theme::primary_button(label))
                        .clicked();
                    cancel = ui
                        .add_enabled(!grid.applying, egui::Button::new("Cancel"))
                        .clicked();
                    if grid.applying {
                        ui.spinner();
                    }
                });
            });
        });
    if let Some(row) = unstage_pick {
        p.unstage_row(tab, row);
    }
    if apply {
        p.apply_staged(tab);
    } else if cancel || escape {
        p.close_review(tab);
    }
}
