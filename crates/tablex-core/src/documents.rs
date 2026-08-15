//! Projecting documents onto columns and rows.
//!
//! A document store returns objects that need not agree with each other about
//! anything: a field present in one document may be absent from the next, hold
//! a number here and a string there, or contain a whole nested object. A grid
//! needs columns. This is where that mapping is decided, so that it is decided
//! once and can be tested without a server.
//!
//! Three decisions carry the weight.
//!
//! **Absent is not null.** `{}` and `{"a": null}` are different documents, and
//! a store that distinguishes them is not served by a grid that does not. The
//! projection returns `None` for a field the document did not have and
//! `Some(Value::Null)` for one it had and set to null. What to draw for each is
//! the driver's decision; conflating them here would take it away.
//!
//! **Nested values stay nested.** Flattening `address.city` into a column reads
//! well for one shape of document and explodes for every other — arrays of
//! objects have no sensible column name at all. A nested value becomes
//! [`Value::Json`], which the grid already renders and pretty-prints.
//!
//! **Extended JSON is decoded, not displayed.** `{"$numberDecimal": "0.1"}` is
//! an exact decimal, and rendering it as the object it is written as — or worse,
//! as an f64 — would lose in the last step what the store took care to keep.

use crate::value::Value;
use serde_json::{Map, Value as Json};

/// Documents laid out as a grid.
#[derive(Debug, Clone, Default)]
pub struct Projection {
    pub columns: Vec<String>,
    /// One row per document. `None` means the document had no such field.
    pub rows: Vec<Vec<Option<Value>>>,
    /// Columns beyond the cap, which were dropped. Non-zero here means the
    /// grid is showing less than the query returned, and saying so is the
    /// difference between a limit and a lie.
    pub dropped_columns: usize,
}

/// The identifier every document has, and the one worth seeing first.
const ID: &str = "_id";

/// Project documents onto a grid, keeping at most `max_columns` of them.
pub fn project(documents: &[Json], max_columns: usize) -> Projection {
    // Union of every key, in the order first seen — which for a collection
    // written by one application is the order its documents declare, and so
    // the order somebody expects to read them in.
    let mut columns: Vec<String> = Vec::new();
    for document in documents {
        match document.as_object() {
            Some(object) => {
                for key in object.keys() {
                    if !columns.iter().any(|c| c == key) {
                        columns.push(key.clone());
                    }
                }
            }
            // An aggregation can return a bare value. It is still a result, and
            // one column holding it beats no result at all.
            None => {
                if !columns.iter().any(|c| c == "value") {
                    columns.push("value".into());
                }
            }
        }
    }

    // `_id` is present on every document and is the key; anywhere but first is
    // the wrong place for it.
    if let Some(position) = columns.iter().position(|c| c == ID) {
        let id = columns.remove(position);
        columns.insert(0, id);
    }

    let dropped_columns = columns.len().saturating_sub(max_columns);
    columns.truncate(max_columns);

    let rows = documents
        .iter()
        .map(|document| match document.as_object() {
            Some(object) => columns
                .iter()
                .map(|column| object.get(column).map(decode))
                .collect(),
            None => columns
                .iter()
                .map(|column| (column == "value").then(|| decode(document)))
                .collect(),
        })
        .collect();

    Projection {
        columns,
        rows,
        dropped_columns,
    }
}

/// One JSON value as the grid's value model.
pub fn decode(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::UInt(u)
            } else {
                // Everything else JSON can hold is a double, and saying so is
                // more honest than promoting it to an exact type it is not.
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Json::String(s) => Value::Text(s.clone()),
        Json::Array(_) => Value::Json(json.clone()),
        Json::Object(object) => decode_object(object, json),
    }
}

/// An object, after checking whether it is really a wrapped scalar.
///
/// MongoDB's extended JSON writes types that JSON has no syntax for as
/// single-key objects. Left alone they render as `{"$oid": "…"}`, which is the
/// encoding rather than the value.
fn decode_object(object: &Map<String, Json>, whole: &Json) -> Value {
    if object.len() != 1 {
        return Value::Json(whole.clone());
    }
    let (key, inner) = object.iter().next().expect("length checked");

    match key.as_str() {
        // An ObjectId is 24 hex characters; it is an identifier, not a number.
        "$oid" => inner.as_str().map(|s| Value::Text(s.into())),

        // Exact by construction, and the whole reason the type exists. Reading
        // it as a float here would lose in the last step what the store took
        // care to keep.
        "$numberDecimal" => inner.as_str().map(|s| Value::Numeric(s.into())),

        // Written as a string precisely because it does not survive JSON's
        // number type.
        "$numberLong" => inner
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map(Value::Int),
        "$numberInt" => inner
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map(Value::Int),
        "$numberDouble" => inner
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .map(Value::Float),

        // Milliseconds since the epoch, or an ISO-8601 string in the relaxed
        // form. Both name an instant, so both become one.
        "$date" => decode_date(inner),

        _ => None,
    }
    .unwrap_or_else(|| Value::Json(whole.clone()))
}

