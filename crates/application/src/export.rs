//! Result export: CSV and JSON writers. Pure and generic over `io::Write`, so they stream to a
//! file (or any sink) without building the whole document in memory.

use std::collections::HashSet;
use std::io::{self, Write};

use domain::{Dialect, QueryResult, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
    /// `INSERT` statements, one per row (for loading the data elsewhere; never run by Fathom).
    Sql,
    /// Apache Parquet (binary; column types are inferred from the values).
    Parquet,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
            ExportFormat::Parquet => "parquet",
        }
    }
}

/// What the SQL format needs to name the target table and quote identifiers.
#[derive(Debug, Clone, Copy)]
pub struct SqlOptions<'a> {
    pub table: &'a str,
    pub dialect: &'a Dialect,
}

impl Default for SqlOptions<'_> {
    fn default() -> Self {
        Self {
            table: "exported_data",
            dialect: &Dialect::ANSI,
        }
    }
}

pub fn write(result: &QueryResult, format: ExportFormat, out: &mut dyn Write) -> io::Result<()> {
    write_with(result, format, &SqlOptions::default(), out)
}

pub fn write_with(
    result: &QueryResult,
    format: ExportFormat,
    sql: &SqlOptions<'_>,
    out: &mut dyn Write,
) -> io::Result<()> {
    match format {
        ExportFormat::Csv => write_csv(result, out),
        ExportFormat::Json => write_json(result, out),
        ExportFormat::Sql => write_sql(result, sql, out),
        ExportFormat::Parquet => out.write_all(&crate::export_parquet::render_parquet(result)?),
    }
}

/// The whole document in memory (exports are capped, so this is bounded).
pub fn render(
    result: &QueryResult,
    format: ExportFormat,
    sql: &SqlOptions<'_>,
) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    write_with(result, format, sql, &mut buffer)?;
    Ok(buffer)
}

fn sql_literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Float(_) => "NULL".into(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Decimal(d) if d.parse::<f64>().is_ok() && !d.contains(['e', 'E', ' ']) => d.clone(),
        Value::Decimal(t) | Value::Text(t) | Value::Temporal(t) | Value::Json(t) => {
            format!("'{}'", t.replace('\'', "''"))
        }
        Value::Bytes(b) => {
            let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
            format!("X'{hex}'")
        }
    }
}

/// One `INSERT INTO table (columns) VALUES (...);` per row. Quotes are doubled and nothing else
/// is interpreted, so the text is safe to read; it is a file, never executed here.
pub fn write_sql(
    result: &QueryResult,
    options: &SqlOptions<'_>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let d = options.dialect;
    let columns = result
        .columns
        .iter()
        .map(|c| d.quote_identifier(&c.name))
        .collect::<Vec<_>>()
        .join(", ");
    let table = d.quote_identifier(options.table);
    for row in &result.rows {
        let values = row.iter().map(sql_literal).collect::<Vec<_>>().join(", ");
        writeln!(out, "INSERT INTO {table} ({columns}) VALUES ({values});")?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// RFC 4180 CSV: a header row, `\r\n` line ends, fields quoted when needed. `NULL` is an empty
/// field and binary values are hex (`0x...`).
pub fn write_csv(result: &QueryResult, out: &mut dyn Write) -> io::Result<()> {
    let header: Vec<String> = result.columns.iter().map(|c| csv_field(&c.name)).collect();
    write!(out, "{}\r\n", header.join(","))?;
    for row in &result.rows {
        let fields: Vec<String> = row
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                Value::Bytes(b) => hex(b),
                other => csv_field(&other.display()),
            })
            .collect();
        write!(out, "{}\r\n", fields.join(","))?;
    }
    Ok(())
}

fn json_string(text: &str) -> String {
    let mut s = String::with_capacity(text.len() + 2);
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

fn json_value(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        // JSON has no NaN or infinity.
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Float(_) => "null".to_owned(),
        Value::Bytes(b) => json_string(&hex(b)),
        // Exact numerics stay strings so no precision is lost; temporal and JSON values are
        // exported as their text.
        Value::Decimal(s) | Value::Text(s) | Value::Temporal(s) | Value::Json(s) => json_string(s),
    }
}

