//! ClickHouse `JSONCompact` decoding.
//!
//! Values arrive as JSON alongside a `meta` array naming each column's type, so
//! decoding is driven by that type name rather than by the JSON shape alone.
//!
//! The server is asked to quote 64-bit integers, so `Int64` and `UInt64` arrive
//! as JSON strings. That is deliberate: JSON numbers are IEEE doubles, and
//! anything past 2^53 would lose precision in transit. `Decimal` is likewise
//! always a string.

use serde::Deserialize;
use tablex_core::Value;

#[derive(Debug, Deserialize)]
pub struct JsonCompact {
    pub meta: Vec<Meta>,
    #[serde(default)]
    pub data: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

/// The `X-ClickHouse-Summary` header, which reports what a statement did.
#[derive(Debug, Default, Deserialize)]
pub struct Summary {
    #[serde(default, deserialize_with = "string_or_number")]
    pub written_rows: u64,
}

impl Summary {
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Summary {
        headers
            .get("X-ClickHouse-Summary")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }
}

/// The summary reports counts as JSON strings, but tolerate numbers too.
fn string_or_number<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Ok(s.parse().unwrap_or(0)),
        serde_json::Value::Number(n) => Ok(n.as_u64().unwrap_or(0)),
        _ => Ok(0),
    }
}

/// Strip `Nullable(...)` and `LowCardinality(...)`, which wrap a real type
/// without changing how its value is represented.
pub fn unwrap_type(ty: &str) -> &str {
    for wrapper in ["Nullable(", "LowCardinality("] {
        if let Some(rest) = ty.strip_prefix(wrapper) {
            if let Some(inner) = rest.strip_suffix(')') {
                return unwrap_type(inner);
            }
        }
    }
    ty
}

/// Decode one cell against its declared ClickHouse type.
pub fn decode(cell: &serde_json::Value, declared: &str) -> Value {
    if cell.is_null() {
        return Value::Null;
    }
    let ty = unwrap_type(declared);

    // Arrays, tuples, maps and nested types are structural; decode elementwise
    // where the JSON is an array so the grid can still show them.
    if let serde_json::Value::Array(items) = cell {
        let inner = ty
            .strip_prefix("Array(")
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or("");
        return Value::Array(items.iter().map(|i| decode(i, inner)).collect());
    }

    match cell {
        serde_json::Value::Bool(b) => Value::Bool(*b),

        serde_json::Value::Number(n) => {
            if ty.starts_with("Float") {
                Value::Float(n.as_f64().unwrap_or(0.0))
            } else if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::UInt(u)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }

        serde_json::Value::String(s) => decode_string(s, ty),

        // Objects appear for Map and Tuple with named fields; JSON is the
        // faithful rendering.
        other => Value::Json(other.clone()),
    }
}

fn decode_string(s: &str, ty: &str) -> Value {
    // Exact numerics and wide integers arrive quoted. Keeping the text is the
    // whole point — parsing would undo the precision the quoting preserved.
    if ty.starts_with("Decimal") {
        return Value::Numeric(s.to_string());
    }
    if ty.starts_with("Int") || ty.starts_with("UInt") {
        // Int128/256 and UInt64 upward exceed i64, so anything that does not fit
        // stays exact as a numeric string rather than being truncated.
        return match s.parse::<i64>() {
            Ok(i) => Value::Int(i),
            Err(_) => match s.parse::<u64>() {
                Ok(u) => Value::UInt(u),
                Err(_) => Value::Numeric(s.to_string()),
            },
        };
    }
    if ty.starts_with("Float") {
        // Denormals are quoted ("inf", "nan") when quote_denormals is on.
        return match s.parse::<f64>() {
            Ok(f) => Value::Float(f),
            Err(_) => Value::Text(s.to_string()),
        };
    }
    if ty.starts_with("UUID") {
        return uuid::Uuid::parse_str(s)
            .map(Value::Uuid)
            .unwrap_or_else(|_| Value::Text(s.to_string()));
    }
    if ty.starts_with("Date32") || ty == "Date" {
        return chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Value::Date)
            .unwrap_or_else(|_| Value::Text(s.to_string()));
    }
    if ty.starts_with("DateTime") {
        // `DateTime('UTC')` and `DateTime64(3, 'UTC')` carry a zone in the type
        // name, but the rendered value has no offset, so it is a wall-clock
        // reading. Promoting it to an instant would invent information.
        return chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
            .map(Value::DateTime)
            .unwrap_or_else(|_| Value::Text(s.to_string()));
    }
    if ty.starts_with("JSON") || ty.starts_with("Object(") {
        return serde_json::from_str(s)
            .map(Value::Json)
            .unwrap_or_else(|_| Value::Text(s.to_string()));
    }

    Value::Text(s.to_string())
}

