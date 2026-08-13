//! Dynamic decoding of PostgreSQL binary values.
//!
//! A database client cannot use `FromSql` the normal way: the target Rust type
//! is unknown at compile time, and the set of types on the server is open-ended
//! (extensions add their own). So rows are fetched as raw bytes and dispatched on
//! the runtime `Type`, with anything unrecognized surfaced as
//! [`Value::Unsupported`] rather than failing the query.

use super::numeric;
// `ArrayValues` iterates fallibly (each element can fail to decode independently),
// so the trait must be in scope to call `next`.
use postgres_protocol::types as pg;
use tablepro_core::Value;
use tokio_postgres::fallible_iterator::FallibleIterator;
use tokio_postgres::types::{FromSql, Kind, Type};

/// Postgres counts time from 2000-01-01, not 1970-01-01.
const PG_EPOCH_DAYS_FROM_UNIX: i64 = 10_957;

/// Captures a column's bytes without interpreting them, so we can dispatch on the
/// runtime type ourselves.
pub struct Raw(pub Option<Vec<u8>>);

impl<'a> FromSql<'a> for Raw {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Raw(Some(raw.to_vec())))
    }

    fn from_sql_null(_ty: &Type) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Raw(None))
    }

    /// Accept everything: the point is to receive bytes for types no
    /// compile-time impl exists for.
    fn accepts(_ty: &Type) -> bool {
        true
    }
}

/// Decode one column value.
pub fn decode(raw: Option<&[u8]>, ty: &Type) -> Value {
    let Some(bytes) = raw else {
        return Value::Null;
    };
    decode_bytes(bytes, ty).unwrap_or_else(|| unsupported(bytes, ty))
}

fn decode_bytes(b: &[u8], ty: &Type) -> Option<Value> {
    // Arrays are a wrapper around any element type, including ones we do not
    // otherwise handle, so they are dispatched before the scalar table.
    if let Kind::Array(inner) = ty.kind() {
        return decode_array(b, inner);
    }

    // A domain is a constrained alias; decode as its base type.
    if let Kind::Domain(inner) = ty.kind() {
        return decode_bytes(b, inner);
    }

    // Enums arrive as their label, which is exactly what should be displayed.
    if let Kind::Enum(_) = ty.kind() {
        return pg::text_from_sql(b)
            .ok()
            .map(|s| Value::Text(s.to_string()));
    }

    Some(match *ty {
        Type::BOOL => Value::Bool(pg::bool_from_sql(b).ok()?),

        Type::INT2 => Value::Int(pg::int2_from_sql(b).ok()? as i64),
        Type::INT4 => Value::Int(pg::int4_from_sql(b).ok()? as i64),
        Type::INT8 => Value::Int(pg::int8_from_sql(b).ok()?),
        Type::OID => Value::Int(pg::oid_from_sql(b).ok()? as i64),

        Type::FLOAT4 => Value::Float(pg::float4_from_sql(b).ok()? as f64),
        Type::FLOAT8 => Value::Float(pg::float8_from_sql(b).ok()?),

        // Exact numerics are decoded digit-for-digit — see the `numeric` module.
        Type::NUMERIC => Value::Numeric(numeric::decode(b)?),

        // `money` is a scaled int64 whose scale depends on the server's lc_monetary,
        // so it is reported as exact-numeric text at the default scale of 2 rather
        // than guessed at.
        Type::MONEY => {
            let cents = pg::int8_from_sql(b).ok()?;
            Value::Numeric(format!("{}.{:02}", cents / 100, (cents % 100).abs()))
        }

        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
            Value::Text(pg::text_from_sql(b).ok()?.to_string())
        }

        Type::CHAR => Value::Int(b.first().copied().unwrap_or(0) as i64),

        Type::BYTEA => Value::Bytes(pg::bytea_from_sql(b).to_vec()),

        Type::UUID => Value::Uuid(uuid::Uuid::from_bytes(pg::uuid_from_sql(b).ok()?)),

        // `json` is raw text on the wire. `jsonb` prefixes a one-byte format
        // version; version 1 is the only one defined, and a future version would
        // have a layout we cannot assume, so anything else degrades rather than
        // being misparsed.
        Type::JSON => Value::Json(serde_json::from_slice(b).ok()?),
        Type::JSONB => match b.split_first() {
            Some((1, rest)) => Value::Json(serde_json::from_slice(rest).ok()?),
            _ => return None,
        },

        Type::DATE => {
            let days = pg::date_from_sql(b).ok()? as i64;
            Value::Date(from_pg_days(days)?)
        }

        Type::TIME => {
            let micros = pg::time_from_sql(b).ok()?;
            Value::Time(chrono::NaiveTime::from_num_seconds_from_midnight_opt(
                (micros / 1_000_000) as u32,
                ((micros % 1_000_000) * 1_000) as u32,
            )?)
        }

        // A wall-clock reading with no zone. Deliberately *not* promoted to an
        // instant: the server does not know which zone it was recorded in, and
        // inventing one here would shift the displayed value.
        Type::TIMESTAMP => Value::DateTime(from_pg_micros(pg::timestamp_from_sql(b).ok()?)?),

        // Genuinely an instant; the server stores it in UTC.
        Type::TIMESTAMPTZ => Value::TimestampTz(chrono::DateTime::from_naive_utc_and_offset(
            from_pg_micros(pg::timestamp_from_sql(b).ok()?)?,
            chrono::Utc,
        )),

        Type::INTERVAL => decode_interval(b)?,

        _ => return None,
    })
}

