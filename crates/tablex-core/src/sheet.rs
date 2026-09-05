//! Reading a spreadsheet as rows of text.
//!
//! A workbook is the other file people arrive at an import with, and it is
//! not a text format: the cells are typed, dates are day counts, and the whole
//! thing is a zip of XML. `calamine` does the reading; this module turns what
//! it finds into the same rows of strings a delimited file produces, so the
//! import path downstream — mapping, typing, batching — is one path.
//!
//! Each value is spelled the way the rest of this application spells it when
//! it prints one: an integer without a point, a date as `YYYY-MM-DD`, a
//! timestamp with a space between the two halves. The type inference that
//! follows reads those spellings, so a spreadsheet's date column becomes a
//! date column and not a column of floats.

use crate::error::{Error, Result};
use calamine::{open_workbook_auto, Data, Reader};

/// The worksheets in a file, in the order the workbook lists them.
pub fn sheet_names(path: &str) -> Result<Vec<String>> {
    let workbook =
        open_workbook_auto(path).map_err(|e| Error::Io(format!("could not open {path}: {e}")))?;
    Ok(workbook.sheet_names().to_vec())
}

/// One worksheet as rows of text, and the name of the sheet that was read.
///
/// The first sheet when none is named. Rows come back as wide as the widest
/// row, since a spreadsheet's rows are ragged and an import's are not; empty
/// trailing rows are dropped, since a sheet's "used range" often extends past
/// the last cell somebody typed into.
pub fn read_sheet(path: &str, name: Option<&str>) -> Result<(String, Vec<Vec<String>>)> {
    let mut workbook =
        open_workbook_auto(path).map_err(|e| Error::Io(format!("could not open {path}: {e}")))?;
    let names = workbook.sheet_names().to_vec();
    let chosen = match name {
        Some(n) => names
            .iter()
            .find(|s| s.as_str() == n)
            .cloned()
            .ok_or_else(|| Error::Config(format!("there is no worksheet named {n}")))?,
        None => names
            .first()
            .cloned()
            .ok_or_else(|| Error::Config("the workbook has no worksheets".into()))?,
    };
    let range = workbook
        .worksheet_range(&chosen)
        .map_err(|e| Error::Io(format!("could not read {chosen}: {e}")))?;

    let width = range.width();
    let mut rows: Vec<Vec<String>> = range
        .rows()
        .map(|row| {
            let mut out: Vec<String> = row.iter().map(cell_text).collect();
            out.resize(width, String::new());
            out
        })
        .collect();
    while rows.last().is_some_and(|r| r.iter().all(|c| c.is_empty())) {
        rows.pop();
    }
    Ok((chosen, rows))
}

/// A cell as the text this application would print for its value.
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            // A whole number stored as a float is how a spreadsheet keeps
            // every number; printing `3` as `3.0` would make an integer column
            // a decimal one.
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(t) if t.time() == chrono::NaiveTime::MIN => {
                t.date().format("%Y-%m-%d").to_string()
            }
            Some(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
            None => dt.as_f64().to_string(),
        },
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        // A cell showing `#DIV/0!` has no value to import.
        Data::Error(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    /// A workbook written by this application's own writer, read back.
    fn workbook(rows: &[Vec<Value>]) -> String {
        let columns = vec!["id".to_string(), "name".to_string(), "when".to_string()];
        let mut wb = crate::xlsx::Workbook::start(&columns).unwrap();
        for row in rows {
            wb.row(row).unwrap();
        }
        let path =
            std::env::temp_dir().join(format!("tablex-sheet-test-{}.xlsx", uuid::Uuid::new_v4()));
        let mut file = std::fs::File::create(&path).unwrap();
        wb.write_to(&mut file).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn reads_back_what_the_writer_wrote() {
        let path = workbook(&[
            vec![
                Value::Int(1),
                Value::Text("Ann".into()),
                Value::Text("2024-07-21".into()),
            ],
            vec![Value::Float(2.0), Value::Null, Value::Bool(true)],
        ]);
        assert_eq!(sheet_names(&path).unwrap(), vec!["Rows".to_string()]);
        let (name, rows) = read_sheet(&path, None).unwrap();
        assert_eq!(name, "Rows");
        assert_eq!(
            rows,
            vec![
                vec!["id", "name", "when"],
                vec!["1", "Ann", "2024-07-21"],
                // A whole float reads as an integer; a null as nothing.
                vec!["2", "", "true"],
            ]
            .into_iter()
            .map(|r| r.into_iter().map(String::from).collect::<Vec<_>>())
            .collect::<Vec<_>>()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_sheet_is_named_in_the_error() {
        let path = workbook(&[]);
        let err = read_sheet(&path, Some("Nope")).unwrap_err().to_string();
        assert!(err.contains("Nope"), "{err}");
        let _ = std::fs::remove_file(path);
    }
}
