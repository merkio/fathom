//! `SchemaContextBuilder`: renders the part of the schema an LLM needs, within a token budget.
//!
//! Only metadata is rendered (names, types, keys). Tables are ranked by relevance to the
//! question and to any SQL already in the editor, so large schemas still give the model the
//! tables that matter, plus their foreign-key neighbours so joins stay possible.

use std::collections::HashSet;

use domain::{ForeignKey, SchemaTree, Table, TableKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaContext {
    pub text: String,
    pub included_tables: usize,
    pub total_tables: usize,
}

impl SchemaContext {
    pub fn is_truncated(&self) -> bool {
        self.included_tables < self.total_tables
    }
}

pub struct SchemaContextBuilder {
    token_budget: usize,
}

/// Rough token estimate: about four characters per token.
fn tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

fn words(text: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for w in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let w = w.to_lowercase();
        if w.len() < 2 {
            continue;
        }
        if let Some(stem) = w.strip_suffix("ies") {
            set.insert(format!("{stem}y"));
        } else if let Some(stem) = w.strip_suffix("es") {
            set.insert(stem.to_owned());
        }
        if let Some(stem) = w.strip_suffix('s') {
            set.insert(stem.to_owned());
        }
        set.insert(w);
    }
    set
}

fn name_matches(name: &str, hint_words: &HashSet<String>) -> bool {
    let lower = name.to_lowercase();
    hint_words.contains(&lower)
        || lower
            .split('_')
            .filter(|p| p.len() > 2)
            .any(|p| hint_words.contains(p))
}

fn score(table: &Table, hint_words: &HashSet<String>) -> u32 {
    let mut s = 0;
    if name_matches(&table.name, hint_words) {
        s += 10;
    }
    let column_hits = table
        .columns
        .iter()
        .filter(|c| name_matches(&c.name, hint_words))
        .count();
    s + 3 * column_hits.min(3) as u32
}

fn fk_text(fk: &ForeignKey) -> String {
    format!(
        "FK({})->{}({})",
        fk.columns.join(","),
        fk.referenced_table,
        fk.referenced_columns.join(",")
    )
}

fn render(schema: &str, table: &Table, max_columns: Option<usize>) -> String {
    let mut columns: Vec<&domain::Column> = table.columns.iter().collect();
    let truncated = max_columns.is_some_and(|m| columns.len() > m);
    if let Some(m) = max_columns {
        // Keep key columns when cutting.
        let is_key = |c: &domain::Column| {
            table.primary_key.contains(&c.name)
                || table
                    .foreign_keys
                    .iter()
                    .any(|fk| fk.columns.contains(&c.name))
        };
        columns.sort_by_key(|c| !is_key(c));
        columns.truncate(m);
    }
    let cols: Vec<String> = columns
        .iter()
        .map(|c| {
            let mut s = format!("{} {}", c.name, c.data_type.0);
            if table.primary_key.contains(&c.name) {
                s.push_str(" PK");
            }
            if !c.nullable {
                s.push_str(" NOT NULL");
            }
            s
        })
        .collect();
    let kind = if table.kind == TableKind::View {
        "view "
    } else {
        ""
    };
    let more = if truncated { ", ..." } else { "" };
    let fks: String = table
        .foreign_keys
        .iter()
        .map(|fk| format!(" {}", fk_text(fk)))
        .collect::<Vec<_>>()
        .join("");
    format!(
        "{kind}{schema}.{}({}{more}){fks}",
        table.name,
        cols.join(", ")
    )
}

impl SchemaContextBuilder {
    pub fn new(token_budget: usize) -> Self {
        Self { token_budget }
    }

    /// `hints` is free text that signals relevance: the user's question and/or the SQL in the editor.
    pub fn build(&self, tree: &SchemaTree, hints: &str) -> SchemaContext {
        let hint_words = words(hints);
        let tables: Vec<(&str, &Table)> = tree.tables().collect();
        let total = tables.len();

        let mut scores: Vec<u32> = tables.iter().map(|(_, t)| score(t, &hint_words)).collect();
        // Foreign-key neighbours of relevant tables keep joins possible.
        let relevant: Vec<usize> = (0..total).filter(|i| scores[*i] >= 10).collect();
        for &i in &relevant {
            let t = tables[i].1;
            for (j, (_, other)) in tables.iter().enumerate() {
                let linked = t
                    .foreign_keys
                    .iter()
                    .any(|fk| fk.referenced_table.eq_ignore_ascii_case(&other.name))
                    || other
                        .foreign_keys
                        .iter()
                        .any(|fk| fk.referenced_table.eq_ignore_ascii_case(&t.name));
                if linked && j != i {
                    scores[j] += 2;
                }
            }
        }

        let mut order: Vec<usize> = (0..total).collect();
        order.sort_by(|&a, &b| {
            scores[b].cmp(&scores[a]).then_with(|| {
                (tables[a].0, &tables[a].1.name).cmp(&(tables[b].0, &tables[b].1.name))
            })
        });

        let mut used = 0usize;
        let mut chosen: Vec<(usize, String)> = Vec::new();
        for i in order {
            let (schema, table) = tables[i];
            let full = render(schema, table, None);
            let cost = tokens(&full) + 1;
            if used + cost <= self.token_budget {
                used += cost;
                chosen.push((i, full));
                continue;
            }
            // Try a shortened version with the key columns first.
            for max in [12, 6, 3] {
                let short = render(schema, table, Some(max));
                let cost = tokens(&short) + 1;
                if used + cost <= self.token_budget {
                    used += cost;
                    chosen.push((i, short));
                    break;
                }
            }
        }
        // Stable output order, independent of relevance.
        chosen.sort_by(|a, b| {
            (tables[a.0].0, &tables[a.0].1.name).cmp(&(tables[b.0].0, &tables[b.0].1.name))
        });

        let included = chosen.len();
        let mut text = chosen
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n");
        if included < total {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!(
                "-- {} more table(s) omitted to fit the size limit",
                total - included
            ));
        }
        SchemaContext {
            text,
            included_tables: included,
            total_tables: total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Catalog, Column, DataType, Schema};