/// `interval` is 16 bytes: microseconds, days, months. The units are kept
/// separate because they are not interconvertible — months vary in length and
/// days vary across DST transitions.
fn decode_interval(b: &[u8]) -> Option<Value> {
    if b.len() < 16 {
        return None;
    }
    Some(Value::Interval {
        micros: i64::from_be_bytes(b[0..8].try_into().ok()?),
        days: i32::from_be_bytes(b[8..12].try_into().ok()?),
        months: i32::from_be_bytes(b[12..16].try_into().ok()?),
    })
}

fn decode_array(b: &[u8], inner: &Type) -> Option<Value> {
    let array = pg::array_from_sql(b).ok()?;
    let mut values = Vec::new();
    let mut iter = array.values();
    while let Ok(Some(element)) = iter.next() {
        values.push(match element {
            Some(bytes) => decode_bytes(bytes, inner).unwrap_or_else(|| unsupported(bytes, inner)),
            None => Value::Null,
        });
    }
    Some(Value::Array(values))
}

fn from_pg_days(days_from_2000: i64) -> Option<chrono::NaiveDate> {
    chrono::DateTime::from_timestamp((days_from_2000 + PG_EPOCH_DAYS_FROM_UNIX) * 86_400, 0)
        .map(|dt| dt.date_naive())
}

fn from_pg_micros(micros_from_2000: i64) -> Option<chrono::NaiveDateTime> {
    let unix_micros = micros_from_2000 + PG_EPOCH_DAYS_FROM_UNIX * 86_400 * 1_000_000;
    chrono::DateTime::from_timestamp_micros(unix_micros).map(|dt| dt.naive_utc())
}

/// A type this build does not understand. Rendered as hex or as text if the
/// bytes happen to be valid UTF-8, and always labelled with the server's own
/// type name so the user can see what it is.
fn unsupported(bytes: &[u8], ty: &Type) -> Value {
    let raw = match std::str::from_utf8(bytes) {
        Ok(s) if s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t') => s.to_string(),
        _ => bytes
            .iter()
            .take(64)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    };
    Value::Unsupported {
        type_name: ty.name().to_string(),
        raw,
    }
}

