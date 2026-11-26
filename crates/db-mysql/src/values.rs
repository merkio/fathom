//! Maps MySQL / MariaDB result cells to domain `Value`s.

use domain::Value;
use sqlx::mysql::{MySqlRow, types::MySqlTime};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use sqlx::types::{BigDecimal, JsonValue};
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// Lowercase type name of a column, for display in result headers.
pub(crate) fn type_name(row: &MySqlRow, idx: usize) -> String {
    row.columns().get(idx).map(|c| c.type_info().name().to_lowercase()).unwrap_or_default()
}

pub(crate) fn map_row(row: &MySqlRow) -> Vec<Value> {
    (0..row.len()).map(|i| map_cell(row, i)).collect()
}

/// An unsigned 64-bit value: `Int` when it fits, exact text otherwise.
fn unsigned(v: u64) -> Value {
    i64::try_from(v).map_or_else(|_| Value::Decimal(v.to_string()), Value::Int)
}

fn map_cell(row: &MySqlRow, idx: usize) -> Value {
    match row.try_get_raw(idx) {
        Ok(raw) if raw.is_null() => return Value::Null,
        Err(_) => return Value::Null,
        Ok(_) => {}
    }
    let name = type_name(row, idx).to_uppercase();
    let mapped = match name.as_str() {
        "BOOLEAN" => row.try_get::<bool, _>(idx).map(Value::Bool).ok(),
        "TINYINT" => row.try_get::<i8, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "SMALLINT" => row.try_get::<i16, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "INT" | "MEDIUMINT" => row.try_get::<i32, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "BIGINT" => row.try_get::<i64, _>(idx).map(Value::Int).ok(),
        "TINYINT UNSIGNED" => row.try_get::<u8, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "SMALLINT UNSIGNED" => row.try_get::<u16, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "INT UNSIGNED" | "MEDIUMINT UNSIGNED" => row.try_get::<u32, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "BIGINT UNSIGNED" => row.try_get::<u64, _>(idx).map(unsigned).ok(),
        "YEAR" => row.try_get::<u16, _>(idx).map(|v| Value::Int(v.into())).ok(),
        "FLOAT" => row.try_get::<f32, _>(idx).map(|v| Value::Float(v.into())).ok(),
        "DOUBLE" => row.try_get::<f64, _>(idx).map(Value::Float).ok(),
        // MySQL sends DECIMAL as text, so the declared scale is kept (1.50 stays 1.50).
        "DECIMAL" => row.try_get::<BigDecimal, _>(idx).map(|v| Value::Decimal(v.to_string())).ok(),
        "DATE" => row.try_get::<NaiveDate, _>(idx).map(|v| Value::Temporal(v.to_string())).ok(),
        // TIME can be negative or beyond 24 hours (838:59:59), which only `MySqlTime` represents.
        "TIME" => row.try_get::<MySqlTime, _>(idx).map(|v| Value::Temporal(v.to_string())).ok(),
        "DATETIME" => row.try_get::<NaiveDateTime, _>(idx).map(|v| Value::Temporal(v.to_string())).ok(),
        "TIMESTAMP" => row
            .try_get::<DateTime<Utc>, _>(idx)
            .map(|v| Value::Temporal(v.to_rfc3339()))
            .ok(),
        "JSON" => row.try_get::<JsonValue, _>(idx).map(|v| Value::Json(v.to_string())).ok(),
        "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BIT" => {
            row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes).ok()
        }
        _ => row.try_get::<String, _>(idx).map(Value::Text).ok(),
    };
    // Types without a mapping (geometry, vectors, ...) show their type, not garbage.
    mapped.unwrap_or_else(|| Value::Text(format!("<{}>", name.to_lowercase())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_bigints_beyond_i64_keep_every_digit() {
        assert_eq!(unsigned(5), Value::Int(5));
        assert_eq!(unsigned(i64::MAX as u64), Value::Int(i64::MAX));
        assert_eq!(unsigned(u64::MAX), Value::Decimal("18446744073709551615".into()));
    }
}