fn decode_date(inner: &Json) -> Option<Value> {
    use chrono::TimeZone;

    if let Some(millis) = inner.as_i64() {
        return chrono::Utc
            .timestamp_millis_opt(millis)
            .single()
            .map(Value::TimestampTz);
    }
    // The relaxed form nests it again: {"$date": {"$numberLong": "…"}}.
    if let Some(object) = inner.as_object() {
        if let Some(text) = object.get("$numberLong").and_then(Json::as_str) {
            return text
                .parse::<i64>()
                .ok()
                .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single())
                .map(Value::TimestampTz);
        }
    }
    inner
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| Value::TimestampTz(dt.with_timezone(&chrono::Utc)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn projected(documents: Vec<Json>) -> Projection {
        project(&documents, 100)
    }

    #[test]
    fn columns_are_the_union_of_every_documents_keys() {
        let p = projected(vec![
            json!({"name": "a", "age": 1}),
            json!({"name": "b", "email": "b@example.com"}),
        ]);
        assert_eq!(p.columns, vec!["name", "age", "email"]);
        assert_eq!(p.rows.len(), 2);
    }

    #[test]
    fn absent_is_not_null() {
        // `{}` and `{"a": null}` are different documents, and a store that
        // distinguishes them is not served by a grid that does not.
        let p = projected(vec![json!({"a": null}), json!({"b": 1})]);
        assert_eq!(p.columns, vec!["a", "b"]);
        assert_eq!(p.rows[0][0], Some(Value::Null));
        assert_eq!(p.rows[0][1], None, "b is absent, not null");
        assert_eq!(p.rows[1][0], None, "a is absent, not null");
        assert_eq!(p.rows[1][1], Some(Value::Int(1)));
    }

    #[test]
    fn the_id_comes_first_wherever_it_was_written() {
        let p = projected(vec![json!({"name": "a", "_id": {"$oid": "abc"}})]);
        assert_eq!(p.columns, vec!["_id", "name"]);
        assert_eq!(p.rows[0][0], Some(Value::Text("abc".into())));
    }

    #[test]
    fn a_nested_object_stays_nested() {
        // Flattening reads well for one shape of document and explodes for
        // every other; the grid already pretty-prints JSON.
        let p = projected(vec![
            json!({"address": {"city": "Nairobi", "zip": "00100"}}),
        ]);
        assert!(matches!(p.rows[0][0], Some(Value::Json(_))));
        // And not as a column per leaf.
        assert_eq!(p.columns, vec!["address"]);
    }

    #[test]
    fn an_array_stays_an_array() {
        let p = projected(vec![json!({"tags": ["a", "b"]})]);
        assert!(matches!(p.rows[0][0], Some(Value::Json(_))));
    }

    #[test]
    fn an_exact_decimal_survives_as_an_exact_decimal() {
        // The whole reason $numberDecimal exists; reading it as a float would
        // lose in the last step what the store took care to keep.
        let p = projected(vec![
            json!({"total": {"$numberDecimal": "123456789.123456789"}}),
        ]);
        assert_eq!(
            p.rows[0][0],
            Some(Value::Numeric("123456789.123456789".into()))
        );
    }

    #[test]
    fn a_long_written_as_a_string_comes_back_as_an_integer() {
        // Written as a string precisely because it does not survive JSON's
        // number type.
        let p = projected(vec![json!({"n": {"$numberLong": "9223372036854775807"}})]);
        assert_eq!(p.rows[0][0], Some(Value::Int(i64::MAX)));
    }

    #[test]
    fn a_date_is_an_instant_in_either_spelling() {
        let strict = projected(vec![json!({"at": {"$date": 1_700_000_000_000i64}})]);
        let relaxed = projected(vec![
            json!({"at": {"$date": {"$numberLong": "1700000000000"}}}),
        ]);
        let iso = projected(vec![json!({"at": {"$date": "2023-11-14T22:13:20Z"}})]);

        assert!(matches!(strict.rows[0][0], Some(Value::TimestampTz(_))));
        assert_eq!(strict.rows[0][0], relaxed.rows[0][0]);
        assert_eq!(strict.rows[0][0], iso.rows[0][0]);
    }

    #[test]
    fn an_unrecognized_dollar_key_is_left_as_json() {
        // Guessing at an encoding this does not know would be worse than
        // showing the document exactly as it is.
        let p = projected(vec![json!({"x": {"$whatever": 1}})]);
        assert!(matches!(p.rows[0][0], Some(Value::Json(_))));
    }

    #[test]
    fn a_two_key_object_is_a_document_not_a_wrapped_scalar() {
        // {"$oid": …} is an encoding; {"$oid": …, "other": …} is a document
        // that happens to have an odd field name.
        let p = projected(vec![json!({"x": {"$oid": "abc", "other": 1}})]);
        assert!(matches!(p.rows[0][0], Some(Value::Json(_))));
    }

    #[test]
    fn a_bare_value_still_produces_a_result() {
        // An aggregation can return one. A row is better than an error.
        let p = projected(vec![json!(42), json!("text")]);
        assert_eq!(p.columns, vec!["value"]);
        assert_eq!(p.rows[0][0], Some(Value::Int(42)));
        assert_eq!(p.rows[1][0], Some(Value::Text("text".into())));
    }

    #[test]
    fn the_column_cap_is_reported_rather_than_silently_applied() {
        // Showing less than the query returned without saying so is the
        // difference between a limit and a lie.
        let wide: Map<String, Json> = (0..10).map(|i| (format!("f{i}"), json!(i))).collect();
        let p = project(&[Json::Object(wide)], 4);
        assert_eq!(p.columns.len(), 4);
        assert_eq!(p.dropped_columns, 6);
        assert_eq!(p.rows[0].len(), 4);
    }

    #[test]
    fn no_documents_is_no_columns_rather_than_a_panic() {
        let p = projected(Vec::new());
        assert!(p.columns.is_empty());
        assert!(p.rows.is_empty());
    }

    #[test]
    fn every_row_is_as_wide_as_the_column_list() {
        // A short row would misalign the grid from that point on.
        let p = projected(vec![json!({"a": 1}), json!({"b": 2, "c": 3}), json!({})]);
        assert_eq!(p.columns.len(), 3);
        for row in &p.rows {
            assert_eq!(row.len(), 3);
        }
    }
}