/// Render a [`Value`] as a literal parameter for an inline edit.
///
/// Values are always sent as text with an explicit cast, letting the server do
/// the conversion. That avoids reimplementing binary encoders for every type and
/// keeps exact numerics exact — the digits go out the way they came in.
pub fn to_param(v: &Value) -> Option<String> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Numeric(s) => s.clone(),
        Value::Text(s) => s.clone(),
        Value::Bytes(b) => format!(
            "\\x{}",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ),
        Value::Uuid(u) => u.to_string(),
        Value::Date(d) => d.to_string(),
        Value::Time(t) => t.to_string(),
        Value::DateTime(dt) => dt.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
        Value::TimestampTz(dt) => dt.to_rfc3339(),
        Value::Json(j) => j.to_string(),
        Value::Interval {
            months,
            days,
            micros,
        } => format!("{months} months {days} days {micros} microseconds"),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(|i| match to_param(i) {
                    Some(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
                    None => "NULL".to_string(),
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{inner}}}")
        }
        Value::Unsupported { raw, .. } => raw.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_decodes_to_null_regardless_of_type() {
        assert_eq!(decode(None, &Type::INT4), Value::Null);
        assert_eq!(decode(None, &Type::TEXT), Value::Null);
    }

    #[test]
    fn integers_widen_to_i64() {
        assert_eq!(
            decode(Some(&7i16.to_be_bytes()), &Type::INT2),
            Value::Int(7)
        );
        assert_eq!(
            decode(Some(&7i32.to_be_bytes()), &Type::INT4),
            Value::Int(7)
        );
        assert_eq!(
            decode(Some(&7i64.to_be_bytes()), &Type::INT8),
            Value::Int(7)
        );
    }

    #[test]
    fn booleans_decode() {
        assert_eq!(decode(Some(&[1]), &Type::BOOL), Value::Bool(true));
        assert_eq!(decode(Some(&[0]), &Type::BOOL), Value::Bool(false));
    }

    #[test]
    fn text_decodes_as_utf8() {
        assert_eq!(
            decode(Some("héllo".as_bytes()), &Type::TEXT),
            Value::Text("héllo".into())
        );
    }

    #[test]
    fn numeric_is_exact() {
        // ndigits=2, weight=0, sign=+, dscale=4, digits=[1234, 5678] → 1234.5678
        let mut b = Vec::new();
        b.extend_from_slice(&2i16.to_be_bytes());
        b.extend_from_slice(&0i16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&4u16.to_be_bytes());
        b.extend_from_slice(&1234i16.to_be_bytes());
        b.extend_from_slice(&5678i16.to_be_bytes());
        assert_eq!(
            decode(Some(&b), &Type::NUMERIC),
            Value::Numeric("1234.5678".into())
        );
    }

    #[test]
    fn timestamp_without_zone_stays_a_wall_clock_reading() {
        // 0 microseconds from the Postgres epoch = 2000-01-01 00:00:00.
        let v = decode(Some(&0i64.to_be_bytes()), &Type::TIMESTAMP);
        match v {
            Value::DateTime(dt) => assert_eq!(dt.to_string(), "2000-01-01 00:00:00"),
            other => panic!("expected DateTime, got {other:?}"),
        }
    }

    #[test]
    fn timestamptz_is_an_instant_in_utc() {
        let v = decode(Some(&0i64.to_be_bytes()), &Type::TIMESTAMPTZ);
        match v {
            Value::TimestampTz(dt) => assert_eq!(dt.timestamp(), 946_684_800),
            other => panic!("expected TimestampTz, got {other:?}"),
        }
    }

    #[test]
    fn date_uses_the_postgres_epoch() {
        let v = decode(Some(&0i32.to_be_bytes()), &Type::DATE);
        match v {
            Value::Date(d) => assert_eq!(d.to_string(), "2000-01-01"),
            other => panic!("expected Date, got {other:?}"),
        }
    }

    #[test]
    fn interval_units_are_kept_separate() {
        // 1 month, 2 days, 3 microseconds. Collapsing these into one unit would
        // be wrong: months and days are not fixed-length.
        let mut b = Vec::new();
        b.extend_from_slice(&3i64.to_be_bytes());
        b.extend_from_slice(&2i32.to_be_bytes());
        b.extend_from_slice(&1i32.to_be_bytes());
        assert_eq!(
            decode(Some(&b), &Type::INTERVAL),
            Value::Interval {
                months: 1,
                days: 2,
                micros: 3
            }
        );
    }

    #[test]
    fn unknown_types_are_labelled_not_dropped() {
        // POINT is not in the scalar table; it must still be visible.
        let v = decode(Some(&[0x01, 0x02]), &Type::POINT);
        match v {
            Value::Unsupported { type_name, raw } => {
                assert_eq!(type_name, "point");
                assert_eq!(raw, "0102");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn truncated_buffers_degrade_instead_of_panicking() {
        // A short INT4 buffer must not index out of bounds.
        let v = decode(Some(&[0x00]), &Type::INT4);
        assert!(matches!(v, Value::Unsupported { .. }), "got {v:?}");
    }

    #[test]
    fn exact_numerics_round_trip_through_to_param() {
        let s = "123456789012345678901234567890.12345";
        assert_eq!(to_param(&Value::Numeric(s.into())).as_deref(), Some(s));
    }

    #[test]
    fn null_has_no_parameter_text() {
        // The caller must bind a real NULL, not the string "NULL".
        assert_eq!(to_param(&Value::Null), None);
    }

    #[test]
    fn bytea_parameters_use_hex_escape_format() {
        assert_eq!(
            to_param(&Value::Bytes(vec![0xde, 0xad])).as_deref(),
            Some("\\xdead")
        );
    }
}
