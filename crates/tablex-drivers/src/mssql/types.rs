//! SQL Server value decoding.
//!
//! tiberius decodes the TDS wire format into [`ColumnData`], so this module maps
//! that onto the shared [`Value`] model rather than parsing bytes itself.

use tablex_core::Value;
use tiberius::{ColumnData, ColumnType};

/// Decode one cell.
pub fn decode(data: &ColumnData<'static>) -> Value {
    match data {
        ColumnData::U8(v) => opt(v.map(|n| Value::Int(i64::from(n)))),
        ColumnData::I16(v) => opt(v.map(|n| Value::Int(i64::from(n)))),
        ColumnData::I32(v) => opt(v.map(|n| Value::Int(i64::from(n)))),
        ColumnData::I64(v) => opt(v.map(Value::Int)),
        ColumnData::F32(v) => opt(v.map(|n| Value::Float(f64::from(n)))),
        ColumnData::F64(v) => opt(v.map(Value::Float)),
        ColumnData::Bit(v) => opt(v.map(Value::Bool)),

        // DECIMAL, NUMERIC, MONEY, SMALLMONEY. `Numeric` renders itself with the
        // right scale, so the exact digits survive without going through f64.
        ColumnData::Numeric(v) => opt(v.map(|n| Value::Numeric(n.to_string()))),

        ColumnData::String(v) => opt(v.as_ref().map(|s| Value::Text(s.to_string()))),
        ColumnData::Guid(v) => opt(v.map(Value::Uuid)),
        ColumnData::Binary(v) => opt(v.as_ref().map(|b| Value::Bytes(b.to_vec()))),

        // XML has no dedicated variant in the value model; its text is what a
        // user wants to see anyway.
        ColumnData::Xml(v) => opt(v.as_ref().map(|x| Value::Text(x.to_string()))),

        // The temporal variants are raw TDS encodings (day counts, tick counts)
        // whose interpretation depends on the type's scale. tiberius exposes
        // chrono conversions rather than the arithmetic, so these are rendered
        // through Debug rather than being decoded incorrectly by hand.
        ColumnData::Date(v) => temporal(v.is_some(), data, "date"),
        ColumnData::Time(v) => temporal(v.is_some(), data, "time"),
        ColumnData::SmallDateTime(v) => temporal(v.is_some(), data, "smalldatetime"),
        ColumnData::DateTime(v) => temporal(v.is_some(), data, "datetime"),
        ColumnData::DateTime2(v) => temporal(v.is_some(), data, "datetime2"),
        ColumnData::DateTimeOffset(v) => temporal(v.is_some(), data, "datetimeoffset"),
    }
}

fn opt(value: Option<Value>) -> Value {
    value.unwrap_or(Value::Null)
}

/// Render a temporal value, preserving NULL.
fn temporal(present: bool, data: &ColumnData<'static>, type_name: &str) -> Value {
    if !present {
        return Value::Null;
    }
    // Chrono conversion goes through tiberius' FromSql impls, which need a
    // target type; without provenance for the exact scale, the honest rendering
    // is the value tiberius already parsed.
    match chrono_from(data) {
        Some(v) => v,
        None => Value::Unsupported {
            type_name: type_name.to_string(),
            raw: format!("{data:?}"),
        },
    }
}

/// Convert via tiberius' chrono support where a direct mapping exists.
fn chrono_from(data: &ColumnData<'static>) -> Option<Value> {
    use tiberius::time::chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use tiberius::FromSql;

    match data {
        ColumnData::Date(_) => NaiveDate::from_sql(data).ok().flatten().map(Value::Date),
        ColumnData::Time(_) => NaiveTime::from_sql(data).ok().flatten().map(Value::Time),
        ColumnData::SmallDateTime(_) | ColumnData::DateTime(_) | ColumnData::DateTime2(_) => {
            NaiveDateTime::from_sql(data)
                .ok()
                .flatten()
                .map(Value::DateTime)
        }
        // DATETIMEOFFSET is a genuine instant; the others are wall-clock
        // readings and must not be promoted to one. The wire form carries a
        // fixed offset, which is then normalized to UTC.
        ColumnData::DateTimeOffset(_) => {
            use tiberius::time::chrono::{DateTime, FixedOffset, Utc};
            let dt: Option<DateTime<FixedOffset>> = DateTime::from_sql(data).ok().flatten();
            dt.map(|d| Value::TimestampTz(d.with_timezone(&Utc)))
        }
        _ => None,
    }
}