/// Column names made unique (`id`, `id_2`, ...) so objects never silently lose a column.
fn unique_names(result: &QueryResult) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    result
        .columns
        .iter()
        .map(|c| {
            let mut name = c.name.clone();
            let mut n = 2;
            while !seen.insert(name.clone()) {
                name = format!("{}_{n}", c.name);
                n += 1;
            }
            name
        })
        .collect()
}

/// A JSON array of objects, one per row, one line per row.
pub fn write_json(result: &QueryResult, out: &mut dyn Write) -> io::Result<()> {
    let names: Vec<String> = unique_names(result)
        .iter()
        .map(|n| json_string(n))
        .collect();
    out.write_all(b"[")?;
    for (i, row) in result.rows.iter().enumerate() {
        out.write_all(if i == 0 { b"\n" } else { b",\n" })?;
        let fields: Vec<String> = names
            .iter()
            .zip(row)
            .map(|(n, v)| format!("{n}: {}", json_value(v)))
            .collect();
        write!(out, "  {{{}}}", fields.join(", "))?;
    }
    out.write_all(if result.rows.is_empty() {
        b"]\n"
    } else {
        b"\n]\n"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{DataType, ResultColumn};

    fn result(cols: &[&str], rows: Vec<Vec<Value>>) -> QueryResult {
        QueryResult {
            columns: cols
                .iter()
                .map(|c| ResultColumn {
                    name: (*c).into(),
                    data_type: DataType::new("t"),
                })
                .collect(),
            rows,
            ..QueryResult::empty()
        }
    }

    fn csv(r: &QueryResult) -> String {
        let mut out = Vec::new();
        write_csv(r, &mut out).expect("write");
        String::from_utf8(out).expect("utf8")
    }

    fn json(r: &QueryResult) -> String {
        let mut out = Vec::new();
        write_json(r, &mut out).expect("write");
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn csv_has_a_header_and_crlf_line_ends() {
        let r = result(
            &["id", "name"],
            vec![vec![Value::Int(1), Value::Text("Ada".into())]],
        );
        assert_eq!(csv(&r), "id,name\r\n1,Ada\r\n");
    }

    #[test]
    fn csv_quotes_commas_quotes_and_newlines_and_empties_nulls() {
        let r = result(
            &["a,b", "q"],
            vec![
                vec![Value::Text("x,y".into()), Value::Text("say \"hi\"".into())],
                vec![Value::Text("line1\nline2".into()), Value::Null],
            ],
        );
        assert_eq!(
            csv(&r),
            "\"a,b\",q\r\n\"x,y\",\"say \"\"hi\"\"\"\r\n\"line1\nline2\",\r\n"
        );
    }

    #[test]
    fn csv_renders_every_value_kind() {
        let r = result(
            &["v"],
            vec![
                vec![Value::Bool(true)],
                vec![Value::Float(1.5)],
                vec![Value::Decimal("12.50".into())],
                vec![Value::Temporal("2024-02-29".into())],
                vec![Value::Bytes(vec![0xde, 0xad])],
                vec![Value::Json("{\"a\":1}".into())],
            ],
        );
        assert_eq!(
            csv(&r),
            "v\r\ntrue\r\n1.5\r\n12.50\r\n2024-02-29\r\n0xdead\r\n\"{\"\"a\"\":1}\"\r\n"
        );
    }

    #[test]
    fn csv_of_an_empty_result_is_just_the_header() {
        assert_eq!(csv(&result(&["a", "b"], vec![])), "a,b\r\n");
    }

    #[test]
    fn json_is_an_array_of_objects_with_typed_values() {
        let r = result(
            &["id", "name", "ok", "score", "price", "when", "raw", "gone"],
            vec![vec![
                Value::Int(1),
                Value::Text("Ada".into()),
                Value::Bool(true),
                Value::Float(2.5),
                Value::Decimal("12.50".into()),
                Value::Temporal("2024-02-29".into()),
                Value::Bytes(vec![1, 2]),
                Value::Null,
            ]],
        );
        assert_eq!(
            json(&r),
            "[\n  {\"id\": 1, \"name\": \"Ada\", \"ok\": true, \"score\": 2.5, \"price\": \"12.50\", \"when\": \"2024-02-29\", \"raw\": \"0x0102\", \"gone\": null}\n]\n"
        );
    }

    #[test]
    fn json_escapes_text_and_survives_hostile_values() {
        let r = result(
            &["t"],
            vec![
                vec![Value::Text("a\"b\\c\nd\te\u{1}é✓".into())],
                vec![Value::Float(f64::NAN)],
                vec![Value::Float(f64::INFINITY)],
            ],
        );
        assert_eq!(
            json(&r),
            "[\n  {\"t\": \"a\\\"b\\\\c\\nd\\te\\u0001é✓\"},\n  {\"t\": null},\n  {\"t\": null}\n]\n"
        );
    }

    #[test]
    fn json_keeps_duplicate_column_names_apart() {
        let r = result(
            &["id", "id", "id_2"],
            vec![vec![Value::Int(1), Value::Int(2), Value::Int(3)]],
        );
        let text = json(&r);
        assert!(
            text.contains("\"id\": 1, \"id_2\": 2, \"id_2_2\": 3"),
            "{text}"
        );
    }

    #[test]
    fn json_of_an_empty_result_is_an_empty_array() {
        assert_eq!(json(&result(&["a"], vec![])), "[]\n");
    }

    #[test]
    fn write_dispatches_on_the_format_and_reports_io_errors() {
        let r = result(&["a"], vec![vec![Value::Int(1)]]);
        let mut out = Vec::new();
        write(&r, ExportFormat::Csv, &mut out).expect("csv");
        assert_eq!(out, b"a\r\n1\r\n");

        struct Failing;
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(write(&r, ExportFormat::Json, &mut Failing).is_err());
        assert_eq!(
            (
                ExportFormat::Csv.extension(),
                ExportFormat::Json.extension()
            ),
            ("csv", "json")
        );
    }

    #[test]
    fn sql_export_writes_one_insert_per_row_with_escaped_literals() {
        use domain::{DataType, ResultColumn};
        let col = |n: &str| ResultColumn {
            name: n.into(),
            data_type: DataType::new("t"),
        };
        let result = QueryResult {
            columns: vec![
                col("id"),
                col("na\"me"),
                col("note"),
                col("ok"),
                col("blob"),
                col("n"),
            ],
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Text("O'Brien; DROP TABLE x; --".into()),
                    Value::Null,
                    Value::Bool(true),
                    Value::Bytes(vec![0xde, 0xad]),
                    Value::Decimal("12.50".into()),
                ],
                vec![
                    Value::Int(2),
                    Value::Text("b".into()),
                    Value::Float(f64::NAN),
                    Value::Bool(false),
                    Value::Bytes(vec![]),
                    Value::Decimal("1e5; evil".into()),
                ],
            ],
            ..QueryResult::empty()
        };
        let text =
            String::from_utf8(render(&result, ExportFormat::Sql, &SqlOptions::default()).unwrap())
                .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            r#"INSERT INTO "exported_data" ("id", "na""me", "note", "ok", "blob", "n") VALUES (1, 'O''Brien; DROP TABLE x; --', NULL, TRUE, X'dead', 12.50);"#
        );
        assert!(
            lines[1].contains("NULL, FALSE, X'', '1e5; evil'"),
            "{}",
            lines[1]
        );
    }

    #[test]
    fn sql_export_uses_the_given_table_and_dialect_quoting() {
        use domain::{DataType, ResultColumn};
        let result = QueryResult {
            columns: vec![ResultColumn {
                name: "id".into(),
                data_type: DataType::new("t"),
            }],
            rows: vec![vec![Value::Int(7)]],
            ..QueryResult::empty()
        };
        let mysql_like = Dialect {
            identifier_quote: '`',
            ..Dialect::ANSI
        };
        let text = String::from_utf8(
            render(
                &result,
                ExportFormat::Sql,
                &SqlOptions {
                    table: "customers",
                    dialect: &mysql_like,
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(text, "INSERT INTO `customers` (`id`) VALUES (7);\n");
    }
}
