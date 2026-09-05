//! Guessing a table from a file: which type each column should be, and what
//! to call it.
//!
//! An import into a table that does not exist yet has to invent one, and the
//! only evidence is the values. The guesses here are deliberately cautious. A
//! column is an integer only if every non-empty value is one; a single `n/a`
//! makes it text, because a type that refuses the file's own rows is worse
//! than one that is wider than it needed to be. The person importing sees the
//! guess beside the values and can change it before anything is created.

use serde::{Deserialize, Serialize};

/// What a column's values look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Integer,
    /// A number with a fractional part. `scale` is the most digits seen after
    /// the point, so an exact type can be sized to hold every value.
    Number {
        scale: u8,
    },
    Boolean,
    /// `YYYY-MM-DD`.
    Date,
    /// A date with a time of day.
    DateTime,
}

/// The kind that holds every value in `values`, ignoring empty ones.
///
/// An all-empty column is text: nothing is known about it, and text is the
/// one type that can take whatever turns up later.
pub fn infer_column(values: impl IntoIterator<Item = impl AsRef<str>>) -> Kind {
    let mut kind: Option<Kind> = None;
    for value in values {
        let v = value.as_ref().trim();
        if v.is_empty() {
            continue;
        }
        let this = kind_of(v);
        kind = Some(match kind {
            None => this,
            Some(so_far) => merge(so_far, this),
        });
        if kind == Some(Kind::Text) {
            break;
        }
    }
    kind.unwrap_or(Kind::Text)
}

/// A kind for each column, over rows that may be ragged.
pub fn infer_columns(rows: &[Vec<String>], columns: usize) -> Vec<Kind> {
    (0..columns)
        .map(|c| {
            infer_column(
                rows.iter()
                    .map(|row| row.get(c).map(String::as_str).unwrap_or("")),
            )
        })
        .collect()
}

fn kind_of(v: &str) -> Kind {
    if is_integer(v) {
        return Kind::Integer;
    }
    if let Some(scale) = decimal_scale(v) {
        return Kind::Number { scale };
    }
    if matches!(
        v.to_ascii_lowercase().as_str(),
        "true" | "false" | "t" | "f" | "yes" | "no" | "y" | "n"
    ) {
        return Kind::Boolean;
    }
    if is_date(v) {
        return Kind::Date;
    }
    if is_datetime(v) {
        return Kind::DateTime;
    }
    Kind::Text
}

/// The narrowest kind that holds both.
fn merge(a: Kind, b: Kind) -> Kind {
    use Kind::*;
    match (a, b) {
        (x, y) if x == y => x,
        (Integer, Number { scale }) | (Number { scale }, Integer) => Number { scale },
        (Number { scale: s1 }, Number { scale: s2 }) => Number { scale: s1.max(s2) },
        // A date is a datetime with no time of day.
        (Date, DateTime) | (DateTime, Date) => DateTime,
        _ => Text,
    }
}

fn is_integer(v: &str) -> bool {
    let body = v.strip_prefix(['-', '+']).unwrap_or(v);
    !body.is_empty() && body.len() <= 18 && body.bytes().all(|b| b.is_ascii_digit())
}

/// Digits after the point, for a plain decimal such as `-12.50`.
fn decimal_scale(v: &str) -> Option<u8> {
    let body = v.strip_prefix(['-', '+']).unwrap_or(v);
    let (whole, frac) = body.split_once('.')?;
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(frac.len().min(u8::MAX as usize) as u8)
}

fn is_date(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| b[i].is_ascii_digit())
}

fn is_datetime(v: &str) -> bool {
    // `YYYY-MM-DD HH:MM[:SS[.fff]]`, with `T` allowed between, and an
    // optional zone: what every engine here prints and what a spreadsheet
    // hands over.
    let (date, time) = match v.split_once([' ', 'T']) {
        Some(parts) => parts,
        None => return false,
    };
    if !is_date(date) {
        return false;
    }
    let time = time
        .trim_end_matches('Z')
        .split(['+', '-'])
        .next()
        .unwrap_or("");
    let mut parts = time.split(':');
    let hh = parts.next().unwrap_or("");
    let mm = parts.next().unwrap_or("");
    let ss = parts.next().unwrap_or("00");
    let two = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
    let seconds = ss.split_once('.').map_or(ss, |(s, frac)| {
        if frac.bytes().all(|b| b.is_ascii_digit()) {
            s
        } else {
            "x"
        }
    });
    two(hh) && two(mm) && two(seconds) && parts.next().is_none()
}

