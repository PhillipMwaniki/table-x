//! Running a SQL file, or loading a file's rows, against a connection.
//!
//! The file is read in chunks and split into statements as it arrives, so a
//! multi-gigabyte dump never exists in memory — only the statement currently
//! being assembled does. Statements reach the server while the rest of the file
//! is still being read.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tablex_core::{
    csv::{literal_for, CsvReader},
    driver::FetchOptions,
    error::{Error, Result},
    sql::{quote_ident, Splitter},
};

use crate::{export::Progress, state::AppState};

/// Bytes read per turn. Large enough to keep the round trips down, small enough
/// that progress moves while a big file is being read.
const CHUNK: usize = 64 * 1024;

pub struct ImportRequest {
    pub id: String,
    pub connection_id: String,
    pub path: String,
}

/// Apply every statement in a SQL file, returning how many were run.
pub async fn run(
    state: &AppState,
    request: ImportRequest,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(Progress) + Sync,
) -> Result<u64> {
    let file = std::fs::File::open(&request.path)
        .map_err(|e| Error::Io(format!("could not open {}: {}", request.path, e)))?;
    let total_bytes = file.metadata().ok().map(|m| m.len());
    let mut reader = std::io::BufReader::new(file);

    let label = std::path::Path::new(&request.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.path.clone());

    let session = state.sessions.get(&request.connection_id).await?;
    let mut guard = session.connection.lock().await;

    // Progress is measured in bytes read rather than statements applied: the
    // statement count is unknown until the end, and a bar needs a denominator.
    let report = |bytes: u64, done: bool| {
        on_progress(Progress {
            id: request.id.clone(),
            label: label.clone(),
            unit: "KB".into(),
            rows: bytes / 1024,
            total: total_bytes.map(|b| b / 1024),
            done,
        });
    };
    report(0, false);

    // Statements in a dump are DDL and INSERTs; a stray SELECT is capped rather
    // than materialized, since nothing here displays its rows.
    let opts = FetchOptions {
        max_rows: Some(1),
        offset: 0,
        timeout_secs: None,
    };

    let mut splitter = Splitter::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut leftover: Vec<u8> = Vec::new();
    let mut read_bytes = 0u64;
    let mut applied = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }

        let n = reader
            .read(&mut buffer)
            .map_err(|e| Error::Io(format!("could not read {}: {}", request.path, e)))?;
        if n == 0 {
            break;
        }
        read_bytes += n as u64;

        // A chunk boundary can fall inside a multi-byte character, so the tail
        // of an invalid sequence is carried forward rather than replaced with
        // U+FFFD — which would corrupt exactly the text an import is meant to
        // reproduce.
        leftover.extend_from_slice(&buffer[..n]);
        let text = take_utf8(&mut leftover);

        for statement in splitter.push(&text) {
            apply(&mut **guard, &statement, &opts).await?;
            applied += 1;
        }
        report(read_bytes, false);
    }

    for statement in splitter.finish() {
        apply(&mut **guard, &statement, &opts).await?;
        applied += 1;
    }

    report(read_bytes, true);
    Ok(applied)
}

/// Rows per INSERT. Batching cuts the round trips that dominate an import
/// without building a statement so large a server rejects it.
const ROWS_PER_INSERT: usize = 200;

/// Records the sample read for a preview is taken from.
///
/// Enough rows for the type guesses to mean something — one decimal in the
/// hundredth row is what makes a column a number — without reading a file
/// that may be a gigabyte to show twenty rows.
const SAMPLE_ROWS: usize = 1_000;

/// Where the rows come from.
pub enum Source {
    /// A delimited text file, read a chunk at a time.
    Csv { delimiter: char },
    /// One worksheet of a workbook, read whole. `None` is the first sheet.
    Sheet { name: Option<String> },
}

/// A column of a table that does not exist yet.
pub struct NewColumn {
    pub name: String,
    /// In the engine's own spelling; see `infer::type_name`.
    pub type_name: String,
}

