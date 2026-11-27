use eframe::egui::{self, Frame, Margin, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use application::export::ExportFormat;

use crate::presenter::{NULL_TEXT, Presenter, ReviewKind, TabState};
use crate::theme;

const ROW_HEIGHT: f32 = 22.0;

/// What the grid asked for this frame. Applied after drawing, because drawing borrows the result.
enum GridAction {
    SetDraft(String),
    BeginEdit(usize, usize),
    CommitEdit,
    CancelEdit,
    ToggleRow(usize),
    Unstage(usize),
}

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let mut fix = false;
    let Some(tab_id) = p.selected else { return };
    let done = matches!(p.selected_tab().map(|t| &t.state), Some(TabState::Done(_)));
    let source = if done { p.grid_source(tab_id) } else { None };
    let grid = p.grid_snapshot(tab_id);
    let mut actions: Vec<GridAction> = Vec::new();
    let pal = *theme::palette(ui);
    let Some(tab) = p.selected_tab() else { return };
    match &tab.state {
        TabState::Idle => {
            ui.weak("Run a query to see results");
        }
        TabState::Running => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running...");
            });
        }
        TabState::Failed(message) => {
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
                ui.label(format!(
                    "{n} row(s) affected in {} ms",
                    result.elapsed.as_millis()
                ));
                return;
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Results").strong());
                ui.label(
                    egui::RichText::new(result.rows.len().to_string())
                        .background_color(pal.surface)
                        .color(pal.text_muted),
                );
                ui.label(
                    egui::RichText::new(format!("\u{2713} {} ms", result.elapsed.as_millis()))
                        .color(pal.success),
                );
                if result.truncated {
                    ui.label(
                        egui::RichText::new("limited: more rows exist; Export saves more")
                            .color(pal.warning),
                    );
                }
                if source.is_some() {
                    ui.label(
                        egui::RichText::new("Double-click a cell to edit")
                            .size(11.0)
                            .color(pal.text_faint),
                    );
                }
            });
            ui.add_space(4.0);

            let editable = source.is_some();
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

            // Only the visible rows are laid out, so large results stay responsive.
            let mut table = TableBuilder::new(ui)
                .id_salt(("results", tab.id.0))
                .striped(!editable)
                .resizable(true)
                .max_scroll_height(f32::INFINITY);
            if editable {
                table = table.column(Column::exact(28.0));
            }
            table
                .columns(
                    Column::initial(140.0).at_least(40.0).clip(true),
                    result.columns.len(),
                )
                .header(ROW_HEIGHT, |mut header| {
                    if editable {
                        header.col(|_| {});
                    }
                    for col in &result.columns {
                        header.col(|ui| {
                            ui.label(
                                egui::RichText::new(&col.name)
                                    .color(theme::palette(ui).text_muted)
                                    .strong(),
                            )
                            .on_hover_text(&col.data_type.0);
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_HEIGHT, result.rows.len(), |mut row| {
                        let r = row.index();
                        let deleted = grid.deleted.contains(&r);
                        let fill = if deleted {
                            Some(pal.danger_soft)
                        } else if grid.selected.contains(&r) {
                            Some(pal.accent_soft)
                        } else {
                            None
                        };
                        if editable {
                            row.col(|ui| {
                                paint_fill(ui, fill);
                                let mut on = grid.selected.contains(&r);
                                if ui.checkbox(&mut on, "").changed() {
                                    actions.push(GridAction::ToggleRow(r));
                                }
                            });
                        }
                        for (c, value) in result.rows[r].iter().enumerate() {
                            row.col(|ui| {
                                paint_fill(ui, fill);
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
                                let staged = grid.edits.get(&(r, c));
                                let mut text = match (staged, value.is_null()) {
                                    (Some(s), _) => RichText::new(s.clone()),
                                    (None, true) => RichText::new(NULL_TEXT)
                                        .italics()
                                        .color(ui.visuals().weak_text_color()),
                                    (None, false) => RichText::new(value.display()),
                                };
                                if staged.is_some() {
                                    text =
                                        text.background_color(pal.warning_soft).color(pal.warning);
                                }
                                if deleted {
                                    text = text.strikethrough().color(pal.danger);
                                }
                                let response =
                                    ui.add(egui::Label::new(text).truncate().sense(Sense::click()));
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
        }
    }
    if fix {
        p.ai_fix_last_error();
    }
    for action in actions {
        match action {
            GridAction::SetDraft(text) => p.set_draft(tab_id, text),
            GridAction::BeginEdit(r, c) => p.begin_cell_edit(tab_id, r, c),
            GridAction::CommitEdit => p.commit_cell_edit(tab_id),
            GridAction::CancelEdit => p.cancel_cell_edit(tab_id),
            GridAction::ToggleRow(r) => p.toggle_row_selected(tab_id, r),
            GridAction::Unstage(r) => p.unstage_delete(tab_id, r),
        }
    }
}

fn paint_fill(ui: &egui::Ui, fill: Option<egui::Color32>) {
    if let Some(fill) = fill {
        ui.painter()
            .rect_filled(ui.max_rect().expand2(egui::vec2(4.0, 1.0)), 0.0, fill);
    }
}

/// Row actions and the pending-changes bar. Shown only for results that can be edited.
pub fn edit_bar(ui: &mut egui::Ui, p: &mut Presenter) {
    let Some(tab) = p.selected else { return };
    if !matches!(p.selected_tab().map(|t| &t.state), Some(TabState::Done(_))) {
        return;
    }
    if p.grid_source(tab).is_none() {
        return;
    }
    let pal = *theme::palette(ui);
    let grid = p.grid_snapshot(tab);
    let (edits, deletions) = p.pending_counts(tab);
    let (mut delete, mut discard, mut review) = (false, false, false);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} selected", grid.selected.len())).color(pal.text_muted),
        );
        delete = ui
            .add_enabled(!grid.selected.is_empty(), egui::Button::new("Delete rows"))
            .on_hover_text("Marks the ticked rows for deletion. Nothing is deleted until you review and apply.")
            .clicked();
        if grid.has_pending() {
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "\u{25CF} {} pending change{} ({edits} edit{}, {deletions} deletion{})",
                    edits + deletions,
                    if edits + deletions == 1 { "" } else { "s" },
                    if edits == 1 { "" } else { "s" },
                    if deletions == 1 { "" } else { "s" }
                ))
                .color(pal.warning)
                .strong(),
            )
            .on_hover_text("Not applied to the database until you review and apply.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                review = ui.add(theme::primary_button("Review & apply")).clicked();
                discard = ui.button("Discard").clicked();
            });
        }
    });
    if delete {
        p.stage_delete_selected(tab);
    }
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
                RichText::new(format!("{} changes to {table}", lines.len())).color(pal.text_muted),
            );
            ui.add_space(8.0);
            Frame::NONE
                .fill(pal.surface)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(connection).strong());
                });
            ui.add_space(8.0);
            // Room for every line (up to the cap): the window itself has no fixed height.
            let wanted = (lines.len() as f32 * 56.0).min(300.0);
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .min_scrolled_height(wanted)
                .auto_shrink([false, true])
                .show(ui, |ui| {
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
                    let label = format!("Apply {} changes", lines.len());
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
    if apply {
        p.apply_staged(tab);
    } else if cancel || escape {
        p.close_review(tab);
    }
}

/// Export controls above the grid: format, file name and the button. Shown only for a result
/// with columns. The export re-runs the statement, so it can hold more rows than the grid.
pub fn export_controls(ui: &mut egui::Ui, p: &mut Presenter) {
    let exportable = p
        .selected_tab()
        .is_some_and(|t| matches!(&t.state, TabState::Done(r) if !r.columns.is_empty()));
    if !exportable {
        return;
    }
    let mut export = false;
    ui.horizontal(|ui| {
        ui.label("Export");
        egui::ComboBox::from_id_salt("export_format")
            .width(70.0)
            .selected_text(match p.export.format {
                ExportFormat::Csv => "CSV",
                ExportFormat::Json => "JSON",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut p.export.format, ExportFormat::Csv, "CSV");
                ui.selectable_value(&mut p.export.format, ExportFormat::Json, "JSON");
            });
        ui.add(
            egui::TextEdit::singleline(&mut p.export.name)
                .hint_text("file name (optional)")
                .desired_width(170.0),
        );
        export = ui
            .add_enabled(!p.export.running, egui::Button::new("Save"))
            .on_hover_text(
                "Re-runs the query and writes a new file. Existing files are never overwritten.",
            )
            .clicked();
        if p.export.running {
            ui.spinner();
        }
    });
    if export {
        p.export_selected();
    }
}
