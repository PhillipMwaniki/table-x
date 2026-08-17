//! MySQL value decoding.
//!
//! The text protocol hands back most values as raw bytes, so the column's
//! declared type is what says how to read them. `DECIMAL` in particular must be
//! kept as text: MySQL allows `DECIMAL(65, 30)`, which no fixed-width numeric
//! type in Rust can hold, and routing it through `f64` would silently round the
//! exact column types people use for money.

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::{Column, Value as My};
use tablex_core::Value;

/// Decode one cell against its column metadata.
pub fn decode(raw: &My, column: &Column) -> Value {
    let unsigned = column.flags().contains(ColumnFlags::UNSIGNED_FLAG);
    let binary = column.flags().contains(ColumnFlags::BINARY_FLAG);

    match raw {
        My::NULL => Value::Null,

        My::Int(i) => decode_int(*i, column),
        My::UInt(u) => {
            // A u64 past i64::MAX is exactly why the Value model has a separate
            // unsigned variant; narrowing here would wrap it into a negative.
            if let Ok(i) = i64::try_from(*u) {
                decode_int(i, column)
            } else {
                Value::UInt(*u)
            }
        }

        My::Float(f) => Value::Float(f64::from(*f)),
        My::Double(f) => Value::Float(*f),

        My::Date(y, m, d, h, min, s, micro) => {
            decode_datetime(*y, *m, *d, *h, *min, *s, *micro, column)
        }

        My::Time(neg, days, hours, minutes, seconds, micros) => {
            // MySQL TIME is a signed duration, not a clock reading: it legally
            // spans -838:59:59 to 838:59:59, so it cannot be a NaiveTime.
            let total = i64::from(*days) * 86_400_000_000
                + i64::from(*hours) * 3_600_000_000
                + i64::from(*minutes) * 60_000_000
                + i64::from(*seconds) * 1_000_000
                + i64::from(*micros);
            Value::Interval {
                months: 0,
                days: 0,
                micros: if *neg { -total } else { total },
            }
        }

        My::Bytes(bytes) => decode_bytes(bytes, column, unsigned, binary),
    }
}

/// Decode the binary protocol's packed date/time form.
///
/// The same variant carries `DATE`, `DATETIME`, and `TIMESTAMP`; the column type
/// is what says whether the time part is meaningful.
#[allow(clippy::too_many_arguments)]
fn decode_datetime(
    y: u16,
    m: u8,
    d: u8,
    h: u8,
    min: u8,
    s: u8,
    micro: u32,
    column: &Column,
) -> Value {
    // MySQL permits the zero date, which no calendar type can represent.
    // Surfacing it keeps a real stored value visible instead of erroring.
    if y == 0 && m == 0 && d == 0 {
        return Value::Unsupported {
            type_name: "date".into(),
            raw: "0000-00-00".into(),
        };
    }

    let Some(date) = chrono::NaiveDate::from_ymd_opt(i32::from(y), u32::from(m), u32::from(d))
    else {
        return Value::Unsupported {
            type_name: "date".into(),
            raw: format!("{y:04}-{m:02}-{d:02}"),
        };
    };

    if column.column_type() == ColumnType::MYSQL_TYPE_DATE {
        return Value::Date(date);
    }

    match date.and_hms_micro_opt(u32::from(h), u32::from(min), u32::from(s), micro) {
        // Deliberately a wall-clock reading, not an instant: the server does not
        // tell us which zone it was recorded in, and inventing one would shift
        // the displayed value.
        Some(dt) => Value::DateTime(dt),
        None => Value::Date(date),
    }
}

fn decode_int(i: i64, column: &Column) -> Value {
    // TINYINT(1) is how MySQL spells BOOLEAN; the server has no separate type.
    if column.column_type() == ColumnType::MYSQL_TYPE_TINY && column.column_length() == 1 {
        return Value::Bool(i != 0);
    }
    Value::Int(i)
}

/// Decode an integer the text protocol handed over as digits.
///
/// This is the path that matters in practice: `query_iter` speaks the text
/// protocol, so every value the grid shows arrives as bytes and lands here,
/// while the binary protocol's already-typed `UInt` is handled in [`decode`].
///
/// The unsigned flag is the only thing that says whether the digits fit an
/// `i64`. Parsing as `i64` unconditionally fails above `i64::MAX` and then falls
/// through to text, which turned `UNSIGNED BIGINT` values near the top of the
/// range into strings — sortable as text, not as numbers, and no longer marked
/// exact.
fn decode_int_text(bytes: &[u8], column: &Column, unsigned: bool) -> Value {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return Value::Bytes(bytes.to_vec());
    };

    match parse_int_text(s, unsigned) {
        Some(IntText::Signed(i)) => decode_int(i, column),
        Some(IntText::Unsigned(u)) => Value::UInt(u),
        None => text_or_bytes(bytes),
    }
}

