//! Query requests, typed cell values and results.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::schema::DataType;

/// Language of a query. SQL is the only one handled today; others are placeholders for later adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryLanguage {
    Sql,
    Mql,
    Cql,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub text: String,
    pub language: QueryLanguage,
    /// Maximum number of rows to return; the result is marked `truncated` beyond it.
    pub row_limit: Option<u32>,
    pub timeout: Option<Duration>,
}

impl QueryRequest {
    pub fn sql(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            language: QueryLanguage::Sql,
            row_limit: None,
            timeout: None,
        }
    }
}

/// A typed result cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact numeric kept as text to avoid precision loss.
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    /// Date, time or timestamp in ISO-8601 text form.
    Temporal(String),
    /// JSON document as text.
    Json(String),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Human-readable rendering for grids and exports.
    pub fn display(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Decimal(s) | Value::Text(s) | Value::Temporal(s) | Value::Json(s) => s.clone(),
            Value::Bytes(b) => format!("<{} bytes>", b.len()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultColumn {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    pub rows_affected: Option<u64>,
    pub elapsed: Duration,
    pub truncated: bool,
}

impl QueryResult {
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: None,
            elapsed: Duration::ZERO,
            truncated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_request_has_no_limits_by_default() {
        let r = QueryRequest::sql("select 1");
        assert_eq!(
            (r.language, r.row_limit, r.timeout),
            (QueryLanguage::Sql, None, None)
        );
    }

    #[test]
    fn value_display() {
        assert_eq!(Value::Null.display(), "NULL");
        assert_eq!(Value::Int(7).display(), "7");
        assert_eq!(Value::Bytes(vec![1, 2, 3]).display(), "<3 bytes>");
        assert!(Value::Null.is_null() && !Value::Bool(false).is_null());
    }
}
