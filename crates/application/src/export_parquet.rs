//! Parquet export (Apache Parquet via the `parquet` crate, without Arrow).
//!
//! Column types are inferred from the values: all integers -> INT64, numbers with a float ->
//! DOUBLE, booleans -> BOOLEAN, binary -> BYTE_ARRAY, and everything else (text, decimals,
//! dates, JSON, mixed columns) -> UTF-8 strings, so no value is ever lossy. Every column is
//! nullable, so NULLs are real nulls.

use std::io;
use std::sync::Arc;

use domain::{QueryResult, Value};
use parquet::basic::{Compression, ConvertedType, Repetition, Type as PhysicalType};
use parquet::data_type::ByteArray;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Int,
    Double,
    Bool,
    Binary,
    Text,
}

fn column_kind(result: &QueryResult, col: usize) -> Kind {
    let mut kind: Option<Kind> = None;
    for row in &result.rows {
        let next = match row.get(col) {
            None | Some(Value::Null) => continue,
            Some(Value::Int(_)) => Kind::Int,
            Some(Value::Float(_)) => Kind::Double,
            Some(Value::Bool(_)) => Kind::Bool,
            Some(Value::Bytes(_)) => Kind::Binary,
            Some(_) => Kind::Text,
        };
        kind = Some(match (kind, next) {
            (None, k) => k,
            (Some(a), b) if a == b => a,
            (Some(Kind::Int), Kind::Double) | (Some(Kind::Double), Kind::Int) => Kind::Double,
            _ => Kind::Text,
        });
        if kind == Some(Kind::Text) {
            break;
        }
    }
    kind.unwrap_or(Kind::Text)
}

fn text_of(v: &Value) -> String {
    v.display()
}

