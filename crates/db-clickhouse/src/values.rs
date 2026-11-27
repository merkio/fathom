//! Maps ClickHouse JSON values (the `JSONCompact` output format) to domain values, driven by the
//! column type the server reports.
//!
//! 64-bit integers arrive as JSON strings (the server quotes them by default) and wide integers
//! and decimals always do; whatever does not fit `i64` is kept as exact decimal text.

use domain::Value;
use serde_json::Value as Json;

/// The type with `Nullable(..)` and `LowCardinality(..)` wrappers removed, plus whether the
/// column can hold NULL.
pub(crate) fn unwrap_type(mut ty: &str) -> (&str, bool) {
    let mut nullable = false;
    loop {
        let ty_trimmed = ty.trim();
        if let Some(inner) = wrapped(ty_trimmed, "Nullable") {
            nullable = true;
            ty = inner;
        } else if let Some(inner) = wrapped(ty_trimmed, "LowCardinality") {
            ty = inner;
        } else {
            return (ty_trimmed, nullable);
        }
    }
}

fn wrapped<'a>(ty: &'a str, name: &str) -> Option<&'a str> {
    ty.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')
}

pub(crate) fn map_value(ty: &str, v: &Json) -> Value {
    let (base_ty, _) = unwrap_type(ty);
    if v.is_null() {
        return Value::Null;
    }
    // `Decimal(10, 2)` -> `Decimal`, `DateTime('UTC')` -> `DateTime`.
    let base = base_ty.split('(').next().unwrap_or(base_ty);
    match base {
        "Int8" | "Int16" | "Int32" | "Int64" | "UInt8" | "UInt16" | "UInt32" | "UInt64" => {
            integer(v)
        }
        "Int128" | "Int256" | "UInt128" | "UInt256" | "Decimal" | "Decimal32" | "Decimal64"
        | "Decimal128" | "Decimal256" => Value::Decimal(text(v)),
        "Float32" | "Float64" | "BFloat16" => match v {
            Json::Number(n) => n.as_f64().map_or(Value::Null, Value::Float),
            Json::String(s) => s
                .parse::<f64>()
                .map_or_else(|_| Value::Text(s.clone()), Value::Float),
            other => Value::Json(other.to_string()),
        },
        "Bool" => v
            .as_bool()
            .map_or_else(|| Value::Text(text(v)), Value::Bool),
        "Date" | "Date32" | "DateTime" | "DateTime64" | "Time" | "Time64" => {
            Value::Temporal(text(v))
        }
        "Array" | "Tuple" | "Map" | "Nested" | "Object" | "JSON" | "Variant" | "Dynamic" => {
            Value::Json(v.to_string())
        }
        "Nothing" => Value::Null,
        // String, FixedString, UUID, IPv4, IPv6, Enum8/16 and anything unknown.
        _ => match v {
            Json::String(s) => Value::Text(s.clone()),
            Json::Number(_) | Json::Bool(_) => Value::Text(v.to_string()),
            other => Value::Json(other.to_string()),
        },
    }
}

fn integer(v: &Json) -> Value {
    match v {
        Json::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Decimal(n.to_string()), Value::Int),
        Json::String(s) => s
            .parse::<i64>()
            .map_or_else(|_| Value::Decimal(s.clone()), Value::Int),
        other => Value::Json(other.to_string()),
    }
}

fn text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn small_integers_are_ints_whether_quoted_or_not() {
        assert_eq!(map_value("Int32", &json!(7)), Value::Int(7));
        assert_eq!(map_value("UInt8", &json!(255)), Value::Int(255));
        assert_eq!(
            map_value("Int64", &json!("-9223372036854775808")),
            Value::Int(i64::MIN)
        );
    }

    #[test]
    fn unsigned_64_bit_beyond_i64_keeps_every_digit() {
        assert_eq!(
            map_value("UInt64", &json!("18446744073709551615")),
            Value::Decimal("18446744073709551615".into())
        );
        assert_eq!(
            map_value("UInt64", &json!(18_446_744_073_709_551_615_u64)),
            Value::Decimal("18446744073709551615".into())
        );
    }

    #[test]
    fn wide_integers_and_decimals_are_exact_text() {
        assert_eq!(
            map_value(
                "UInt256",
                &json!(
                    "115792089237316195423570985008687907853269984665640564039457584007913129639935"
                )
            ),
            Value::Decimal(
                "115792089237316195423570985008687907853269984665640564039457584007913129639935"
                    .into()
            )
        );
        assert_eq!(
            map_value("Decimal(10, 2)", &json!("12345678.90")),
            Value::Decimal("12345678.90".into())
        );
    }

    #[test]
    fn floats_bools_and_text() {
        assert_eq!(map_value("Float64", &json!(1.5)), Value::Float(1.5));
        assert_eq!(map_value("Bool", &json!(true)), Value::Bool(true));
        assert_eq!(map_value("String", &json!("x")), Value::Text("x".into()));
        assert_eq!(
            map_value("UUID", &json!("61f0c404-5cb3-11e7-907b-a6006ad3dba0")),
            Value::Text("61f0c404-5cb3-11e7-907b-a6006ad3dba0".into())
        );
        assert_eq!(
            map_value("Enum8('a' = 1)", &json!("a")),
            Value::Text("a".into())
        );
    }

    #[test]
    fn dates_and_times_are_temporal() {
        assert_eq!(
            map_value("Date", &json!("2026-11-27")),
            Value::Temporal("2026-11-27".into())
        );
        assert_eq!(
            map_value("DateTime('UTC')", &json!("2026-11-27 09:42:18")),
            Value::Temporal("2026-11-27 09:42:18".into())
        );
        assert_eq!(
            map_value("DateTime64(3)", &json!("2026-11-27 09:42:18.123")),
            Value::Temporal("2026-11-27 09:42:18.123".into())
        );
    }

    #[test]
    fn composite_types_become_json_text() {
        assert_eq!(
            map_value("Array(Int32)", &json!([1, 2, 3])),
            Value::Json("[1,2,3]".into())
        );
        assert_eq!(
            map_value("Map(String, Int32)", &json!({"a": 1})),
            Value::Json("{\"a\":1}".into())
        );
        assert_eq!(
            map_value("Tuple(Int32, String)", &json!([1, "a"])),
            Value::Json("[1,\"a\"]".into())
        );
    }

    #[test]
    fn nullable_and_low_cardinality_wrappers_are_transparent() {
        assert_eq!(map_value("Nullable(Int32)", &Json::Null), Value::Null);
        assert_eq!(map_value("Nullable(Int32)", &json!(3)), Value::Int(3));
        assert_eq!(
            map_value("LowCardinality(Nullable(String))", &json!("a")),
            Value::Text("a".into())
        );
        assert_eq!(map_value("Nothing", &Json::Null), Value::Null);
    }

    #[test]
    fn unwrap_type_reports_nullability() {
        assert_eq!(unwrap_type("Int32"), ("Int32", false));
        assert_eq!(unwrap_type("Nullable(Int32)"), ("Int32", true));
        assert_eq!(
            unwrap_type("LowCardinality(Nullable(String))"),
            ("String", true)
        );
        assert_eq!(unwrap_type("LowCardinality(String)"), ("String", false));
        // Not a wrapper: a type whose name merely starts with the same letters.
        assert_eq!(
            unwrap_type("Array(Nullable(Int32))"),
            ("Array(Nullable(Int32))", false)
        );
    }

    #[test]
    fn non_finite_floats_arrive_as_null() {
        assert_eq!(map_value("Float64", &Json::Null), Value::Null);
    }
}
