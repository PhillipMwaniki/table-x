//! Writing result rows out as CSV, JSON, or SQL.
//!
//! Every writer takes rows one batch at a time and writes them straight to a
//! [`std::io::Write`], so an export is bounded by the size of a batch rather
//! than the size of the table.
//!
//! The three formats disagree about almost everything that matters — what a
//! NULL is, whether a number is quoted, what happens to a newline inside a
//! value — so each one states its choice where it makes it.

use crate::driver::RowSink;
use crate::result::Column;
use crate::value::Value;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Csv,
    Json,
    Sql,
}

impl Format {
    /// Conventional extension, used to seed the save dialog's filename.
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Sql => "sql",
        }
    }
}

/// Writes one result set to a sink, a batch at a time.
pub struct Writer<W: Write> {
    sink: W,
    format: Format,
    columns: Vec<String>,
    /// Table name used by the SQL writer's `INSERT INTO`.
    table: String,
    quote: char,
    rows_written: u64,
}

impl<W: Write> Writer<W> {
    pub fn new(sink: W, format: Format, columns: &[Column], table: &str, quote: char) -> Self {
        Writer {
            sink,
            format,
            columns: columns.iter().map(|c| c.name.clone()).collect(),
            table: table.to_string(),
            quote,
            rows_written: 0,
        }
    }

    /// Anything that precedes the first row.
    pub fn begin(&mut self) -> std::io::Result<()> {
        match self.format {
            Format::Csv => {
                let header = self
                    .columns
                    .iter()
                    .map(|c| csv_field(c))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(self.sink, "{header}")
            }
            Format::Json => writeln!(self.sink, "["),
            // The SQL writer names its columns on every statement instead, so
            // the file survives a table whose column order later changes.
            Format::Sql => Ok(()),
        }
    }

    pub fn write_batch(&mut self, rows: &[Vec<Value>]) -> std::io::Result<()> {
        for row in rows {
            match self.format {
                Format::Csv => {
                    let line = row.iter().map(csv_value).collect::<Vec<_>>().join(",");
                    writeln!(self.sink, "{line}")?;
                }
                Format::Json => {
                    // Commas go *before* each row but the first, so the file is
                    // valid the moment the last batch is written — no rewinding
                    // to remove a trailing comma.
                    if self.rows_written > 0 {
                        writeln!(self.sink, ",")?;
                    }
                    let fields = self
                        .columns
                        .iter()
                        .zip(row.iter())
                        .map(|(name, value)| format!("{}:{}", json_string(name), json_value(value)))
                        .collect::<Vec<_>>()
                        .join(",");
                    write!(self.sink, "  {{{fields}}}")?;
                }
                Format::Sql => {
                    let names = self
                        .columns
                        .iter()
                        .map(|c| quote_ident(c, self.quote))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let values = row.iter().map(sql_literal).collect::<Vec<_>>().join(", ");
                    writeln!(
                        self.sink,
                        "INSERT INTO {} ({names}) VALUES ({values});",
                        quote_ident(&self.table, self.quote)
                    )?;
                }
            }
            self.rows_written += 1;
        }
        Ok(())
    }

    /// Anything that follows the last row. Returns the number of rows written.
    pub fn finish(mut self) -> std::io::Result<u64> {
        if self.format == Format::Json {
            write!(self.sink, "\n]\n")?;
        }
        self.sink.flush()?;
        Ok(self.rows_written)
    }
}

/// Quote an identifier for the target engine.
///
/// A local copy of the rule in `sql::quote_ident`, because SQL Server's `[`
/// closes with `]` — a doubling rule that a general helper handles too.
fn quote_ident(name: &str, quote: char) -> String {
    crate::sql::quote_ident(name, quote)
}

/// One CSV field, quoted per RFC 4180 when it has to be.
///
/// Quoting only when necessary keeps the common case readable, and the rule for
/// when it is necessary is exactly: the field contains a comma, a quote, or a
/// line break.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// A value as a CSV field.
///
/// NULL is written as an empty field, which is the convention every spreadsheet
/// and `COPY … FROM` implementation reads back. It does mean NULL and the empty
/// string are indistinguishable in CSV — a property of the format, not of this
/// writer, and the reason the SQL and JSON exports exist.
fn csv_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        other => csv_field(&other.to_string()),
    }
}