/// The whole file in memory (exports are capped, so this is bounded).
pub fn render_parquet(result: &QueryResult) -> io::Result<Vec<u8>> {
    let kinds: Vec<Kind> = (0..result.columns.len())
        .map(|c| column_kind(result, c))
        .collect();

    let mut fields = Vec::with_capacity(kinds.len());
    for (column, kind) in result.columns.iter().zip(&kinds) {
        let physical = match kind {
            Kind::Int => PhysicalType::INT64,
            Kind::Double => PhysicalType::DOUBLE,
            Kind::Bool => PhysicalType::BOOLEAN,
            Kind::Binary | Kind::Text => PhysicalType::BYTE_ARRAY,
        };
        let mut builder = Type::primitive_type_builder(&column.name, physical)
            .with_repetition(Repetition::OPTIONAL);
        if *kind == Kind::Text {
            builder = builder.with_converted_type(ConvertedType::UTF8);
        }
        fields.push(Arc::new(builder.build().map_err(other)?));
    }
    let schema = Arc::new(
        Type::group_type_builder("fathom_export")
            .with_fields(fields)
            .build()
            .map_err(other)?,
    );
    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build(),
    );

    let mut buffer = Vec::new();
    {
        let mut writer = SerializedFileWriter::new(&mut buffer, schema, props).map_err(other)?;
        let mut group = writer.next_row_group().map_err(other)?;
        for (index, kind) in kinds.iter().enumerate() {
            let Some(mut column) = group.next_column().map_err(other)? else {
                break;
            };
            // 1 = a value follows, 0 = NULL (the column is OPTIONAL, so one level suffices).
            let mut levels: Vec<i16> = Vec::with_capacity(result.rows.len());
            fn cell(row: &[Value], index: usize) -> Option<&Value> {
                row.get(index).filter(|v| !v.is_null())
            }
            match kind {
                Kind::Int => {
                    let mut values: Vec<i64> = Vec::new();
                    for row in &result.rows {
                        match cell(row, index) {
                            Some(Value::Int(i)) => {
                                values.push(*i);
                                levels.push(1);
                            }
                            _ => levels.push(0),
                        }
                    }
                    column
                        .typed::<parquet::data_type::Int64Type>()
                        .write_batch(&values, Some(&levels), None)
                        .map_err(other)?;
                }
                Kind::Double => {
                    let mut values: Vec<f64> = Vec::new();
                    for row in &result.rows {
                        match cell(row, index) {
                            Some(Value::Float(f)) => {
                                values.push(*f);
                                levels.push(1);
                            }
                            Some(Value::Int(i)) => {
                                values.push(*i as f64);
                                levels.push(1);
                            }
                            _ => levels.push(0),
                        }
                    }
                    column
                        .typed::<parquet::data_type::DoubleType>()
                        .write_batch(&values, Some(&levels), None)
                        .map_err(other)?;
                }
                Kind::Bool => {
                    let mut values: Vec<bool> = Vec::new();
                    for row in &result.rows {
                        match cell(row, index) {
                            Some(Value::Bool(b)) => {
                                values.push(*b);
                                levels.push(1);
                            }
                            _ => levels.push(0),
                        }
                    }
                    column
                        .typed::<parquet::data_type::BoolType>()
                        .write_batch(&values, Some(&levels), None)
                        .map_err(other)?;
                }
                Kind::Binary | Kind::Text => {
                    let mut values: Vec<ByteArray> = Vec::new();
                    for row in &result.rows {
                        match cell(row, index) {
                            Some(Value::Bytes(b)) if *kind == Kind::Binary => {
                                values.push(ByteArray::from(b.clone()));
                                levels.push(1);
                            }
                            Some(v) => {
                                values.push(ByteArray::from(text_of(v).into_bytes()));
                                levels.push(1);
                            }
                            None => levels.push(0),
                        }
                    }
                    column
                        .typed::<parquet::data_type::ByteArrayType>()
                        .write_batch(&values, Some(&levels), None)
                        .map_err(other)?;
                }
            }
            column.close().map_err(other)?;
        }
        group.close().map_err(other)?;
        writer.close().map_err(other)?;
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use domain::{DataType, ResultColumn};
    use parquet::basic::Type as Physical;
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use parquet::record::Field;

    use super::*;

    fn result(columns: &[&str], rows: Vec<Vec<Value>>) -> QueryResult {
        QueryResult {
            columns: columns
                .iter()
                .map(|n| ResultColumn {
                    name: (*n).into(),
                    data_type: DataType::new("t"),
                })
                .collect(),
            rows,
            ..QueryResult::empty()
        }
    }

    fn read(bytes: Vec<u8>) -> (Vec<Physical>, Vec<Vec<Field>>) {
        let reader = SerializedFileReader::new(Bytes::from(bytes)).expect("valid parquet");
        let schema = reader.metadata().file_metadata().schema_descr();
        let types = (0..schema.num_columns())
            .map(|i| schema.column(i).physical_type())
            .collect();
        let rows = reader
            .get_row_iter(None)
            .expect("rows")
            .map(|r| {
                r.expect("row")
                    .get_column_iter()
                    .map(|(_, f)| f.clone())
                    .collect()
            })
            .collect();
        (types, rows)
    }

    #[test]
    fn types_are_inferred_and_values_round_trip() {
        let r = result(
            &["id", "price", "ok", "blob", "name", "mixed"],
            vec![
                vec![
                    Value::Int(1),
                    Value::Float(1.5),
                    Value::Bool(true),
                    Value::Bytes(vec![1, 2]),
                    Value::Text("Olivia".into()),
                    Value::Int(7),
                ],
                vec![
                    Value::Int(2),
                    Value::Int(2),
                    Value::Bool(false),
                    Value::Null,
                    Value::Text("O'Brien".into()),
                    Value::Text("x".into()),
                ],
            ],
        );
        let (types, rows) = read(render_parquet(&r).expect("render"));
        assert_eq!(
            types,
            [
                Physical::INT64,
                Physical::DOUBLE,
                Physical::BOOLEAN,
                Physical::BYTE_ARRAY,
                Physical::BYTE_ARRAY,
                Physical::BYTE_ARRAY
            ]
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Field::Long(1));
        assert_eq!(rows[0][1], Field::Double(1.5));
        assert_eq!(
            rows[1][1],
            Field::Double(2.0),
            "an int in a float column is widened"
        );
        assert_eq!(rows[0][2], Field::Bool(true));
        assert_eq!(rows[1][3], Field::Null, "NULL is a real null");
        assert_eq!(rows[1][4], Field::Str("O'Brien".into()));
        assert_eq!(
            rows[0][5],
            Field::Str("7".into()),
            "a mixed column falls back to text"
        );
    }

    #[test]
    fn exact_values_that_are_not_numbers_stay_text_and_lossless() {
        let r = result(
            &["big", "when", "doc"],
            vec![vec![
                Value::Decimal("18446744073709551615".into()),
                Value::Temporal("2026-11-27 09:42:18".into()),
                Value::Json("{\"a\":1}".into()),
            ]],
        );
        let (types, rows) = read(render_parquet(&r).expect("render"));
        assert!(types.iter().all(|t| *t == Physical::BYTE_ARRAY));
        assert_eq!(rows[0][0], Field::Str("18446744073709551615".into()));
        assert_eq!(rows[0][1], Field::Str("2026-11-27 09:42:18".into()));
        assert_eq!(rows[0][2], Field::Str("{\"a\":1}".into()));
    }

    #[test]
    fn all_null_columns_and_empty_results_are_valid_files() {
        let r = result(&["a"], vec![vec![Value::Null], vec![Value::Null]]);
        let (types, rows) = read(render_parquet(&r).expect("render"));
        assert_eq!(types, [Physical::BYTE_ARRAY]);
        assert_eq!(rows, vec![vec![Field::Null], vec![Field::Null]]);

        let empty = result(&["a", "b"], vec![]);
        let (_, rows) = read(render_parquet(&empty).expect("render"));
        assert!(rows.is_empty());
    }

    #[test]
    fn column_names_with_spaces_and_quotes_are_fine() {
        let r = result(
            &["first name", "o\"dd"],
            vec![vec![Value::Int(1), Value::Int(2)]],
        );
        let bytes = render_parquet(&r).expect("render");
        let reader = SerializedFileReader::new(Bytes::from(bytes)).expect("valid");
        let schema = reader.metadata().file_metadata().schema_descr();
        assert_eq!(schema.column(0).name(), "first name");
        assert_eq!(schema.column(1).name(), "o\"dd");
    }

    #[test]
    fn export_format_writes_parquet_through_the_common_path() {
        let r = result(&["id"], vec![vec![Value::Int(1)]]);
        let bytes = crate::export::render(
            &r,
            crate::export::ExportFormat::Parquet,
            &Default::default(),
        )
        .expect("render");
        assert_eq!(&bytes[..4], b"PAR1", "Parquet magic number");
        assert_eq!(crate::export::ExportFormat::Parquet.extension(), "parquet");
    }
}
