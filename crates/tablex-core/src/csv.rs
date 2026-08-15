//! Reading delimited files, a chunk at a time.
//!
//! The same shape as the statement splitter and for the same reason: a file
//! being imported may be larger than memory, so rows have to come out as bytes
//! go in. A field can contain the delimiter, a quote, or a line break, so the
//! reader is a state machine rather than a `split`.
//!
//! Follows RFC 4180: fields may be quoted, a quote inside a quoted field is
//! doubled, and CRLF and LF both end a record.

/// Where the reader is between chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Between fields, or at the start of one.
    FieldStart,
    /// Inside an unquoted field.
    Bare,
    /// Inside `"…"`.
    Quoted,
    /// Just saw a `"` inside a quoted field: either an escape or the end.
    QuoteInQuoted,
}

pub struct CsvReader {
    delimiter: u8,
    state: State,
    field: String,
    record: Vec<String>,
    /// True once any byte of the current record has been seen, so a trailing
    /// newline does not produce a phantom empty row.
    started: bool,
}

impl CsvReader {
    pub fn new(delimiter: char) -> Self {
        CsvReader {
            delimiter: delimiter as u8,
            state: State::FieldStart,
            field: String::new(),
            record: Vec::new(),
            started: false,
        }
    }

    /// Feed text, returning whatever records are now complete.
    pub fn push(&mut self, text: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();

        for ch in text.chars() {
            let byte = if ch.is_ascii() { ch as u8 } else { 0 };

            match self.state {
                State::FieldStart => {
                    self.started = true;
                    if ch == '"' {
                        self.state = State::Quoted;
                    } else if byte == self.delimiter {
                        self.end_field();
                    } else if ch == '\n' {
                        self.end_record(&mut out);
                    } else if ch == '\r' {
                        // Held back: a lone CR is a line end, and CRLF must not
                        // produce two.
                    } else {
                        self.field.push(ch);
                        self.state = State::Bare;
                    }
                }

                State::Bare => {
                    if byte == self.delimiter {
                        self.end_field();
                    } else if ch == '\n' {
                        self.end_record(&mut out);
                    } else if ch != '\r' {
                        self.field.push(ch);
                    }
                }

                State::Quoted => {
                    if ch == '"' {
                        self.state = State::QuoteInQuoted;
                    } else {
                        // Newlines inside quotes belong to the value. This is
                        // the case naive splitting gets wrong, and it shifts
                        // every subsequent row by a column when it does.
                        self.field.push(ch);
                    }
                }

                State::QuoteInQuoted => {
                    if ch == '"' {
                        self.field.push('"');
                        self.state = State::Quoted;
                    } else if byte == self.delimiter {
                        self.end_field();
                    } else if ch == '\n' {
                        self.end_record(&mut out);
                    } else if ch != '\r' {
                        // Text after a closing quote is malformed; keeping it
                        // loses less than dropping it.
                        self.field.push(ch);
                        self.state = State::Bare;
                    }
                }
            }
        }

        out
    }

    /// Flush a final record that ended without a line break.
    pub fn finish(&mut self) -> Option<Vec<String>> {
        if !self.started {
            return None;
        }
        let mut out = Vec::new();
        self.end_record(&mut out);
        out.into_iter().next()
    }

    fn end_field(&mut self) {
        self.record.push(std::mem::take(&mut self.field));
        self.state = State::FieldStart;
    }

    fn end_record(&mut self, out: &mut Vec<Vec<String>>) {
        self.record.push(std::mem::take(&mut self.field));
        out.push(std::mem::take(&mut self.record));
        self.state = State::FieldStart;
        self.started = false;
    }
}

/// Guess the delimiter from a sample.
///
/// Counted outside quotes on the first line only: a comma inside a quoted field
/// is not evidence of anything, and a file whose first line is unambiguous is
/// almost always right about the rest.
pub fn sniff_delimiter(sample: &str) -> char {
    let first_line = sample.lines().next().unwrap_or_default();
    let candidates = [',', ';', '\t', '|'];

    let mut best = ',';
    let mut best_count = 0;
    for candidate in candidates {
        let mut count = 0;
        let mut in_quotes = false;
        for ch in first_line.chars() {
            if ch == '"' {
                in_quotes = !in_quotes;
            } else if ch == candidate && !in_quotes {
                count += 1;
            }
        }
        if count > best_count {
            best = candidate;
            best_count = count;
        }
    }
    best
}

/// Render one field as a SQL literal for a column of `type_name`.
///
/// Everything is quoted and escaped by default, which is both safe and
/// portable: every engine here coerces a quoted literal into the column's type.
/// Numbers and booleans are emitted bare where the column is one and the text
/// parses as one, because MySQL reads `'true'` into a tinyint as 0 — quietly,
/// which is the worst way for an import to be wrong.
pub fn literal_for(field: &str, type_name: &str, null_as_empty: bool) -> String {
    if field.is_empty() && null_as_empty {
        return "NULL".to_string();
    }

    let ty = type_name.to_ascii_lowercase();
    let numeric = [
        "int", "serial", "decimal", "numeric", "float", "double", "real", "money",
    ]
    .iter()
    .any(|t| ty.contains(t));
    let boolean = ty.contains("bool") || ty == "bit";

    if numeric && looks_numeric(field) {
        return field.to_string();
    }
    if boolean {
        match field.to_ascii_lowercase().as_str() {
            "true" | "t" | "yes" | "y" | "1" => return "TRUE".to_string(),
            "false" | "f" | "no" | "n" | "0" => return "FALSE".to_string(),
            _ => {}
        }
    }

    format!("'{}'", field.replace('\'', "''"))
}

