//! SQLite value decoding.
//!
//! SQLite is dynamically typed: a column declared `INTEGER` can hold a string,
//! and the declared type is only an *affinity* hint. Decoding therefore looks at
//! the storage class actually present, and consults the declared type only to
//! disambiguate cases where the storage class alone is not enough — an `INTEGER`
//! could be a number, a boolean, or a Unix timestamp, and only the declaration
//! distinguishes them.

use rusqlite::types::ValueRef;
use tablepro_core::Value;

/// The declared-type hint for one column, precomputed once per statement so we
/// do not re-parse the declaration for every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Boolean,
    /// Stored as text or as seconds since the epoch, depending on how it was written.
    DateTime,
    Date,
    Time,
    /// Exact numeric: must not round-trip through f64.
    Numeric,
    Json,
    Uuid,
    Blob,
    Other,
}

impl Affinity {
    /// Classify a SQLite declared type. Declarations are free-form strings, so
    /// this matches on substrings the way SQLite's own affinity rules do.
    pub fn from_decltype(decl: Option<&str>) -> Affinity {
        let Some(decl) = decl else {
            return Affinity::Other;
        };
        let d = decl.to_ascii_uppercase();

        // Order matters: "DATETIME" contains "DATE", so test the longer form first.
        if d.contains("BOOL") {
            Affinity::Boolean
        } else if d.contains("DATETIME") || d.contains("TIMESTAMP") {
            Affinity::DateTime
        } else if d.contains("DATE") {
            Affinity::Date
        } else if d.contains("TIME") {
            Affinity::Time
        } else if d.contains("DECIMAL") || d.contains("NUMERIC") || d.contains("MONEY") {
            Affinity::Numeric
        } else if d.contains("JSON") {
            Affinity::Json
        } else if d.contains("UUID") || d.contains("GUID") {
            Affinity::Uuid
        } else if d.contains("BLOB") || d.contains("BINARY") {
            Affinity::Blob
        } else {
            Affinity::Other
        }
    }
}

/// Decode one cell.
pub fn decode(raw: ValueRef<'_>, affinity: Affinity, decl: &str) -> Value {
    match raw {
        ValueRef::Null => Value::Null,

        ValueRef::Integer(i) => match affinity {
            Affinity::Boolean => Value::Bool(i != 0),
            // A DATETIME column holding an integer is a Unix timestamp in seconds.
            Affinity::DateTime => chrono::DateTime::from_timestamp(i, 0)
                .map(Value::TimestampTz)
                .unwrap_or(Value::Int(i)),
            // Report a DECIMAL column as exact-numeric even when SQLite happens to
            // have stored the value in the INTEGER storage class, so the UI treats
            // the column consistently regardless of which rows are integral.
            Affinity::Numeric => Value::Numeric(i.to_string()),
            _ => Value::Int(i),
        },

        ValueRef::Real(f) => match affinity {
            // A REAL in a DECIMAL column has already lost exactness in storage;
            // render it without pretending to a precision it does not have.
            Affinity::Numeric => Value::Numeric(format_real(f)),
            _ => Value::Float(f),
        },

        ValueRef::Text(bytes) => {
            let Ok(s) = std::str::from_utf8(bytes) else {
                // Invalid UTF-8 in a TEXT column: surface the bytes rather than
                // losing the value to a lossy conversion.
                return Value::Bytes(bytes.to_vec());
            };
            decode_text(s, affinity, decl)
        }

        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
    }
}

fn decode_text(s: &str, affinity: Affinity, decl: &str) -> Value {
    match affinity {
        Affinity::Boolean => match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "yes" => Value::Bool(true),
            "0" | "false" | "f" | "no" => Value::Bool(false),
            _ => Value::Text(s.to_string()),
        },

        Affinity::Numeric => {
            // Keep the exact text. Parsing to f64 here would be the precision
            // bug this whole value model exists to avoid.
            if s.parse::<f64>().is_ok() {
                Value::Numeric(s.to_string())
            } else {
                Value::Text(s.to_string())
            }
        }

        Affinity::Date => parse_date(s)
            .map(Value::Date)
            .unwrap_or_else(|| fallback(s, decl)),

        Affinity::Time => chrono::NaiveTime::parse_from_str(s, "%H:%M:%S")
            .or_else(|_| chrono::NaiveTime::parse_from_str(s, "%H:%M:%S%.f"))
            .map(Value::Time)
            .unwrap_or_else(|_| fallback(s, decl)),

        Affinity::DateTime => parse_datetime(s).unwrap_or_else(|| fallback(s, decl)),

        Affinity::Json => serde_json::from_str(s)
            .map(Value::Json)
            .unwrap_or_else(|_| Value::Text(s.to_string())),

        Affinity::Uuid => uuid::Uuid::parse_str(s)
            .map(Value::Uuid)
            .unwrap_or_else(|_| Value::Text(s.to_string())),

        Affinity::Blob | Affinity::Other => Value::Text(s.to_string()),
    }
}

/// A value that does not match its declared type is still shown, tagged with the
/// declaration so the mismatch is visible rather than silently coerced.
fn fallback(s: &str, decl: &str) -> Value {
    Value::Unsupported {
        type_name: decl.to_string(),
        raw: s.to_string(),
    }
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn parse_datetime(s: &str) -> Option<Value> {
    // SQLite has no canonical datetime format; these are the ones its own
    // date functions emit, plus ISO 8601 with a zone.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(Value::TimestampTz(dt.into()));
    }
    for fmt in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Value::DateTime(dt));
        }
    }
    // A bare date in a DATETIME column is midnight.
    parse_date(s).map(|d| Value::DateTime(d.into()))
}