/// Quote a string for a ClickHouse SQL literal.
///
/// Used for catalog lookups where the value is a database or table name. Both
/// backslash and single quote are escapes in ClickHouse string literals, so both
/// are doubled up.
pub fn literal_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nullable_and_lowcardinality_wrappers_are_stripped() {
        assert_eq!(unwrap_type("Nullable(String)"), "String");
        assert_eq!(unwrap_type("LowCardinality(String)"), "String");
        // Wrappers nest in either order.
        assert_eq!(unwrap_type("Nullable(LowCardinality(String))"), "String");
        assert_eq!(unwrap_type("String"), "String");
    }

    #[test]
    fn null_decodes_to_null() {
        assert_eq!(decode(&json!(null), "Nullable(Int64)"), Value::Null);
    }

    #[test]
    fn wide_integers_arrive_quoted_and_stay_exact() {
        // The server is asked to quote 64-bit integers precisely because a JSON
        // number is a double and would lose the low bits past 2^53.
        let big = "9223372036854775807"; // i64::MAX
        assert_eq!(
            decode(&json!(big), "Int64"),
            Value::Int(9_223_372_036_854_775_807)
        );

        let past_i64 = "18446744073709551615"; // u64::MAX
        assert_eq!(decode(&json!(past_i64), "UInt64"), Value::UInt(u64::MAX));

        // Int128 exceeds every fixed-width integer; it must stay exact as text.
        let huge = "170141183460469231731687303715884105727";
        assert_eq!(
            decode(&json!(huge), "Int128"),
            Value::Numeric(huge.to_string())
        );
    }

    #[test]
    fn decimals_keep_every_digit() {
        let exact = "12345678901234567890.123456789012345678";
        assert_eq!(
            decode(&json!(exact), "Decimal(38, 18)"),
            Value::Numeric(exact.to_string())
        );
    }

    #[test]
    fn floats_decode_including_denormals() {
        assert_eq!(decode(&json!(1.5), "Float64"), Value::Float(1.5));
        // With quote_denormals on, these arrive as strings.
        match decode(&json!("inf"), "Float64") {
            Value::Float(f) => assert!(f.is_infinite()),
            other => panic!("expected a float, got {other:?}"),
        }
    }

    #[test]
    fn dates_and_datetimes_are_distinguished() {
        assert!(matches!(
            decode(&json!("2026-08-13"), "Date"),
            Value::Date(_)
        ));
        // A DateTime with a zone in its *type* still renders without an offset,
        // so it stays a wall-clock reading rather than becoming an instant.
        assert!(matches!(
            decode(&json!("2026-08-13 11:30:00"), "DateTime('UTC')"),
            Value::DateTime(_)
        ));
        assert!(matches!(
            decode(&json!("2026-08-13 11:30:00.123"), "DateTime64(3)"),
            Value::DateTime(_)
        ));
    }

    #[test]
    fn arrays_decode_elementwise_with_the_inner_type() {
        let v = decode(&json!(["1", "2"]), "Array(Int64)");
        assert_eq!(v, Value::Array(vec![Value::Int(1), Value::Int(2)]));
    }

    #[test]
    fn uuids_decode_and_malformed_ones_stay_visible() {
        let u = "550e8400-e29b-41d4-a716-446655440000";
        assert!(matches!(decode(&json!(u), "UUID"), Value::Uuid(_)));
        // A value that does not parse is shown rather than dropped.
        assert_eq!(
            decode(&json!("not-a-uuid"), "UUID"),
            Value::Text("not-a-uuid".into())
        );
    }

    #[test]
    fn literals_escape_quotes_and_backslashes() {
        assert_eq!(literal_str("plain"), "'plain'");
        // Both are escape characters in ClickHouse string literals.
        assert_eq!(literal_str("O'Brien"), r"'O\'Brien'");
        assert_eq!(literal_str(r"back\slash"), r"'back\\slash'");
    }

    #[test]
    fn the_summary_header_parses_counts_given_as_strings() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "X-ClickHouse-Summary",
            r#"{"read_rows":"10","written_rows":"3"}"#.parse().unwrap(),
        );
        assert_eq!(Summary::from_headers(&headers).written_rows, 3);
    }

    #[test]
    fn a_missing_summary_header_is_zero_not_an_error() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(Summary::from_headers(&headers).written_rows, 0);
    }
}