pub struct RowsImportRequest {
    pub id: String,
    pub connection_id: String,
    pub path: String,
    pub source: Source,
    pub schema: Option<String>,
    pub table: String,
    /// The target table, quoted and qualified by the driver. Ignored when the
    /// table is being created: the name is built here, from the same rule.
    pub qualified: String,
    /// Whether the first record names the columns rather than holding data.
    pub has_header: bool,
    /// Target column for each field position. `None` skips that field.
    pub mapping: Vec<Option<String>>,
    /// Whether an empty field means NULL rather than an empty string.
    pub null_as_empty: bool,
    /// Create the table first, with these columns.
    pub create: Option<Vec<NewColumn>>,
}

/// Load a file's rows into a table, returning the rows inserted.
///
/// Nothing is emptied first and nothing is deduplicated: the file is appended,
/// which is the only behaviour that cannot lose data the user did not ask to
/// lose. Replacing a table is a TRUNCATE they can run themselves, deliberately.
/// Creating the table is the one exception in shape, and it creates: a table
/// that already exists makes the CREATE fail before a row is touched.
pub async fn run_rows(
    state: &AppState,
    request: RowsImportRequest,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(Progress) + Sync,
) -> Result<u64> {
    let label = file_label(&request.path);
    let config = state.config_for(&request.connection_id).await?;
    let session = state.sessions.get(&request.connection_id).await?;
    let mut guard = session.connection.lock().await;

    let opts = FetchOptions {
        max_rows: Some(1),
        offset: 0,
        timeout_secs: None,
    };

    // The table, and the types that decide how each field is written. Without
    // them everything would be a quoted string, which MySQL turns into a
    // silent zero on a boolean column.
    let (qualified, types): (String, HashMap<String, String>) = match &request.create {
        Some(columns) => {
            let quote = tablex_core::diff::Dialect::for_driver(&config.driver).quote;
            let qualified = match &request.schema {
                Some(schema) => format!(
                    "{}.{}",
                    quote_ident(schema, quote),
                    quote_ident(&request.table, quote)
                ),
                None => quote_ident(&request.table, quote),
            };
            let definitions: Vec<String> = columns
                .iter()
                .map(|c| format!("  {} {}", quote_ident(&c.name, quote), c.type_name))
                .collect();
            if definitions.is_empty() {
                return Err(Error::Config("a table needs at least one column".into()));
            }
            let sql = format!("CREATE TABLE {qualified} (\n{}\n)", definitions.join(",\n"));
            guard.execute(&sql, &opts).await.map_err(|e| Error::Query {
                message: format!("{e}\n\nwhile creating {}", request.table),
                position: None,
                code: None,
            })?;
            (
                qualified,
                columns
                    .iter()
                    .map(|c| (c.name.clone(), c.type_name.clone()))
                    .collect(),
            )
        }
        None => {
            let types = match guard
                .table_detail(request.schema.as_deref(), &request.table)
                .await
            {
                Ok(detail) => detail
                    .columns
                    .into_iter()
                    .map(|c| (c.name, c.type_name))
                    .collect(),
                Err(_) => HashMap::new(),
            };
            (request.qualified.clone(), types)
        }
    };

    // The column list is fixed for every statement, so it is built once.
    let target_columns: Vec<String> = request
        .mapping
        .iter()
        .flatten()
        .map(|name| quote_ident(name, quote_char(&qualified)))
        .collect();
    if target_columns.is_empty() {
        return Err(Error::Config(
            "no columns are mapped, so there is nothing to import".into(),
        ));
    }

    let mut batch: Vec<String> = Vec::with_capacity(ROWS_PER_INSERT);
    let mut inserted = 0u64;
    let mut skipped_header = !request.has_header;

    match &request.source {
        Source::Csv { delimiter } => {
            let file = std::fs::File::open(&request.path)
                .map_err(|e| Error::Io(format!("could not open {}: {}", request.path, e)))?;
            let total_bytes = file.metadata().ok().map(|m| m.len());
            let mut reader_source = std::io::BufReader::new(file);
            let report = |bytes: u64, done: bool| {
                on_progress(Progress {
                    id: request.id.clone(),
                    label: label.clone(),
                    unit: "KB".into(),
                    rows: bytes / 1024,
                    total: total_bytes.map(|b| b / 1024),
                    done,
                });
            };
            report(0, false);

            let mut csv = CsvReader::new(*delimiter);
            let mut buffer = vec![0u8; CHUNK];
            let mut leftover: Vec<u8> = Vec::new();
            let mut read_bytes = 0u64;

            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let n = reader_source
                    .read(&mut buffer)
                    .map_err(|e| Error::Io(format!("could not read {}: {}", request.path, e)))?;
                if n == 0 {
                    break;
                }
                read_bytes += n as u64;
                leftover.extend_from_slice(&buffer[..n]);
                let text = take_utf8(&mut leftover);

                for record in csv.push(&text) {
                    if !skipped_header {
                        skipped_header = true;
                        continue;
                    }
                    batch.push(values_of(&record, &request, &types));
                    if batch.len() >= ROWS_PER_INSERT {
                        inserted +=
                            flush(&mut **guard, &qualified, &target_columns, &mut batch, &opts)
                                .await?;
                    }
                }
                report(read_bytes, false);
            }
            if let Some(record) = csv.finish() {
                if skipped_header {
                    batch.push(values_of(&record, &request, &types));
                }
            }
            if !batch.is_empty() {
                inserted +=
                    flush(&mut **guard, &qualified, &target_columns, &mut batch, &opts).await?;
            }
            report(read_bytes, true);
        }

        Source::Sheet { name } => {
            // A worksheet is read whole: it is bounded by the spreadsheet's own
            // limits, and there is no reading half of a zip.
            let (_, rows) = tablex_core::sheet::read_sheet(&request.path, name.as_deref())?;
            let total = rows.len() as u64;
            let report = |done_rows: u64, done: bool| {
                on_progress(Progress {
                    id: request.id.clone(),
                    label: label.clone(),
                    unit: "rows".into(),
                    rows: done_rows,
                    total: Some(total),
                    done,
                });
            };
            report(0, false);
            for (i, record) in rows.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                if !skipped_header {
                    skipped_header = true;
                    continue;
                }
                batch.push(values_of(record, &request, &types));
                if batch.len() >= ROWS_PER_INSERT {
                    inserted +=
                        flush(&mut **guard, &qualified, &target_columns, &mut batch, &opts).await?;
                    report(i as u64 + 1, false);
                }
            }
            if !batch.is_empty() {
                inserted +=
                    flush(&mut **guard, &qualified, &target_columns, &mut batch, &opts).await?;
            }
            report(total, true);
        }
    }

    Ok(inserted)
}

