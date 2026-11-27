//! The explorer: instances, their schemas and tables as one tree, then saved queries and the
//! link to storage connections (the design's left column).

use eframe::egui::{self, Frame, Margin, Panel, RichText, Sense};

use domain::ConnectionId;

use crate::bridge::SessionId;
use crate::keymap::Action;
use crate::presenter::{Load, Presenter};
use crate::theme;

use super::saved;

/// What the tree asked for this frame. Applied after drawing, because drawing borrows the state.
enum Act {
    Connect(ConnectionId),
    Disconnect(SessionId),
    NewQuery(SessionId),
    Refresh(SessionId),
    Edit(ConnectionId),
    Delete(ConnectionId),
    OpenTable(SessionId, String, String),
}

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let pal = *theme::palette(ui);

    // Saved queries and the storage link stay at the bottom; the tree scrolls above them.
    Panel::bottom("sidebar_bottom")
        .frame(Frame::NONE)
        .show_separator_line(false)
        .show(ui, |ui| {
            saved::sidebar(ui, p);
            ui.add_space(6.0);
            ui.separator();
            let link = ui.add(
                egui::Button::new(
                    RichText::new("\u{25A4}  Storage connections...").color(pal.text_muted),
                )
                .frame(false),
            );
            if link.on_hover_text("Where exports can be written").clicked() {
                p.open_storage_dialog();
            }
        });

    ui.horizontal(|ui| {
        theme::section_label(ui, "Explorer");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(RichText::new("+").size(16.0)).frame(false))
                .on_hover_text("Add a database instance")
                .clicked()
            {
                p.new_connection_form();
            }
            ui.label(
                RichText::new(p.keymap.hint(Action::ToggleExplorer).unwrap_or_default())
                    .size(10.5)
                    .color(pal.text_faint),
            );
        });
    });
    ui.add(
        egui::TextEdit::singleline(&mut p.explorer_filter)
            .hint_text("\u{1F50D}  Find an object...")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        theme::section_label(ui, "Instances");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.link("Manage...").clicked() {
                p.open_instances(None);
            }
        });
    });

    let filter = p.explorer_filter.trim().to_lowercase();
    let mut act: Option<Act> = None;
    egui::ScrollArea::vertical()
        .id_salt("explorer_tree")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if p.connections.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("No instances yet.").color(pal.text_muted));
                if ui.link("Add a database instance").clicked() {
                    p.new_connection_form();
                }
            }
            for c in &p.connections {
                instance(ui, p, c.id, &filter, &mut act);
                ui.add_space(4.0);
            }
        });

    match act {
        Some(Act::Connect(id)) => p.connect(id),
        Some(Act::Disconnect(s)) => p.disconnect(s),
        Some(Act::NewQuery(s)) => {
            p.new_tab(s);
        }
        Some(Act::Refresh(s)) => p.refresh_schema(s),
        Some(Act::Edit(id)) => p.edit_connection(id),
        Some(Act::Delete(id)) => p.delete_connection(id),
        Some(Act::OpenTable(s, schema, table)) => p.open_table(s, &schema, &table),
        None => {}
    }
}

fn instance(
    ui: &mut egui::Ui,
    p: &Presenter,
    id: ConnectionId,
    filter: &str,
    act: &mut Option<Act>,
) {
    let pal = *theme::palette(ui);
    let Some(c) = p.connections.iter().find(|c| c.id == id) else {
        return;
    };
    let open = p.open.iter().find(|o| o.connection == id);
    let connecting = p.connecting.contains(&id);
    let status = if open.is_some() {
        let place = super::place_label(c);
        if place.is_empty() {
            c.driver_kind.as_str().to_owned()
        } else {
            place
        }
    } else if connecting {
        "Connecting...".to_owned()
    } else {
        format!("{} \u{00B7} Disconnected", c.driver_kind.as_str())
    };
    let title = if c.read_only() {
        format!("{} (read-only)", c.name)
    } else {
        c.name.clone()
    };

    let mut header = |ui: &mut egui::Ui| {
        let icon_color = if open.is_some() {
            pal.success
        } else {
            pal.text_faint
        };
        ui.label(RichText::new("\u{1F5C4}").color(icon_color).size(15.0));
        ui.vertical(|ui| {
            let name =
                ui.add(egui::Label::new(RichText::new(&title).strong()).sense(Sense::click()));
            ui.label(RichText::new(&status).size(11.0).color(pal.text_muted));
            // A disconnected instance connects on click; a connected one has its arrow.
            if name.clicked() && open.is_none() && !connecting {
                *act = Some(Act::Connect(id));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(RichText::new("\u{22EF}").color(pal.text_muted), |ui| {
                match open {
                    Some(o) => {
                        if ui.button("New query").clicked() {
                            *act = Some(Act::NewQuery(o.session));
                            ui.close();
                        }
                        if ui.button("Refresh schema").clicked() {
                            *act = Some(Act::Refresh(o.session));
                            ui.close();
                        }
                        if ui.button("Disconnect").clicked() {
                            *act = Some(Act::Disconnect(o.session));
                            ui.close();
                        }
                    }
                    None => {
                        if ui
                            .add_enabled(!connecting, egui::Button::new("Connect"))
                            .clicked()
                        {
                            *act = Some(Act::Connect(id));
                            ui.close();
                        }
                    }
                }
                ui.separator();
                if ui.button("Edit...").clicked() {
                    *act = Some(Act::Edit(id));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    *act = Some(Act::Delete(id));
                    ui.close();
                }
            });
            if connecting {
                ui.spinner();
            }
        });
    };

    match open {
        Some(o) => {
            let state_id = ui.make_persistent_id(("instance", id.0));
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                state_id,
                true,
            )
            .show_header(ui, |ui| header(ui))
            .body(|ui| match &o.schema {
                Load::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading schema...").color(pal.text_muted));
                    });
                }
                Load::Failed(e) => {
                    ui.label(RichText::new(e).color(pal.danger));
                }
                Load::Ready(tree) => schemas(ui, p, o.session, o.connection, tree, filter, act),
            });
        }
        None => {
            ui.horizontal(|ui| header(ui));
        }
    }
}