fn format_real(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e15 {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}

/// Convert a [`Value`] back into something rusqlite can bind, for inline edits.
pub fn to_sql(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::Bool(b) => S::Integer(i64::from(*b)),
        Value::Int(i) => S::Integer(*i),
        Value::UInt(u) => i64::try_from(*u)
            .map(S::Integer)
            .unwrap_or(S::Real(*u as f64)),
        Value::Float(f) => S::Real(*f),
        // Exact numerics bind as text so SQLite stores the digits we were given.
        Value::Numeric(s) => S::Text(s.clone()),
        Value::Text(s) => S::Text(s.clone()),
        Value::Bytes(b) => S::Blob(b.clone()),
        Value::Uuid(u) => S::Text(u.to_string()),
        Value::Date(d) => S::Text(d.to_string()),
        Value::Time(t) => S::Text(t.to_string()),
        Value::DateTime(dt) => S::Text(dt.format("%Y-%m-%d %H:%M:%S%.f").to_string()),
        Value::TimestampTz(dt) => S::Text(dt.to_rfc3339()),
        Value::Json(j) => S::Text(j.to_string()),
        Value::Interval { .. } | Value::Array(_) => S::Text(v.to_string()),
        Value::Unsupported { raw, .. } => S::Text(raw.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_is_matched_before_date() {
        // "DATETIME" contains "DATE"; a naive substring order would misclassify it.
        assert_eq!(
            Affinity::from_decltype(Some("DATETIME")),
            Affinity::DateTime
        );
        assert_eq!(Affinity::from_decltype(Some("DATE")), Affinity::Date);
        assert_eq!(
            Affinity::from_decltype(Some("TIMESTAMP WITH TIME ZONE")),
            Affinity::DateTime
        );
    }

    #[test]
    fn declared_types_are_matched_case_insensitively() {
        assert_eq!(Affinity::from_decltype(Some("boolean")), Affinity::Boolean);
        assert_eq!(
            Affinity::from_decltype(Some("Decimal(10,2)")),
            Affinity::Numeric
        );
        assert_eq!(Affinity::from_decltype(None), Affinity::Other);
    }

    #[test]
    fn integers_in_boolean_columns_decode_as_bools() {
        assert_eq!(
            decode(ValueRef::Integer(1), Affinity::Boolean, "BOOLEAN"),
            Value::Bool(true)
        );
        assert_eq!(
            decode(ValueRef::Integer(0), Affinity::Boolean, "BOOLEAN"),
            Value::Bool(false)
        );
        // The same storage class in a plain INTEGER column stays an integer.
        assert_eq!(
            decode(ValueRef::Integer(1), Affinity::Other, "INTEGER"),
            Value::Int(1)
        );
    }

    #[test]
    fn exact_numerics_keep_their_digits() {
        // The whole point of the Numeric variant: no f64 in the path.
        let raw = "123456789012345678901234567890.99";
        let v = decode(ValueRef::Text(raw.as_bytes()), Affinity::Numeric, "DECIMAL");
        assert_eq!(v, Value::Numeric(raw.to_string()));
    }

    #[test]
    fn text_that_contradicts_its_declared_type_is_still_shown() {
        // SQLite permits this; blanking the cell would hide real data.
        let v = decode(ValueRef::Text(b"not a date"), Affinity::Date, "DATE");
        assert_eq!(
            v,
            Value::Unsupported {
                type_name: "DATE".into(),
                raw: "not a date".into()
            }
        );
    }

    #[test]
    fn invalid_utf8_in_a_text_column_falls_back_to_bytes() {
        // Lossy conversion would silently corrupt the value.
        let bad = [0xff, 0xfe, 0x00];
        let v = decode(ValueRef::Text(&bad), Affinity::Other, "TEXT");
        assert_eq!(v, Value::Bytes(bad.to_vec()));
    }

    #[test]
    fn datetimes_parse_in_sqlite_native_formats() {
        for s in [
            "2026-08-13 11:30:00",
            "2026-08-13T11:30:00",
            "2026-08-13 11:30",
        ] {
            let v = decode(ValueRef::Text(s.as_bytes()), Affinity::DateTime, "DATETIME");
            assert!(matches!(v, Value::DateTime(_)), "failed on {s}: {v:?}");
        }
        let v = decode(
            ValueRef::Text(b"2026-08-13T11:30:00Z"),
            Affinity::DateTime,
            "DATETIME",
        );
        assert!(matches!(v, Value::TimestampTz(_)), "got {v:?}");
    }

    #[test]
    fn integer_datetimes_are_read_as_unix_timestamps() {
        let v = decode(ValueRef::Integer(0), Affinity::DateTime, "DATETIME");
        match v {
            Value::TimestampTz(dt) => assert_eq!(dt.timestamp(), 0),
            other => panic!("expected TimestampTz, got {other:?}"),
        }
    }

    #[test]
    fn binding_round_trips_exact_numerics_as_text() {
        // Binding through Real would reintroduce the rounding we avoided on read.
        let v = Value::Numeric("0.1234567890123456789".into());
        assert_eq!(
            to_sql(&v),
            rusqlite::types::Value::Text("0.1234567890123456789".into())
        );
    }

    #[test]
    fn oversized_unsigned_values_do_not_wrap() {
        // u64::MAX does not fit in i64; wrapping would change the value silently.
        let v = Value::UInt(u64::MAX);
        assert!(matches!(to_sql(&v), rusqlite::types::Value::Real(_)));
    }
}