/// The engine's own spelling of a kind.
///
/// Exact types for numbers wherever the engine has one, sized from the widest
/// scale seen with room to spare; text and timestamps in the spelling each
/// engine prefers rather than a lowest common denominator that every one of
/// them accepts grudgingly.
pub fn type_name(kind: Kind, driver: &str) -> String {
    let scale = |s: u8| (u32::from(s) + 2).min(18);
    match driver {
        "mysql" | "mariadb" => match kind {
            Kind::Text => "TEXT".into(),
            Kind::Integer => "BIGINT".into(),
            Kind::Number { scale: s } => format!("DECIMAL(38, {})", scale(s)),
            Kind::Boolean => "BOOLEAN".into(),
            Kind::Date => "DATE".into(),
            Kind::DateTime => "DATETIME".into(),
        },
        "mssql" => match kind {
            Kind::Text => "NVARCHAR(MAX)".into(),
            Kind::Integer => "BIGINT".into(),
            Kind::Number { scale: s } => format!("DECIMAL(38, {})", scale(s)),
            Kind::Boolean => "BIT".into(),
            Kind::Date => "DATE".into(),
            Kind::DateTime => "DATETIME2".into(),
        },
        "oracle" => match kind {
            Kind::Text => "VARCHAR2(4000)".into(),
            Kind::Integer => "NUMBER(19)".into(),
            Kind::Number { scale: s } => format!("NUMBER(38, {})", scale(s)),
            Kind::Boolean => "NUMBER(1)".into(),
            Kind::Date => "DATE".into(),
            Kind::DateTime => "TIMESTAMP".into(),
        },
        "sqlite" => match kind {
            Kind::Text => "TEXT".into(),
            Kind::Integer => "INTEGER".into(),
            // SQLite keeps what it is given; the name only sets the affinity.
            Kind::Number { .. } => "NUMERIC".into(),
            Kind::Boolean => "BOOLEAN".into(),
            Kind::Date => "DATE".into(),
            Kind::DateTime => "DATETIME".into(),
        },
        "clickhouse" => match kind {
            Kind::Text => "String".into(),
            Kind::Integer => "Int64".into(),
            Kind::Number { scale: s } => format!("Decimal(38, {})", scale(s)),
            Kind::Boolean => "Bool".into(),
            Kind::Date => "Date32".into(),
            Kind::DateTime => "DateTime".into(),
        },
        _ => match kind {
            Kind::Text => "text".into(),
            Kind::Integer => "bigint".into(),
            Kind::Number { .. } => "numeric".into(),
            Kind::Boolean => "boolean".into(),
            Kind::Date => "date".into(),
            Kind::DateTime => "timestamp".into(),
        },
    }
}

/// Column names from a header row, made usable and unique.
///
/// Trimmed; an empty header becomes `column_N`; a repeat is numbered. What is
/// left is the header as written, spaces and capitals included — the engine
/// quotes it, and renaming somebody's columns for them is not this module's
/// business.
pub fn column_names(header: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(header.len());
    for (i, raw) in header.iter().enumerate() {
        let base = raw.trim();
        let base = if base.is_empty() {
            format!("column_{}", i + 1)
        } else {
            base.to_string()
        };
        let mut name = base.clone();
        let mut n = 2;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            name = format!("{base}_{n}");
            n += 1;
        }
        out.push(name);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(values: &[&str]) -> Kind {
        infer_column(values.iter().copied())
    }

    #[test]
    fn integers_numbers_and_the_widening_between_them() {
        assert_eq!(col(&["1", "-2", " 300 "]), Kind::Integer);
        assert_eq!(col(&["1.5", "2.25"]), Kind::Number { scale: 2 });
        // One decimal among integers makes the column a number, sized to it.
        assert_eq!(col(&["1", "2.125", "3"]), Kind::Number { scale: 3 });
    }

    #[test]
    fn one_odd_value_makes_the_column_text() {
        assert_eq!(col(&["1", "2", "n/a"]), Kind::Text);
        assert_eq!(col(&["true", "maybe"]), Kind::Text);
    }

    #[test]
    fn empties_say_nothing() {
        assert_eq!(col(&["", " ", "7"]), Kind::Integer);
        assert_eq!(col(&["", ""]), Kind::Text);
        assert_eq!(col(&[]), Kind::Text);
    }

    #[test]
    fn booleans_dates_and_times() {
        assert_eq!(col(&["true", "FALSE", "yes", "n"]), Kind::Boolean);
        assert_eq!(col(&["2024-07-21", "2025-01-01"]), Kind::Date);
        assert_eq!(
            col(&["2024-07-21 00:00:00", "2024-07-21T13:45"]),
            Kind::DateTime
        );
        assert_eq!(col(&["2024-07-21", "2024-07-21 09:00:00"]), Kind::DateTime);
        assert_eq!(col(&["2024-07-21 09:00:00.123Z"]), Kind::DateTime);
        assert_eq!(col(&["2024-13-99"]), Kind::Date); // shape, not calendar
        assert_eq!(col(&["21/07/2024"]), Kind::Text);
    }

    #[test]
    fn a_number_too_long_for_an_integer_is_still_a_number() {
        // Nineteen digits overflow a 64-bit integer; text is the safe answer.
        assert_eq!(col(&["1234567890123456789"]), Kind::Text);
    }

    #[test]
    fn columns_over_ragged_rows() {
        let rows = vec![
            vec!["1".to_string(), "a".to_string()],
            vec!["2".to_string()],
        ];
        assert_eq!(
            infer_columns(&rows, 3),
            vec![Kind::Integer, Kind::Text, Kind::Text]
        );
    }

    #[test]
    fn each_engine_gets_its_own_spelling() {
        assert_eq!(
            type_name(Kind::Number { scale: 2 }, "mysql"),
            "DECIMAL(38, 4)"
        );
        assert_eq!(
            type_name(Kind::Number { scale: 30 }, "mssql"),
            "DECIMAL(38, 18)"
        );
        assert_eq!(type_name(Kind::Boolean, "mssql"), "BIT");
        assert_eq!(type_name(Kind::Boolean, "oracle"), "NUMBER(1)");
        assert_eq!(type_name(Kind::Text, "postgres"), "text");
        assert_eq!(type_name(Kind::DateTime, "sqlite"), "DATETIME");
        assert_eq!(type_name(Kind::Integer, "clickhouse"), "Int64");
    }

    #[test]
    fn header_names_are_trimmed_filled_and_made_unique() {
        let header: Vec<String> = ["id", " Name ", "", "name", "ID"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            column_names(&header),
            vec!["id", "Name", "column_3", "name_2", "ID_2"]
        );
    }
}