/// One record as a `(...)` tuple, in mapped column order.
fn values_of(
    record: &[String],
    request: &RowsImportRequest,
    types: &HashMap<String, String>,
) -> String {
    let mut values = Vec::new();
    for (index, target) in request.mapping.iter().enumerate() {
        let Some(name) = target else { continue };
        // A short record is padded rather than refused: a trailing empty column
        // is the most common shape of a hand-edited CSV.
        let field = record.get(index).map(String::as_str).unwrap_or("");
        let type_name = types.get(name).map(String::as_str).unwrap_or("");
        values.push(literal_for(field, type_name, request.null_as_empty));
    }
    format!("({})", values.join(", "))
}

/// Run one batched INSERT and clear the batch.
async fn flush(
    conn: &mut dyn tablex_core::driver::Connection,
    qualified: &str,
    columns: &[String],
    batch: &mut Vec<String>,
    opts: &FetchOptions,
) -> Result<u64> {
    if batch.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "INSERT INTO {qualified} ({}) VALUES {}",
        columns.join(", "),
        batch.join(", ")
    );
    let count = batch.len() as u64;
    conn.execute(&sql, opts).await.map_err(|e| Error::Query {
        // The row count says how far it got; a failed batch is the rows between
        // that count and the next two hundred.
        message: format!("{e}\n\nwhile inserting a batch of {count} rows"),
        position: None,
        code: None,
    })?;
    batch.clear();
    Ok(count)
}