fn schemas(
    ui: &mut egui::Ui,
    p: &Presenter,
    session: SessionId,
    connection: ConnectionId,
    tree: &domain::SchemaTree,
    filter: &str,
    act: &mut Option<Act>,
) {
    let pal = *theme::palette(ui);
    let multiple: usize = tree.catalogs.iter().map(|c| c.schemas.len()).sum();
    for schema in tree
        .catalogs
        .iter()
        .flat_map(|c| &c.schemas)
        .filter(|s| p.is_database_visible(connection, &s.name))
    {
        let tables: Vec<&domain::Table> = schema
            .tables
            .iter()
            .filter(|t| t.kind == domain::TableKind::Table)
            .filter(|t| filter.is_empty() || t.name.to_lowercase().contains(filter))
            .collect();
        let views: Vec<&domain::Table> = schema
            .tables
            .iter()
            .filter(|t| t.kind == domain::TableKind::View)
            .filter(|t| filter.is_empty() || t.name.to_lowercase().contains(filter))
            .collect();
        if !filter.is_empty() && tables.is_empty() && views.is_empty() {
            continue;
        }
        let id = ui.make_persistent_id(("schema", session.0, &schema.name));
        let open_by_default = multiple <= 3 || !filter.is_empty();
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            open_by_default,
        )
        .show_header(ui, |ui| {
            ui.label(RichText::new(&schema.name).color(pal.text));
        })
        .body(|ui| {
            group(
                ui,
                p,
                session,
                &schema.name,
                "Tables",
                &tables,
                !filter.is_empty() || true,
                act,
            );
            if !views.is_empty() {
                group(
                    ui,
                    p,
                    session,
                    &schema.name,
                    "Views",
                    &views,
                    !filter.is_empty(),
                    act,
                );
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn group(
    ui: &mut egui::Ui,
    p: &Presenter,
    session: SessionId,
    schema: &str,
    title: &str,
    tables: &[&domain::Table],
    open: bool,
    act: &mut Option<Act>,
) {
    let pal = *theme::palette(ui);
    let id = ui.make_persistent_id(("group", session.0, schema, title));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open)
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).color(pal.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(tables.len().to_string())
                        .size(11.0)
                        .color(pal.text_muted),
                );
            });
        })
        .body(|ui| {
            for t in tables {
                if table_row(ui, p, session, schema, t) {
                    *act = Some(Act::OpenTable(session, schema.to_owned(), t.name.clone()));
                }
            }
        });
}

/// One table. A click opens `SELECT * ... LIMIT 100` in the editor (without running it); the
/// tooltip lists the columns.
fn table_row(
    ui: &mut egui::Ui,
    p: &Presenter,
    session: SessionId,
    schema: &str,
    table: &domain::Table,
) -> bool {
    let pal = *theme::palette(ui);
    let selected =
        p.explorer_selected.as_ref() == Some(&(session, schema.to_owned(), table.name.clone()));
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
    if selected || response.hovered() {
        let fill = if selected {
            pal.accent_soft
        } else {
            pal.chrome
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), fill);
    }
    let color = if selected { pal.accent } else { pal.text };
    ui.painter().text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "\u{25A6}",
        egui::FontId::proportional(13.0),
        pal.text_muted,
    );
    ui.painter().text(
        rect.left_center() + egui::vec2(30.0, 0.0),
        egui::Align2::LEFT_CENTER,
        &table.name,
        egui::FontId::proportional(13.0),
        color,
    );
    let clicked = response.clicked();
    response.on_hover_ui(|ui| {
        ui.label(RichText::new(&table.name).strong());
        for col in &table.columns {
            let pk = if table.primary_key.contains(&col.name) {
                "  PK"
            } else {
                ""
            };
            let null = if col.nullable { "" } else { "  NOT NULL" };
            ui.label(format!("{}  {}{pk}{null}", col.name, col.data_type.0));
        }
    });
    clicked
}

#[allow(dead_code)]
fn _frame_margin() -> Margin {
    Margin::ZERO
}
