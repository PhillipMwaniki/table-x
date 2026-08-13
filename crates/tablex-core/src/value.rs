//! The dynamic value model.
//!
//! A database client sees values whose types are only known at runtime, and it must
//! survive types it has never heard of. Every driver decodes into [`Value`], and the
//! frontend renders and edits [`Value`] without knowing which database produced it.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A single cell value, decoded from any supported database.
///
/// # Design notes
///
/// - Exact numerics are carried as strings ([`Value::Numeric`]) rather than `f64`.
///   `NUMERIC(65,30)` in MySQL and unbounded `NUMERIC` in PostgreSQL both exceed
///   every fixed-width decimal type, and silently rounding a monetary column is
///   the kind of bug a database client must never introduce.
/// - Unknown types degrade to [`Value::Unsupported`] instead of failing the query.
///   PostGIS geometry, custom enums, and range types should still be *visible*
///   even when we cannot interpret them; one exotic column must never blank out
///   an entire result set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// SQL `NULL`. Distinct from an empty string, and rendered differently.
    Null,
    Bool(bool),
    /// Signed integers up to 64 bits.
    Int(i64),
    /// Unsigned 64-bit values that do not fit in `i64` (MySQL `BIGINT UNSIGNED`).
    UInt(u64),
    /// Approximate numerics (`REAL`, `DOUBLE PRECISION`, `FLOAT`).
    Float(f64),
    /// Exact numerics, kept in their canonical textual form. Lossless.
    Numeric(String),
    Text(String),
    /// Binary data (`BYTEA`, `BLOB`, `VARBINARY`).
    Bytes(Vec<u8>),
    Uuid(uuid::Uuid),
    /// Calendar date with no time or zone.
    Date(chrono::NaiveDate),
    /// Wall-clock time with no date or zone.
    Time(chrono::NaiveTime),
    /// Timestamp without time zone — a wall-clock reading, not an instant.
    DateTime(chrono::NaiveDateTime),
    /// Timestamp with time zone — a true instant, normalized to UTC.
    TimestampTz(chrono::DateTime<chrono::Utc>),
    /// An interval, kept in months/days/microseconds because those units are not
    /// interconvertible: months vary in length and days vary across DST boundaries.
    Interval {
        months: i32,
        days: i32,
        micros: i64,
    },
    Json(serde_json::Value),
    /// Homogeneous arrays (PostgreSQL `int[]`, `text[]`, …).
    Array(Vec<Value>),
    /// A value we can display but not interpret. Carries the database's own type
    /// name so the UI can label it accurately.
    Unsupported {
        type_name: String,
        raw: String,
    },
}

impl Value {
    /// Whether this value is SQL `NULL`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// A short, stable identifier for the value's kind, used by the UI to pick
    /// an alignment, an editor widget, and a color.
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Null => ValueKind::Null,
            Value::Bool(_) => ValueKind::Bool,
            Value::Int(_) | Value::UInt(_) => ValueKind::Integer,
            Value::Float(_) | Value::Numeric(_) => ValueKind::Number,
            Value::Text(_) => ValueKind::Text,
            Value::Bytes(_) => ValueKind::Binary,
            Value::Uuid(_) => ValueKind::Uuid,
            Value::Date(_) | Value::Time(_) | Value::DateTime(_) | Value::TimestampTz(_) => {
                ValueKind::Temporal
            }
            Value::Interval { .. } => ValueKind::Interval,
            Value::Json(_) => ValueKind::Json,
            Value::Array(_) => ValueKind::Array,
            Value::Unsupported { .. } => ValueKind::Unknown,
        }
    }

    /// Whether the grid should right-align this value.
    pub fn is_numeric(&self) -> bool {
        matches!(self.kind(), ValueKind::Integer | ValueKind::Number)
    }
}

impl fmt::Display for Value {
    /// Renders a value for display. Binary is summarized rather than dumped —
    /// a 4 MB BLOB must not be stringified into a grid cell.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::UInt(u) => write!(f, "{u}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Numeric(s) => write!(f, "{s}"),
            Value::Text(s) => write!(f, "{s}"),
            Value::Bytes(b) => write!(f, "[{} bytes]", b.len()),
            Value::Uuid(u) => write!(f, "{u}"),
            Value::Date(d) => write!(f, "{d}"),
            Value::Time(t) => write!(f, "{t}"),
            Value::DateTime(dt) => write!(f, "{}", dt.format("%Y-%m-%d %H:%M:%S%.f")),
            Value::TimestampTz(dt) => write!(f, "{}", dt.format("%Y-%m-%d %H:%M:%S%.f %Z")),
            Value::Interval {
                months,
                days,
                micros,
            } => write!(f, "{months} months {days} days {micros} µs"),
            Value::Json(j) => write!(f, "{j}"),
            Value::Array(items) => {
                write!(f, "{{")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "}}")
            }
            Value::Unsupported { raw, .. } => write!(f, "{raw}"),
        }
    }
}

/// Coarse classification of a [`Value`], used for UI affordances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Null,
    Bool,
    Integer,
    Number,
    Text,
    Binary,
    Uuid,
    Temporal,
    Interval,
    Json,
    Array,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_distinct_from_empty_text() {
        assert!(Value::Null.is_null());
        assert!(!Value::Text(String::new()).is_null());
        assert_ne!(Value::Null, Value::Text(String::new()));
    }

    #[test]
    fn exact_numerics_survive_a_round_trip() {
        // A value with more precision than f64 or any 128-bit decimal can hold.
        let huge = "123456789012345678901234567890.123456789012345678901234567890";
        let v = Value::Numeric(huge.to_string());
        assert_eq!(v.to_string(), huge);

        let json = serde_json::to_string(&v).expect("serialize");
        let back: Value = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(v, back, "exact numerics must not be rounded in transit");
    }

    #[test]
    fn binary_is_summarized_not_dumped() {
        let v = Value::Bytes(vec![0u8; 4_000_000]);
        assert_eq!(v.to_string(), "[4000000 bytes]");
    }

    #[test]
    fn unknown_types_stay_visible() {
        let v = Value::Unsupported {
            type_name: "geometry".into(),
            raw: "POINT(1 2)".into(),
        };
        assert_eq!(v.to_string(), "POINT(1 2)");
        assert_eq!(v.kind(), ValueKind::Unknown);
    }

    #[test]
    fn numeric_kinds_are_right_aligned() {
        assert!(Value::Int(1).is_numeric());
        assert!(Value::Numeric("1.5".into()).is_numeric());
        assert!(!Value::Text("1".into()).is_numeric());
    }
}