fn json_string(text: &str) -> String {
    // serde_json rather than hand-rolled escaping: control characters and
    // lone surrogates have rules that are easy to get subtly wrong.
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// A value as JSON.
///
/// Numbers stay numbers and booleans stay booleans, so the file round-trips
/// into anything that reads JSON. Exact numerics are the exception: they are
/// written as strings, because a `NUMERIC(38,10)` does not survive a
/// double-precision JSON number, and silently rounding money in an export is
/// the same bug this whole codebase avoids elsewhere.
fn json_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        // NaN and the infinities have no JSON representation at all.
        Value::Float(_) => "null".to_string(),
        Value::Json(raw) => raw.to_string(),
        other => json_string(&other.to_string()),
    }
}

/// A value as a SQL literal.
///
/// Strings are single-quoted with doubled quotes, which is the one escaping
/// rule every engine here agrees on — backslash escapes are not, so they are
/// deliberately not used.
fn sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Int(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Float(_) => "NULL".to_string(),
        // Exact numerics are unquoted so they arrive as numbers, and unaltered
        // so they arrive with every digit.
        Value::Numeric(text) => text.clone(),
        Value::Bytes(bytes) => {
            let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
            format!("X'{hex}'")
        }
        other => format!("'{}'", other.to_string().replace('\'', "''")),
    }
}

/// A [`RowSink`] that writes straight through a [`Writer`].
///
/// Lives here rather than in either caller because both the desktop app and the
/// CLI need exactly this, and two copies of "stream a result into a file" would
/// eventually disagree about something — most likely about what happens to a
/// half-written file when the query fails halfway.
///
/// `on_batch` is called after every batch with the running row count. Returning
/// an error from it stops the stream at the next batch, which is how a
/// cancelled export stops without the driver knowing anything about
/// cancellation.
pub struct StreamSink<'a, W: std::io::Write + Send> {
    writer: Option<Writer<W>>,
    sink: Option<W>,
    format: Format,
    table: String,
    quote: char,
    rows: u64,
    // `Sync` because the sink crosses into the driver, and a driver is free to
    // read on a blocking thread — SQLite does exactly that.
    on_batch: &'a (dyn Fn(u64) -> crate::error::Result<()> + Sync),
}

impl<'a, W: std::io::Write + Send> StreamSink<'a, W> {
    pub fn new(
        sink: W,
        format: Format,
        table: impl Into<String>,
        quote: char,
        on_batch: &'a (dyn Fn(u64) -> crate::error::Result<()> + Sync),
    ) -> Self {
        StreamSink {
            writer: None,
            sink: Some(sink),
            format,
            table: table.into(),
            quote,
            rows: 0,
            on_batch,
        }
    }

    /// Rows handed over so far.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Close the file properly, writing whatever the format needs at the end.
    ///
    /// A statement that returned no columns at all never opened a writer; an
    /// empty file is a better answer there than a panic.
    pub fn finish(mut self) -> std::io::Result<u64> {
        match self.writer.take() {
            Some(writer) => writer.finish(),
            None => Ok(self.rows),
        }
    }
}