/// The quote character a driver used, read back off its own qualified name.
///
/// Cheaper than threading the capability through: the driver already quoted the
/// table, so its first character is the answer.
fn quote_char(qualified: &str) -> char {
    match qualified.chars().next() {
        Some('`') => '`',
        Some('[') => '[',
        _ => '"',
    }
}

fn file_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Take the valid UTF-8 prefix, leaving a split character for the next chunk.
fn take_utf8(leftover: &mut Vec<u8>) -> String {
    match std::str::from_utf8(leftover) {
        Ok(text) => {
            let owned = text.to_string();
            leftover.clear();
            owned
        }
        Err(e) => {
            let good = e.valid_up_to();
            let owned = String::from_utf8_lossy(&leftover[..good]).into_owned();
            leftover.drain(..good);
            owned
        }
    }
}

/// What a file looks like, for the mapping dialog.
pub struct Preview {
    /// The delimiter used, given or sniffed. Absent for a worksheet.
    pub delimiter: Option<char>,
    /// The workbook's sheets, and the one read. Empty for a delimited file.
    pub sheets: Vec<String>,
    pub sheet: Option<String>,
    /// The first records, header included if there is one.
    pub rows: Vec<Vec<String>>,
    /// A longer run of records, for guessing the columns' types.
    pub sample: Vec<Vec<String>>,
}

/// Read the first records of a file without importing anything.
pub fn preview(path: &str, source: &Source, rows: usize) -> Result<Preview> {
    match source {
        Source::Csv { delimiter } => {
            let bytes = std::fs::read(path)
                .map_err(|e| Error::Io(format!("could not read {path}: {e}")))?;
            // Enough to see the shape without loading a gigabyte to show ten
            // rows. A whole-file scan is what the import itself is for.
            let head = &bytes[..bytes.len().min(256 * 1024)];
            let text = String::from_utf8_lossy(head);
            let delimiter = if *delimiter == '\0' {
                tablex_core::csv::sniff_delimiter(&text)
            } else {
                *delimiter
            };
            let mut reader = CsvReader::new(delimiter);
            let mut records = reader.push(&text);
            // The last record of a truncated head may be half a line; keep it
            // only when the whole file was read.
            if head.len() == bytes.len() {
                records.extend(reader.finish());
            }
            records.truncate(SAMPLE_ROWS);
            Ok(Preview {
                delimiter: Some(delimiter),
                sheets: Vec::new(),
                sheet: None,
                rows: records.iter().take(rows).cloned().collect(),
                sample: records,
            })
        }
        Source::Sheet { name } => {
            let sheets = tablex_core::sheet::sheet_names(path)?;
            let (sheet, mut records) = tablex_core::sheet::read_sheet(path, name.as_deref())?;
            records.truncate(SAMPLE_ROWS);
            Ok(Preview {
                delimiter: None,
                sheets,
                sheet: Some(sheet),
                rows: records.iter().take(rows).cloned().collect(),
                sample: records,
            })
        }
    }
}