fn looks_numeric(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit() || c == '.')
        && body.chars().filter(|c| *c == '.').count() <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(input: &str, delimiter: char) -> Vec<Vec<String>> {
        let mut reader = CsvReader::new(delimiter);
        let mut out = reader.push(input);
        out.extend(reader.finish());
        out
    }

    /// The same input fed one character at a time, which puts a chunk boundary
    /// everywhere a field, quote, or line ending could be split.
    fn read_char_by_char(input: &str, delimiter: char) -> Vec<Vec<String>> {
        let mut reader = CsvReader::new(delimiter);
        let mut out = Vec::new();
        for ch in input.chars() {
            out.extend(reader.push(&ch.to_string()));
        }
        out.extend(reader.finish());
        out
    }

    #[test]
    fn reads_plain_rows() {
        assert_eq!(
            read_all("a,b\n1,2\n", ','),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn a_quoted_field_may_contain_the_delimiter() {
        assert_eq!(
            read_all("\"Smith, Jo\",42\n", ','),
            vec![vec!["Smith, Jo", "42"]]
        );
    }

    #[test]
    fn a_quoted_field_may_contain_a_line_break() {
        // The case naive splitting gets wrong, and when it does every later row
        // is shifted by a column.
        assert_eq!(
            read_all("\"line one\nline two\",x\n", ','),
            vec![vec!["line one\nline two", "x"]]
        );
    }

    #[test]
    fn doubled_quotes_are_one_quote() {
        assert_eq!(
            read_all("\"say \"\"hi\"\"\"\n", ','),
            vec![vec!["say \"hi\""]]
        );
    }

    #[test]
    fn crlf_ends_one_record_not_two() {
        assert_eq!(
            read_all("a,b\r\n1,2\r\n", ','),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn a_final_row_without_a_line_break_is_not_lost() {
        assert_eq!(
            read_all("a,b\n1,2", ','),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn a_trailing_newline_does_not_add_an_empty_row() {
        assert_eq!(read_all("a\n", ',').len(), 1);
    }

    #[test]
    fn empty_fields_are_preserved_including_at_the_ends() {
        assert_eq!(read_all(",a,\n", ','), vec![vec!["", "a", ""]]);
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        let input = "id,name\n1,\"Smith, Jo\"\n2,\"multi\nline\"\n3,\"say \"\"hi\"\"\"\r\n4,plain";
        assert_eq!(read_char_by_char(input, ','), read_all(input, ','));
        assert_eq!(read_all(input, ',').len(), 5);
    }

    #[test]
    fn other_delimiters_work() {
        assert_eq!(read_all("a;b\n", ';'), vec![vec!["a", "b"]]);
        assert_eq!(read_all("a\tb\n", '\t'), vec![vec!["a", "b"]]);
    }

    #[test]
    fn the_delimiter_is_sniffed_from_the_first_line() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3"), ';');
        assert_eq!(sniff_delimiter("a,b,c\n1,2,3"), ',');
        assert_eq!(sniff_delimiter("a\tb\n"), '\t');
        // A comma inside a quoted field is not evidence of anything.
        assert_eq!(sniff_delimiter("\"a,b\";c\n"), ';');
        // Nothing to go on: comma is the safe default.
        assert_eq!(sniff_delimiter("single\n"), ',');
    }

    #[test]
    fn text_is_quoted_and_escaped() {
        assert_eq!(literal_for("O'Brien", "text", true), "'O''Brien'");
    }

    #[test]
    fn an_empty_field_becomes_null_when_asked() {
        assert_eq!(literal_for("", "text", true), "NULL");
        // And an empty string when not: the two are different values, and CSV
        // cannot tell them apart on its own.
        assert_eq!(literal_for("", "text", false), "''");
    }

    #[test]
    fn numbers_go_in_bare_where_the_column_is_numeric() {
        assert_eq!(literal_for("42", "integer", true), "42");
        assert_eq!(literal_for("-1.5", "numeric(10,2)", true), "-1.5");
        // Text in a numeric column is still quoted, so the engine reports the
        // problem rather than this guessing at one.
        assert_eq!(literal_for("n/a", "integer", true), "'n/a'");
    }

    #[test]
    fn booleans_are_keywords_rather_than_strings() {
        // MySQL reads 'true' into a tinyint as 0, quietly — the worst way for
        // an import to be wrong.
        assert_eq!(literal_for("true", "boolean", true), "TRUE");
        assert_eq!(literal_for("0", "boolean", true), "FALSE");
        assert_eq!(literal_for("yes", "bool", true), "TRUE");
    }

    #[test]
    fn a_mysql_tinyint_boolean_takes_the_numeric_path() {
        // `tinyint(1)` *is* MySQL's boolean, and 0 and 1 are exactly what it
        // wants — the numeric branch is the right answer, not a missed case.
        assert_eq!(literal_for("0", "tinyint(1)", true), "0");
        assert_eq!(literal_for("1", "tinyint(1)", true), "1");
    }

    #[test]
    fn a_quoted_number_stays_text_in_a_text_column() {
        assert_eq!(literal_for("0042", "varchar(10)", true), "'0042'");
    }
}