impl<W: std::io::Write + Send> RowSink for StreamSink<'_, W> {
    fn columns(&mut self, columns: &[crate::result::Column]) -> crate::error::Result<()> {
        let mut writer = Writer::new(
            self.sink.take().ok_or_else(|| {
                crate::error::Error::Other("columns was called more than once".into())
            })?,
            self.format,
            columns,
            &self.table,
            self.quote,
        );
        writer
            .begin()
            .map_err(|e| crate::error::Error::Io(e.to_string()))?;
        self.writer = Some(writer);
        Ok(())
    }

    fn rows(&mut self, rows: &[Vec<crate::value::Value>]) -> crate::error::Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| crate::error::Error::Other("rows arrived before columns".into()))?;
        writer
            .write_batch(rows)
            .map_err(|e| crate::error::Error::Io(e.to_string()))?;
        self.rows += rows.len() as u64;
        (self.on_batch)(self.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(names: &[&str]) -> Vec<Column> {
        names
            .iter()
            .map(|n| Column {
                name: n.to_string(),
                type_name: "text".into(),
                nullable: None,
                source: None,
            })
            .collect()
    }

    fn export(format: Format, cols: &[&str], rows: Vec<Vec<Value>>) -> String {
        let mut buffer = Vec::new();
        let mut writer = Writer::new(&mut buffer, format, &columns(cols), "users", '"');
        writer.begin().expect("begin");
        writer.write_batch(&rows).expect("batch");
        writer.finish().expect("finish");
        String::from_utf8(buffer).expect("utf-8")
    }

    #[test]
    fn csv_quotes_only_the_fields_that_need_it() {
        let out = export(
            Format::Csv,
            &["a", "b"],
            vec![vec![
                Value::Text("plain".into()),
                Value::Text("has,comma".into()),
            ]],
        );
        assert_eq!(out, "a,b\nplain,\"has,comma\"\n");
    }

    #[test]
    fn csv_doubles_embedded_quotes_and_keeps_newlines_inside_the_field() {
        // A newline inside a quoted field is legal CSV and must not become a
        // row break, or every row after it shifts by one column.
        let out = export(
            Format::Csv,
            &["a"],
            vec![vec![Value::Text("say \"hi\"\nagain".into())]],
        );
        assert_eq!(out, "a\n\"say \"\"hi\"\"\nagain\"\n");
    }

    #[test]
    fn csv_writes_null_as_an_empty_field() {
        let out = export(Format::Csv, &["a"], vec![vec![Value::Null]]);
        assert_eq!(out, "a\n\n");
    }

    #[test]
    fn json_is_valid_after_the_last_row() {
        // Commas precede rows rather than following them, so the document never
        // needs a trailing comma removed after the fact.
        let out = export(
            Format::Json,
            &["a", "b"],
            vec![
                vec![Value::Int(1), Value::Text("x".into())],
                vec![Value::Int(2), Value::Null],
            ],
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed[0]["a"], 1);
        assert_eq!(parsed[1]["b"], serde_json::Value::Null);
    }

    #[test]
    fn json_keeps_exact_numerics_as_strings() {
        // A JSON number is a double. Writing 38 digits as one would round them,
        // which is the failure this codebase refuses everywhere else.
        let exact = "123456789012345678.1234567890";
        let out = export(
            Format::Json,
            &["amount"],
            vec![vec![Value::Numeric(exact.into())]],
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed[0]["amount"], exact);
    }

    #[test]
    fn json_escapes_control_characters() {
        let out = export(Format::Json, &["a"], vec![vec![Value::Text("a\tb".into())]]);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed[0]["a"], "a\tb");
    }

    #[test]
    fn sql_names_its_columns_on_every_statement() {
        // Without the column list, the file breaks the moment the table gains a
        // column — and breaks silently if one is merely reordered.
        let out = export(
            Format::Sql,
            &["id", "email"],
            vec![vec![Value::Int(1), Value::Text("a@b.c".into())]],
        );
        assert_eq!(
            out,
            "INSERT INTO \"users\" (\"id\", \"email\") VALUES (1, 'a@b.c');\n"
        );
    }

    #[test]
    fn sql_doubles_quotes_rather_than_backslash_escaping() {
        // Backslash escapes are MySQL-specific; doubling is the rule every
        // engine here accepts.
        let out = export(
            Format::Sql,
            &["a"],
            vec![vec![Value::Text("O'Brien \\ Co".into())]],
        );
        assert!(out.contains("'O''Brien \\ Co'"), "{out}");
    }

    #[test]
    fn sql_writes_exact_numerics_unquoted_and_unaltered() {
        let exact = "123456789012345678.1234567890";
        let out = export(
            Format::Sql,
            &["a"],
            vec![vec![Value::Numeric(exact.into())]],
        );
        assert!(out.contains(&format!("VALUES ({exact})")), "{out}");
    }

    #[test]
    fn sql_writes_bytes_as_a_hex_literal() {
        let out = export(
            Format::Sql,
            &["a"],
            vec![vec![Value::Bytes(vec![0x00, 0xff, 0x10])]],
        );
        assert!(out.contains("X'00FF10'"), "{out}");
    }

    #[test]
    fn an_empty_result_still_produces_a_readable_file() {
        // A header with no rows is a valid CSV; an empty JSON array is valid
        // JSON. Neither should need special handling by whatever reads it.
        assert_eq!(export(Format::Csv, &["a"], vec![]), "a\n");
        let json = export(Format::Json, &["a"], vec![]);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed.as_array().map(Vec::len), Some(0));
    }
}
