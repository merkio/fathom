//! Maps PostgreSQL result cells to domain `Value`s.

use domain::Value;
use sqlx::postgres::PgRow;
use sqlx::postgres::{PgValueFormat, PgValueRef};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::types::{JsonValue, Uuid};
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// Lowercase PostgreSQL type name of a column, for display in result headers.
pub(crate) fn type_name(row: &PgRow, idx: usize) -> String {
    row.columns()
        .get(idx)
        .map(|c| c.type_info().name().to_lowercase())
        .unwrap_or_default()
}

pub(crate) fn map_row(row: &PgRow) -> Vec<Value> {
    (0..row.len()).map(|i| map_cell(row, i)).collect()
}

fn map_cell(row: &PgRow, idx: usize) -> Value {
    match row.try_get_raw(idx) {
        Ok(raw) if raw.is_null() => return Value::Null,
        Err(_) => return Value::Null,
        Ok(_) => {}
    }
    let name = type_name(row, idx).to_uppercase();
    let mapped = match name.as_str() {
        "BOOL" => row.try_get::<bool, _>(idx).map(Value::Bool).ok(),
        "INT2" => row
            .try_get::<i16, _>(idx)
            .map(|v| Value::Int(v.into()))
            .ok(),
        "INT4" => row
            .try_get::<i32, _>(idx)
            .map(|v| Value::Int(v.into()))
            .ok(),
        "INT8" => row.try_get::<i64, _>(idx).map(Value::Int).ok(),
        "FLOAT4" => row
            .try_get::<f32, _>(idx)
            .map(|v| Value::Float(v.into()))
            .ok(),
        "FLOAT8" => row.try_get::<f64, _>(idx).map(Value::Float).ok(),
        "NUMERIC" => row
            .try_get_raw(idx)
            .ok()
            .and_then(numeric_text)
            .map(Value::Decimal),
        "BYTEA" => row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes).ok(),
        "DATE" => row
            .try_get::<NaiveDate, _>(idx)
            .map(|v| Value::Temporal(v.to_string()))
            .ok(),
        "TIME" => row
            .try_get::<NaiveTime, _>(idx)
            .map(|v| Value::Temporal(v.to_string()))
            .ok(),
        "TIMESTAMP" => row
            .try_get::<NaiveDateTime, _>(idx)
            .map(|v| Value::Temporal(v.to_string()))
            .ok(),
        "TIMESTAMPTZ" => row
            .try_get::<DateTime<Utc>, _>(idx)
            .map(|v| Value::Temporal(v.to_rfc3339()))
            .ok(),
        "UUID" => row
            .try_get::<Uuid, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .ok(),
        "JSON" | "JSONB" => row
            .try_get::<JsonValue, _>(idx)
            .map(|v| Value::Json(v.to_string()))
            .ok(),
        _ => row.try_get::<String, _>(idx).map(Value::Text).ok(),
    };
    // Types without a mapping (intervals, arrays, geometric, ...) show their type, not garbage.
    mapped.unwrap_or_else(|| Value::Text(format!("<{}>", name.to_lowercase())))
}

/// Exact text of a `numeric`, honouring its declared scale (`sqlx`'s `BigDecimal` decoding
/// drops it, turning `1.50` into `1.5000`).
fn numeric_text(raw: PgValueRef<'_>) -> Option<String> {
    match raw.format() {
        PgValueFormat::Text => raw.as_str().ok().map(str::to_owned),
        PgValueFormat::Binary => decode_numeric(raw.as_bytes().ok()?),
    }
}

/// Decodes PostgreSQL's binary `numeric`: header `ndigits, weight, sign, dscale` (i16/u16,
/// big-endian) followed by base-10000 digit groups.
fn decode_numeric(bytes: &[u8]) -> Option<String> {
    let word = |i: usize| {
        Some(i16::from_be_bytes([
            *bytes.get(2 * i)?,
            *bytes.get(2 * i + 1)?,
        ]))
    };
    let ndigits = usize::try_from(word(0)?).ok()?;
    let weight = i32::from(word(1)?);
    let sign = word(2)? as u16;
    let dscale = usize::try_from(word(3)?).ok()?;
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let group = |exponent: i32| -> Option<u16> {
        let k = usize::try_from(weight - exponent).ok()?;
        if k >= ndigits {
            return Some(0);
        }
        let at = 8 + 2 * k;
        Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]))
    };

    let mut int_part = String::new();
    for exponent in (0..=weight).rev() {
        int_part.push_str(&format!("{:04}", group(exponent)?));
    }
    let int_part = int_part.trim_start_matches('0');
    let mut out = if int_part.is_empty() {
        "0".to_owned()
    } else {
        int_part.to_owned()
    };

    if dscale > 0 {
        let mut frac = String::new();
        let mut exponent = -1;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", group(exponent)?));
            exponent -= 1;
        }
        frac.truncate(dscale);
        out.push('.');
        out.push_str(&frac);
    }
    let is_zero = out.bytes().all(|b| b == b'0' || b == b'.');
    if sign == 0x4000 && !is_zero {
        out.insert(0, '-');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::decode_numeric;

    fn encode(weight: i16, sign: u16, dscale: u16, digits: &[u16]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend((digits.len() as i16).to_be_bytes());
        b.extend(weight.to_be_bytes());
        b.extend(sign.to_be_bytes());
        b.extend(dscale.to_be_bytes());
        for d in digits {
            b.extend(d.to_be_bytes());
        }
        b
    }

    #[test]
    fn keeps_declared_scale() {
        assert_eq!(
            decode_numeric(&encode(0, 0, 2, &[1, 5000])).as_deref(),
            Some("1.50")
        );
        assert_eq!(
            decode_numeric(&encode(0, 0, 4, &[0])).as_deref(),
            Some("0.0000")
        );
    }

    #[test]
    fn large_and_small_magnitudes() {
        // 12345678901234567890.123456789
        let d = encode(4, 0, 9, &[1234, 5678, 9012, 3456, 7890, 1234, 5678, 9000]);
        assert_eq!(
            decode_numeric(&d).as_deref(),
            Some("12345678901234567890.123456789")
        );
        // 0.0001 (weight -1, one group)
        assert_eq!(
            decode_numeric(&encode(-1, 0, 4, &[1])).as_deref(),
            Some("0.0001")
        );
        // 10000 (weight 1, trailing zero group omitted)
        assert_eq!(
            decode_numeric(&encode(1, 0, 0, &[1])).as_deref(),
            Some("10000")
        );
        // zero has no digits
        assert_eq!(decode_numeric(&encode(0, 0, 0, &[])).as_deref(), Some("0"));
    }

    #[test]
    fn signs_and_specials() {
        assert_eq!(
            decode_numeric(&encode(0, 0x4000, 1, &[12, 3000])).as_deref(),
            Some("-12.3")
        );
        assert_eq!(
            decode_numeric(&encode(0, 0xC000, 0, &[])).as_deref(),
            Some("NaN")
        );
        assert_eq!(
            decode_numeric(&encode(0, 0xD000, 0, &[])).as_deref(),
            Some("Infinity")
        );
        assert_eq!(
            decode_numeric(&encode(0, 0xF000, 0, &[])).as_deref(),
            Some("-Infinity")
        );
    }

    #[test]
    fn truncated_input_is_rejected() {
        assert_eq!(decode_numeric(&[0, 1]), None);
    }
}