/// Display name for a column type.
pub fn type_name(ty: ColumnType) -> String {
    format!("{ty:?}").to_lowercase()
}

/// Render a [`Value`] as a T-SQL literal for an inline edit.
///
/// tiberius' parameter binding needs a compile-time type per placeholder, which
/// a dynamic client does not have, so edits are built as literals. Every string
/// is escaped by doubling quotes and binary is emitted as a `0x` literal, so a
/// value containing a quote cannot terminate the literal early.
pub fn literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => i32::from(*b).to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        // Unquoted so the server parses it as an exact numeric, keeping the
        // digits rather than coercing through a float.
        Value::Numeric(s) if is_numeric_literal(s) => s.clone(),
        Value::Bytes(b) => {
            format!(
                "0x{}",
                b.iter().map(|x| format!("{x:02x}")).collect::<String>()
            )
        }
        // The `N` prefix makes it an nvarchar literal, so non-ASCII survives.
        // Everything else — text, dates, JSON, an out-of-shape numeric — goes
        // through here quoted, and the server does the conversion.
        other => format!("N'{}'", quote(&other.to_string())),
    }
}

/// Reject anything that is not a plain decimal number, so a malformed value
/// cannot be injected unquoted.
fn is_numeric_literal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit() || c == '.')
        && body.matches('.').count() <= 1
}

fn quote(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_stays_null_for_every_variant() {
        assert_eq!(decode(&ColumnData::I32(None)), Value::Null);
        assert_eq!(decode(&ColumnData::String(None)), Value::Null);
        assert_eq!(decode(&ColumnData::Bit(None)), Value::Null);
        assert_eq!(decode(&ColumnData::Numeric(None)), Value::Null);
    }

    #[test]
    fn integers_widen_to_i64() {
        assert_eq!(decode(&ColumnData::U8(Some(7))), Value::Int(7));
        assert_eq!(decode(&ColumnData::I16(Some(7))), Value::Int(7));
        assert_eq!(decode(&ColumnData::I32(Some(7))), Value::Int(7));
        assert_eq!(decode(&ColumnData::I64(Some(7))), Value::Int(7));
    }

    #[test]
    fn bits_decode_as_booleans() {
        assert_eq!(decode(&ColumnData::Bit(Some(true))), Value::Bool(true));
        assert_eq!(decode(&ColumnData::Bit(Some(false))), Value::Bool(false));
    }

    #[test]
    fn strings_decode_as_text() {
        let data = ColumnData::String(Some("héllo".into()));
        assert_eq!(decode(&data), Value::Text("héllo".into()));
    }

    #[test]
    fn string_literals_escape_embedded_quotes() {
        // Without doubling, a value containing a quote would end the literal and
        // the rest would be parsed as SQL.
        assert_eq!(
            literal(&Value::Text("O'Brien".into())),
            "N'O''Brien'".to_string()
        );
        assert_eq!(
            literal(&Value::Text("'; DROP TABLE t --".into())),
            "N'''; DROP TABLE t --'".to_string()
        );
    }

    #[test]
    fn exact_numerics_are_emitted_unquoted_and_intact() {
        let s = "12345678901234567890.1234567890";
        assert_eq!(literal(&Value::Numeric(s.into())), s.to_string());
        assert_eq!(literal(&Value::Numeric("-1.5".into())), "-1.5".to_string());
    }

    #[test]
    fn a_malformed_numeric_is_quoted_rather_than_injected() {
        // If it does not look like a number it must not be emitted unquoted,
        // whatever produced it.
        let sneaky = "1; DROP TABLE t";
        let out = literal(&Value::Numeric(sneaky.into()));
        assert!(out.starts_with("N'"), "got {out}");
        assert!(!out.contains("; DROP TABLE t;"), "got {out}");
    }

    #[test]
    fn binary_uses_a_hex_literal() {
        assert_eq!(
            literal(&Value::Bytes(vec![0xde, 0xad])),
            "0xdead".to_string()
        );
    }

    #[test]
    fn null_literal_is_the_keyword_not_a_string() {
        assert_eq!(literal(&Value::Null), "NULL".to_string());
    }
}