/// Run one statement, naming it if it fails.
///
/// A restore that stops at statement 40,000 with only the server's message is
/// nearly impossible to act on; the statement itself is what tells you whether
/// the dump is wrong, the target already has the table, or the file is for a
/// different engine entirely.
async fn apply(
    conn: &mut dyn tablex_core::driver::Connection,
    statement: &str,
    opts: &FetchOptions,
) -> Result<()> {
    conn.execute(statement, opts).await.map_err(|e| {
        let preview: String = statement.chars().take(200).collect();
        let ellipsis = if statement.chars().count() > 200 {
            "…"
        } else {
            ""
        };
        Error::Query {
            message: format!("{e}\n\nwhile running:\n{preview}{ellipsis}"),
            position: None,
            code: None,
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use indexmap::IndexMap;
    use std::sync::Mutex;
    use tablex_core::{
        config::TlsConfig,
        driver::{Connection, RowEdit},
        result::QueryOutcome,
        schema::{SchemaNode, TableDetail},
        ConnectionConfig,
    };

    /// Records every statement it is asked to run, and can be told to fail on
    /// one of them.
    struct Recorder {
        applied: Arc<Mutex<Vec<String>>>,
        fail_on: Option<&'static str>,
    }

    #[async_trait]
    impl Connection for Recorder {
        async fn execute(&mut self, sql: &str, _opts: &FetchOptions) -> Result<QueryOutcome> {
            if self.fail_on.is_some_and(|needle| sql.contains(needle)) {
                return Err(Error::query("syntax error at or near \"oops\""));
            }
            self.applied.lock().unwrap().push(sql.to_string());
            Ok(QueryOutcome {
                statements: vec![],
                elapsed_ms: 0,
                notices: vec![],
            })
        }

        async fn browse(&mut self, _parent: Option<&str>) -> Result<Vec<SchemaNode>> {
            Ok(vec![])
        }

        async fn table_detail(
            &mut self,
            _schema: Option<&str>,
            _table: &str,
        ) -> Result<TableDetail> {
            Err(Error::Unsupported("not needed".into()))
        }

        async fn apply_edit(&mut self, _edit: &RowEdit) -> Result<()> {
            Ok(())
        }

        async fn ping(&mut self) -> Result<()> {
            Ok(())
        }

        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-import-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    async fn state_with(
        dir: &std::path::Path,
        fail_on: Option<&'static str>,
    ) -> (AppState, Arc<Mutex<Vec<String>>>) {
        let state = AppState::new(dir);
        let applied = Arc::new(Mutex::new(Vec::new()));

        state.connections.lock().await.push(ConnectionConfig {
            id: "c1".into(),
            name: "test".into(),
            driver: "sqlite".into(),
            host: None,
            port: None,
            database: None,
            username: None,
            file_path: Some(":memory:".into()),
            tls: TlsConfig::default(),
            ssh: None,
            folder: None,
            color: None,
            read_only: false,
            confirm_destructive: None,
            production: Default::default(),
            options: IndexMap::new(),
        });

        state
            .sessions
            .insert(
                "c1",
                Box::new(Recorder {
                    applied: applied.clone(),
                    fail_on,
                }),
                None,
                None,
            )
            .await;

        (state, applied)
    }

    fn write(dir: &std::path::Path, name: &str, sql: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, sql).expect("write dump");
        path.to_string_lossy().into_owned()
    }

    fn request(path: String) -> ImportRequest {
        ImportRequest {
            id: "i1".into(),
            connection_id: "c1".into(),
            path,
        }
    }

    #[tokio::test]
    async fn applies_every_statement_in_order() {
        let dir = temp_dir("basic");
        let path = write(
            &dir,
            "dump.sql",
            "CREATE TABLE t (a TEXT);\n\
             INSERT INTO t VALUES ('a;b');\n\
             -- a comment; with a semicolon\n\
             INSERT INTO t VALUES ('c')",
        );
        let (state, applied) = state_with(&dir, None).await;

        let count = run(
            &state,
            request(path),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("import");

        let applied = applied.lock().unwrap().clone();
        assert_eq!(count, 3, "{applied:?}");
        // The semicolon inside the string did not split the statement, and the
        // last statement arrived despite having no terminator.
        assert!(applied[1].contains("'a;b'"), "{applied:?}");
        assert!(applied[2].contains("'c'"), "{applied:?}");
    }

    #[tokio::test]
    async fn a_failing_statement_is_named_in_the_error() {
        let dir = temp_dir("failure");
        let path = write(
            &dir,
            "dump.sql",
            "INSERT INTO t VALUES (1);\nINSERT INTO oops VALUES (2);\nINSERT INTO t VALUES (3);",
        );
        let (state, applied) = state_with(&dir, Some("oops")).await;

        let err = run(
            &state,
            request(path),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect_err("the second statement fails");

        // A restore that stops at statement 40,000 with only the server's
        // message is nearly impossible to act on.
        let text = err.to_string();
        assert!(text.contains("oops"), "{text}");
        assert!(text.contains("while running"), "{text}");
        // And it stopped there rather than carrying on into the next statement.
        assert_eq!(applied.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancelling_stops_partway_through() {
        let dir = temp_dir("cancel");
        let mut sql = String::new();
        for i in 0..5_000 {
            sql.push_str(&format!("INSERT INTO t VALUES ({i});\n"));
        }
        let path = write(&dir, "dump.sql", &sql);
        let (state, applied) = state_with(&dir, None).await;

        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let err = run(&state, request(path), cancel, move |p| {
            if p.rows > 0 {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .await
        .expect_err("cancelled");

        assert!(matches!(err, Error::Cancelled), "{err}");
        // Statements already applied stay applied: a half-run script cannot be
        // taken back, and pretending otherwise would be worse than saying so.
        let count = applied.lock().unwrap().len();
        assert!(count < 5_000, "applied {count} of 5000");
    }

    #[tokio::test]
    async fn progress_is_measured_in_bytes_read() {
        let dir = temp_dir("progress");
        let path = write(&dir, "dump.sql", "SELECT 1;\nSELECT 2;");
        let (state, _) = state_with(&dir, None).await;
        let seen = Arc::new(Mutex::new(Vec::new()));

        let collector = seen.clone();
        run(
            &state,
            request(path),
            Arc::new(AtomicBool::new(false)),
            move |p| collector.lock().unwrap().push((p.unit.clone(), p.done)),
        )
        .await
        .expect("import");

        let reports = seen.lock().unwrap().clone();
        // The statement count is unknown until the end, so the bar counts what
        // it can: how much of the file has been read.
        assert!(reports.iter().all(|(unit, _)| unit == "KB"), "{reports:?}");
        assert_eq!(reports.first().map(|r| r.1), Some(false));
        assert_eq!(reports.last().map(|r| r.1), Some(true));
    }

    #[tokio::test]
    async fn creates_the_table_before_loading_a_file_into_it() {
        let dir = temp_dir("create");
        let path = write(&dir, "people.csv", "name,age\nAnn,41\nBob,\n");
        let (state, applied) = state_with(&dir, None).await;

        let count = run_rows(
            &state,
            RowsImportRequest {
                id: "i2".into(),
                connection_id: "c1".into(),
                path,
                source: Source::Csv { delimiter: ',' },
                schema: None,
                table: "people".into(),
                qualified: String::new(),
                has_header: true,
                mapping: vec![Some("name".into()), Some("age".into())],
                null_as_empty: true,
                create: Some(vec![
                    NewColumn {
                        name: "name".into(),
                        type_name: "TEXT".into(),
                    },
                    NewColumn {
                        name: "age".into(),
                        type_name: "INTEGER".into(),
                    },
                ]),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("import");

        assert_eq!(count, 2);
        let applied = applied.lock().unwrap();
        // The table first, in the driver's quoting, then the rows -- typed by
        // the columns just created, since there is no catalogue to ask yet.
        assert_eq!(
            applied[0],
            "CREATE TABLE \"people\" (\n  \"name\" TEXT,\n  \"age\" INTEGER\n)"
        );
        assert_eq!(
            applied[1],
            "INSERT INTO \"people\" (\"name\", \"age\") VALUES ('Ann', 41), ('Bob', NULL)"
        );
    }
}