    fn table(name: &str, cols: &[&str]) -> Table {
        let mut t = Table::new(name, TableKind::Table);
        t.columns = cols
            .iter()
            .map(|c| Column {
                name: (*c).into(),
                data_type: DataType::new("text"),
                nullable: true,
                default: None,
            })
            .collect();
        t
    }

    fn tree(tables: Vec<Table>) -> SchemaTree {
        SchemaTree {
            catalogs: vec![Catalog {
                name: "d".into(),
                schemas: vec![Schema {
                    name: "public".into(),
                    tables,
                }],
            }],
        }
    }

    #[test]
    fn everything_fits_is_rendered_in_stable_order() {
        let mut orders = table("orders", &["id", "customer_id"]);
        orders.primary_key = vec!["id".into()];
        orders.foreign_keys.push(ForeignKey {
            name: "fk".into(),
            columns: vec!["customer_id".into()],
            referenced_schema: None,
            referenced_table: "customers".into(),
            referenced_columns: vec!["id".into()],
        });
        let ctx = SchemaContextBuilder::new(1000)
            .build(&tree(vec![orders, table("customers", &["id"])]), "");
        assert_eq!(
            ctx.text,
            "public.customers(id text)\npublic.orders(id text PK, customer_id text) FK(customer_id)->customers(id)"
        );
        assert!(!ctx.is_truncated());
    }

    #[test]
    fn relevant_tables_win_when_the_budget_is_tight() {
        let tables: Vec<Table> = (0..40)
            .map(|i| table(&format!("noise_{i}"), &["a", "b", "c", "d"]))
            .chain([table("invoices", &["id", "amount"])])
            .collect();
        let ctx = SchemaContextBuilder::new(60)
            .build(&tree(tables), "total amount of invoices per month");
        assert!(ctx.text.contains("invoices"), "{}", ctx.text);
        assert!(ctx.is_truncated() && ctx.text.contains("more table(s) omitted"));
        assert!(
            tokens(&ctx.text) <= 60 + 12,
            "stays near the budget: {}",
            tokens(&ctx.text)
        );
    }

    #[test]
    fn plural_and_snake_case_names_match_the_question() {
        let tables = vec![
            table("order_items", &["id"]),
            table("customers", &["id"]),
            table("logs", &["id"]),
        ];
        let ctx = SchemaContextBuilder::new(17)
            .build(&tree(tables), "how many customers have an order item");
        assert!(
            ctx.text.contains("customers") && ctx.text.contains("order_items"),
            "{}",
            ctx.text
        );
        assert!(!ctx.text.contains("logs("), "{}", ctx.text);
    }

    #[test]
    fn foreign_key_neighbours_are_kept_for_joins() {
        let mut orders = table("orders", &["id", "customer_id"]);
        orders.foreign_keys.push(ForeignKey {
            name: "fk".into(),
            columns: vec!["customer_id".into()],
            referenced_schema: None,
            referenced_table: "customers".into(),
            referenced_columns: vec!["id".into()],
        });
        let tables = vec![
            orders,
            table("customers", &["id", "name"]),
            table("audit_log", &["id", "msg"]),
        ];
        // The question mentions only orders; customers is its neighbour, audit_log is unrelated.
        let ctx = SchemaContextBuilder::new(30).build(&tree(tables), "list orders");
        assert!(
            ctx.text.contains("orders(") && ctx.text.contains("customers("),
            "{}",
            ctx.text
        );
        assert!(!ctx.text.contains("audit_log("), "{}", ctx.text);
    }

    #[test]
    fn an_oversized_table_is_shortened_keeping_key_columns() {
        let cols: Vec<String> = (0..80).map(|i| format!("column_number_{i}")).collect();
        let refs: Vec<&str> = cols.iter().map(String::as_str).collect();
        let mut big = table("wide", &refs);
        big.primary_key = vec!["column_number_79".into()];
        let ctx = SchemaContextBuilder::new(80).build(&tree(vec![big]), "wide");
        assert!(
            ctx.text.contains("column_number_79 text PK"),
            "{}",
            ctx.text
        );
        assert!(ctx.text.contains(", ...)"), "{}", ctx.text);
        assert_eq!(ctx.included_tables, 1);
    }

    #[test]
    fn views_are_marked_and_empty_schema_is_empty() {
        let mut v = table("recent", &["id"]);
        v.kind = TableKind::View;
        assert!(
            SchemaContextBuilder::new(100)
                .build(&tree(vec![v]), "")
                .text
                .starts_with("view public.recent(")
        );
        let empty = SchemaContextBuilder::new(100).build(&SchemaTree::default(), "x");
        assert_eq!((empty.text.as_str(), empty.total_tables), ("", 0));
    }
}
