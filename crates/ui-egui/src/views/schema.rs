use eframe::egui;

use crate::bridge::SessionId;
use crate::presenter::{Load, Presenter};
use crate::theme;

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    theme::section_label(ui, "Explorer");
    ui.add(
        egui::TextEdit::singleline(&mut p.explorer_filter)
            .hint_text("Find an object...")
            .desired_width(f32::INFINITY),
    );
    let filter = p.explorer_filter.trim().to_lowercase();
    let mut disconnect: Option<SessionId> = None;
    let mut refresh: Option<SessionId> = None;
    let mut new_tab: Option<SessionId> = None;

    egui::ScrollArea::both()
        .id_salt("schema")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if p.open.is_empty() {
                ui.weak("Connect to a database to browse its schema");
            }
            for open in &p.open {
                egui::CollapsingHeader::new(egui::RichText::new(&open.name).strong())
                    .id_salt(open.session.0)
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button("New query").clicked() {
                                new_tab = Some(open.session);
                            }
                            if ui.small_button("Refresh").clicked() {
                                refresh = Some(open.session);
                            }
                            if ui.small_button("Disconnect").clicked() {
                                disconnect = Some(open.session);
                            }
                        });
                        match &open.schema {
                            Load::Loading => {
                                ui.spinner();
                            }
                            Load::Failed(e) => {
                                ui.colored_label(ui.visuals().error_fg_color, e);
                            }
                            Load::Ready(tree) => tree_view(
                                ui,
                                open.session,
                                tree,
                                &filter,
                                p.visibility.get(&open.connection),
                            ),
                        }
                    });
            }
        });

    if let Some(s) = disconnect {
        p.disconnect(s);
    }
    if let Some(s) = refresh {
        p.refresh_schema(s);
    }
    if let Some(s) = new_tab {
        p.new_tab(s);
    }
}

fn tree_view(
    ui: &mut egui::Ui,
    session: SessionId,
    tree: &domain::SchemaTree,
    filter: &str,
    visible: Option<&Vec<String>>,
) {
    for catalog in &tree.catalogs {
        for schema in catalog
            .schemas
            .iter()
            .filter(|s| visible.is_none_or(|v| v.contains(&s.name)))
        {
            egui::CollapsingHeader::new(&schema.name)
                .id_salt((session.0, &catalog.name, &schema.name))
                .default_open(schema.tables.len() < 40)
                .show(ui, |ui| {
                    for table in schema
                        .tables
                        .iter()
                        .filter(|t| matches_filter(&t.name, filter))
                    {
                        let kind = match table.kind {
                            domain::TableKind::Table => "table",
                            domain::TableKind::View => "view",
                        };
                        egui::CollapsingHeader::new(&table.name)
                            .id_salt((session.0, &schema.name, &table.name))
                            .show(ui, |ui| {
                                ui.weak(kind);
                                for col in &table.columns {
                                    let pk = if table.primary_key.contains(&col.name) {
                                        " PK"
                                    } else {
                                        ""
                                    };
                                    let null = if col.nullable { "" } else { " NOT NULL" };
                                    ui.label(format!(
                                        "{}: {}{pk}{null}",
                                        col.name, col.data_type.0
                                    ));
                                }
                                for fk in &table.foreign_keys {
                                    ui.weak(format!(
                                        "FK ({}) -> {}({})",
                                        fk.columns.join(", "),
                                        fk.referenced_table,
                                        fk.referenced_columns.join(", ")
                                    ));
                                }
                                for ix in &table.indexes {
                                    let unique = if ix.unique { "unique " } else { "" };
                                    ui.weak(format!(
                                        "{unique}index {} ({})",
                                        ix.name,
                                        ix.columns.join(", ")
                                    ));
                                }
                            });
                    }
                });
        }
    }
}

/// Case-insensitive substring match; an empty filter matches everything. `filter` is lower-case.
fn matches_filter(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(filter)
}

#[cfg(test)]
mod tests {
    use super::matches_filter;

    #[test]
    fn empty_filter_matches_everything() {
        assert!(matches_filter("customers", ""));
    }

    #[test]
    fn filter_is_a_case_insensitive_substring() {
        assert!(matches_filter("Order_Items", "item"));
        assert!(!matches_filter("customers", "item"));
    }
}
