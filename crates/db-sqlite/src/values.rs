use domain::Value;
use rusqlite::types::ValueRef;

pub(crate) fn map_value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::Bytes(b.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_storage_class() {
        assert_eq!(map_value(ValueRef::Null), Value::Null);
        assert_eq!(map_value(ValueRef::Integer(7)), Value::Int(7));
        assert_eq!(map_value(ValueRef::Real(1.5)), Value::Float(1.5));
        assert_eq!(
            map_value(ValueRef::Text("héllo".as_bytes())),
            Value::Text("héllo".into())
        );
        assert_eq!(map_value(ValueRef::Blob(&[1, 2])), Value::Bytes(vec![1, 2]));
    }

    #[test]
    fn invalid_utf8_text_is_replaced_not_dropped() {
        assert_eq!(
            map_value(ValueRef::Text(&[0x66, 0xff, 0x6f])),
            Value::Text("f\u{fffd}o".into())
        );
    }
}