/// Which integer variant a run of digits needs to stay exact.
#[derive(Debug, PartialEq, Eq)]
enum IntText {
    /// Fits an `i64`, so the column's own rules still apply — `TINYINT(1)` is a
    /// bool, everything else an `Int`.
    Signed(i64),
    /// Past `i64::MAX` and unsigned, so only `UInt` holds it without wrapping.
    Unsigned(u64),
}

/// Widest exact variant a run of integer digits fits, or `None` if they are not
/// an integer at all.
///
/// Split out from [`decode_int_text`] so the rule can be tested without
/// fabricating a `mysql_async::Column`, which has no public constructor — and
/// this is a rule worth testing offline, since getting it wrong silently
/// downgrades an exact value rather than failing.
fn parse_int_text(s: &str, unsigned: bool) -> Option<IntText> {
    // Tried first only when the column says unsigned. A signed column holding
    // "-1" must not be read as a u64 — `parse::<u64>` rejects it anyway, so this
    // is belt as much as order.
    if unsigned {
        if let Ok(u) = s.parse::<u64>() {
            return Some(match i64::try_from(u) {
                Ok(i) => IntText::Signed(i),
                Err(_) => IntText::Unsigned(u),
            });
        }
    }
    s.parse::<i64>().ok().map(IntText::Signed)
}

fn decode_bytes(bytes: &[u8], column: &Column, unsigned: bool, binary: bool) -> Value {
    use ColumnType::*;

    match column.column_type() {
        // Exact numerics: keep every digit. This is the whole reason the text
        // form is preserved rather than parsed.
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => match std::str::from_utf8(bytes) {
            Ok(s) => Value::Numeric(s.to_string()),
            Err(_) => Value::Bytes(bytes.to_vec()),
        },

        MYSQL_TYPE_JSON => std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| serde_json::from_str(s).ok())
            .map(Value::Json)
            .unwrap_or_else(|| text_or_bytes(bytes)),

        MYSQL_TYPE_BIT => {
            // BIT arrives big-endian in as few bytes as fit the declared width.
            let mut n: u64 = 0;
            for b in bytes.iter().take(8) {
                n = (n << 8) | u64::from(*b);
            }
            if column.column_length() == 1 {
                Value::Bool(n != 0)
            } else {
                Value::UInt(n)
            }
        }

        MYSQL_TYPE_BLOB
        | MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_GEOMETRY
            if binary =>
        {
            // The BINARY flag is what separates BLOB from TEXT: they share a
            // column type and differ only by collation.
            Value::Bytes(bytes.to_vec())
        }

        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING if binary => {
            Value::Bytes(bytes.to_vec())
        }

        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => parse_text_date(bytes),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_TIMESTAMP => parse_text_datetime(bytes),

        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .map(Value::Float)
            .unwrap_or_else(|| text_or_bytes(bytes)),

        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR => decode_int_text(bytes, column, unsigned),

        _ => text_or_bytes(bytes),
    }
}

/// UTF-8 if it is valid, raw bytes otherwise. A lossy conversion would corrupt
/// values in non-UTF-8 collations, which MySQL still has plenty of.
fn text_or_bytes(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(s) => Value::Text(s.to_string()),
        Err(_) => Value::Bytes(bytes.to_vec()),
    }
}

fn parse_text_date(bytes: &[u8]) -> Value {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return Value::Bytes(bytes.to_vec());
    };
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(Value::Date)
        .unwrap_or_else(|_| zero_aware(s, "date"))
}

fn parse_text_datetime(bytes: &[u8]) -> Value {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return Value::Bytes(bytes.to_vec());
    };
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .map(Value::DateTime)
        .unwrap_or_else(|_| zero_aware(s, "datetime"))
}

/// MySQL permits the zero date `0000-00-00`, which is not a real calendar date
/// and no date type can represent. Surfacing it as unsupported keeps it visible
/// instead of turning it into a wrong date or an error.
fn zero_aware(s: &str, type_name: &str) -> Value {
    Value::Unsupported {
        type_name: type_name.to_string(),
        raw: s.to_string(),
    }
}

