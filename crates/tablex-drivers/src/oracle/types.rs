//! Oracle value decoding.
//!
//! ODPI hands back a `SqlValue` that knows its own Oracle type, so this maps
//! that onto the shared [`Value`] model.
//!
//! Two decisions carry most of the weight here.
//!
//! **`NUMBER` becomes text, not a float.** Oracle's `NUMBER` is a
//! base-100 exact decimal with up to 38 significant digits. A double holds 15
//! or 16, so reading a price or an id through `f64` is not a rounding risk in
//! the abstract — it is data loss on ordinary values, and it is silent. The
//! digits are taken as Oracle formatted them and kept as [`Value::Numeric`].
//! `NUMBER` with a zero scale is still an integer as far as anybody reading it
//! is concerned, so it becomes [`Value::Int`] when it fits in one.
//!
//! **`DATE` is a timestamp.** Oracle's `DATE` carries a time of day, unlike
//! every other engine's, and rendering it as a calendar date would quietly drop
//! the hours somebody stored in it.

use oracle::sql_type::{OracleType, Timestamp};
use oracle::SqlValue;
use tablex_core::Value;

/// Decode one cell.
pub fn decode(value: &SqlValue, oracle_type: &OracleType) -> Value {
    // Asked first and separately: a null of any type is a null, and several of
    // the conversions below would otherwise report an error for one.
    match value.is_null() {
        Ok(true) => return Value::Null,
        Ok(false) => {}
        Err(_) => return Value::Null,
    }

    match oracle_type {
        OracleType::Boolean => value
            .get::<bool>()
            .map(Value::Bool)
            .unwrap_or_else(|_| text(value)),

        // Integer types that cannot lose anything on the way through i64.
        OracleType::Int64 => value
            .get::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| text(value)),

        // Approximate by definition: these *are* IEEE floats in the database,
        // so a float is the honest representation rather than a lossy one.
        OracleType::BinaryFloat | OracleType::BinaryDouble => value
            .get::<f64>()
            .map(Value::Float)
            .unwrap_or_else(|_| text(value)),

        // See the module note: exact, and up to 38 digits. A scale of zero is
        // an integer as far as anybody reading it is concerned.
        OracleType::Number(_, 0) => match value.get::<i64>() {
            Ok(n) => Value::Int(n),
            // Too big for an i64 — 38 digits is far past one — so the digits
            // themselves are the value.
            Err(_) => numeric(value),
        },
        OracleType::Number(..) | OracleType::Float(_) => numeric(value),

        OracleType::Date | OracleType::Timestamp(_) => match value.get::<Timestamp>() {
            Ok(ts) => timestamp(&ts),
            Err(_) => text(value),
        },

        // With a zone, so a true instant rather than a wall-clock reading. The
        // offset is applied here and the result normalised to UTC, which is
        // what `TimestampTz` means.
        OracleType::TimestampTZ(_) | OracleType::TimestampLTZ(_) => {
            match value.get::<Timestamp>() {
                Ok(ts) => zoned(&ts),
                Err(_) => text(value),
            }
        }

        // Oracle's two intervals are not interconvertible with each other, which
        // is exactly why the value model keeps months, days and micros apart.
        OracleType::IntervalYM(_) => match value.get::<oracle::sql_type::IntervalYM>() {
            Ok(iv) => Value::Interval {
                months: iv.years() * 12 + iv.months(),
                days: 0,
                micros: 0,
            },
            Err(_) => text(value),
        },
        OracleType::IntervalDS(..) => match value.get::<oracle::sql_type::IntervalDS>() {
            Ok(iv) => Value::Interval {
                months: 0,
                days: iv.days(),
                micros: i64::from(iv.hours()) * 3_600_000_000
                    + i64::from(iv.minutes()) * 60_000_000
                    + i64::from(iv.seconds()) * 1_000_000
                    + i64::from(iv.nanoseconds()) / 1_000,
            },
            Err(_) => text(value),
        },

        OracleType::Raw(_) | OracleType::LongRaw | OracleType::BLOB | OracleType::BFILE => value
            .get::<Vec<u8>>()
            .map(Value::Bytes)
            .unwrap_or_else(|_| text(value)),

        // A JSON column is JSON; anything that will not parse is still text
        // somebody wants to read rather than an error.
        OracleType::Json => match value.get::<String>() {
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(json) => Value::Json(json),
                Err(_) => Value::Text(raw),
            },
            Err(_) => text(value),
        },

        // Everything textual, including the LOBs and XML: what a reader wants
        // from these is their text.
        OracleType::Varchar2(_)
        | OracleType::NVarchar2(_)
        | OracleType::Char(_)
        | OracleType::NChar(_)
        | OracleType::Long
        | OracleType::CLOB
        | OracleType::NCLOB
        | OracleType::Xml
        | OracleType::Rowid => text(value),

        // A cursor or an object type has no rendering that would mean anything
        // in a grid cell. Named rather than shown as an empty cell, so the type
        // is visible instead of the value looking absent.
        other => Value::Unsupported {
            type_name: other.to_string(),
            raw: value.get::<String>().unwrap_or_default(),
        },
    }
}

/// The digits exactly as Oracle formatted them.
fn numeric(value: &SqlValue) -> Value {
    match value.get::<String>() {
        Ok(text) => Value::Numeric(text),
        Err(_) => Value::Null,
    }
}

fn text(value: &SqlValue) -> Value {
    match value.get::<String>() {
        Ok(text) => Value::Text(text),
        // A value that will not render as text at all is not a null, and saying
        // so is better than pretending it is one.
        Err(e) => Value::Unsupported {
            type_name: "unknown".into(),
            raw: e.to_string(),
        },
    }
}

/// An Oracle `DATE` or `TIMESTAMP` as a wall-clock reading.
fn timestamp(ts: &Timestamp) -> Value {
    let date = chrono::NaiveDate::from_ymd_opt(ts.year(), ts.month(), ts.day());
    let time =
        chrono::NaiveTime::from_hms_nano_opt(ts.hour(), ts.minute(), ts.second(), ts.nanosecond());
    match (date, time) {
        (Some(date), Some(time)) => Value::DateTime(date.and_time(time)),
        // Out of chrono's range rather than out of Oracle's — year 0, or a
        // proleptic date. The text is still readable.
        _ => Value::Unsupported {
            type_name: "timestamp".into(),
            raw: ts.to_string(),
        },
    }
}

/// An Oracle `TIMESTAMP WITH TIME ZONE` as an instant.
fn zoned(ts: &Timestamp) -> Value {
    let Value::DateTime(naive) = timestamp(ts) else {
        return timestamp(ts);
    };
    match chrono::FixedOffset::east_opt(ts.tz_offset()) {
        Some(offset) => match naive.and_local_timezone(offset).single() {
            Some(local) => Value::TimestampTz(local.with_timezone(&chrono::Utc)),
            // An hour that does not exist in that offset, or one that happens
            // twice. Rather than picking one, the reading is kept as it was
            // stored.
            None => Value::DateTime(naive),
        },
        None => Value::DateTime(naive),
    }
}

/// A value as an Oracle literal.
///
/// Escaped literals rather than bound parameters, matching every other driver
/// here: the statement is built from a row the grid already has, and one string
/// is easier to show the user before it runs than a statement plus a parameter
/// list.
pub fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        // Oracle has no boolean in SQL before 23c, and a column that holds one
        // is a NUMBER(1) or a CHAR(1). One and zero is what such a column
        // holds.
        Value::Bool(b) => i32::from(*b).to_string(),
        Value::Int(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        // Unquoted so the server parses it as an exact numeric, keeping the
        // digits rather than coercing through a float.
        Value::Numeric(s) if is_numeric_literal(s) => s.clone(),
        Value::Bytes(bytes) => format!(
            "HEXTORAW('{}')",
            bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()
        ),
        // Dates and timestamps are given to Oracle in an explicit format rather
        // than left to NLS_DATE_FORMAT, which varies by session and would make
        // the same statement mean different things to two users.
        Value::Date(d) => format!("DATE '{d}'"),
        Value::DateTime(dt) => format!(
            "TO_TIMESTAMP('{}', 'YYYY-MM-DD HH24:MI:SS.FF')",
            dt.format("%Y-%m-%d %H:%M:%S%.6f")
        ),
        Value::TimestampTz(ts) => format!(
            "TO_TIMESTAMP_TZ('{}', 'YYYY-MM-DD HH24:MI:SS.FF TZH:TZM')",
            ts.format("%Y-%m-%d %H:%M:%S%.6f %:z")
        ),
        Value::Time(t) => quoted(&t.to_string()),
        other => quoted(&other.to_string()),
    }
}

/// Whether text is a plain decimal that can go into a statement unquoted.
fn is_numeric_literal(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit() || c == '.')
        && body.chars().filter(|c| *c == '.').count() <= 1
}

/// A quoted Oracle string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The type name to show in a column header.
///
/// Oracle's own spelling, including the size or precision, because that is what
/// the column was declared as and what somebody comparing two schemas needs to
/// see.
pub fn type_name(oracle_type: &OracleType) -> String {
    oracle_type.to_string()
}