/// Render a [`Value`] as a parameter for an inline edit.
///
/// Everything is sent as text and converted by the server, so exact numerics
/// keep the digits they arrived with.
pub fn to_param(v: &Value) -> Option<String> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => i32::from(*b).to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Numeric(s) => s.clone(),
        Value::Text(s) => s.clone(),
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Uuid(u) => u.to_string(),
        Value::Date(d) => d.to_string(),
        Value::Time(t) => t.to_string(),
        Value::DateTime(dt) => dt.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
        Value::TimestampTz(dt) => dt.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string(),
        Value::Json(j) => j.to_string(),
        Value::Interval { micros, .. } => {
            let secs = micros / 1_000_000;
            format!(
                "{:02}:{:02}:{:02}",
                secs / 3600,
                (secs / 60) % 60,
                secs % 60
            )
        }
        Value::Array(_) => v.to_string(),
        Value::Unsupported { raw, .. } => raw.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_decimals_keep_every_digit() {
        // DECIMAL(65,30) is legal MySQL and exceeds every fixed-width numeric
        // type; parsing it here would be the precision bug this avoids.
        let raw = "12345678901234567890123456789012345.123456789012345678901234567890";
        assert_eq!(
            super::text_or_bytes(raw.as_bytes()),
            Value::Text(raw.to_string()),
            "sanity: the helper itself does not mangle text"
        );
    }

    #[test]
    fn invalid_utf8_falls_back_to_bytes() {
        // MySQL still ships latin1 columns; a lossy decode would corrupt them.
        let bad = [0xff, 0xfe];
        assert_eq!(text_or_bytes(&bad), Value::Bytes(bad.to_vec()));
    }

    #[test]
    fn the_zero_date_stays_visible() {
        // '0000-00-00' is not a real date, but blanking it would hide data that
        // is genuinely in the table.
        match zero_aware("0000-00-00", "date") {
            Value::Unsupported { type_name, raw } => {
                assert_eq!(type_name, "date");
                assert_eq!(raw, "0000-00-00");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn exact_numerics_round_trip_through_to_param() {
        let s = "99999999999999999999.99999999999999999999";
        assert_eq!(to_param(&Value::Numeric(s.into())).as_deref(), Some(s));
    }

    #[test]
    fn null_has_no_parameter_text() {
        assert_eq!(to_param(&Value::Null), None);
    }

    #[test]
    fn unsigned_integers_past_i64_stay_exact() {
        // The bug this pins: `query_iter` speaks the text protocol, so every
        // value the grid shows arrives as digits. Parsing them as i64 first
        // fails above i64::MAX and fell through to text, so an UNSIGNED BIGINT
        // near the top of its range was shown as a string — sorted as text and
        // no longer marked exact.
        assert_eq!(
            parse_int_text("18446744073709551615", true),
            Some(IntText::Unsigned(u64::MAX))
        );
    }

    #[test]
    fn values_that_fit_i64_stay_signed_so_the_column_rules_still_apply() {
        // Narrowing when it fits is what keeps TINYINT(1) decoding as a bool
        // rather than as 1 and 0: that decision belongs to `decode_int`, which
        // only sees the signed case.
        assert_eq!(parse_int_text("1", true), Some(IntText::Signed(1)));
        assert_eq!(
            parse_int_text("9223372036854775807", true),
            Some(IntText::Signed(i64::MAX))
        );
    }

    #[test]
    fn a_signed_column_never_reads_its_value_as_unsigned() {
        // Negatives must survive, and a signed column holding a value that only
        // fits a u64 is a contradiction — reporting the digits as text is the
        // honest answer, not silently choosing a variant for it.
        assert_eq!(parse_int_text("-1", false), Some(IntText::Signed(-1)));
        assert_eq!(parse_int_text("-1", true), Some(IntText::Signed(-1)));
        assert_eq!(parse_int_text("18446744073709551615", false), None);
    }

    #[test]
    fn what_is_not_an_integer_is_not_claimed_to_be_one() {
        assert_eq!(parse_int_text("", true), None);
        assert_eq!(parse_int_text("12.5", true), None);
        assert_eq!(parse_int_text("banana", false), None);
    }

    #[test]
    fn booleans_bind_as_mysqls_own_spelling() {
        // MySQL has no BOOLEAN type; TINYINT(1) with 0/1 is the convention.
        assert_eq!(to_param(&Value::Bool(true)).as_deref(), Some("1"));
        assert_eq!(to_param(&Value::Bool(false)).as_deref(), Some("0"));
    }
}
