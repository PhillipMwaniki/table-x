//! Exporting a table to a file.
//!
//! The rows are streamed: the driver hands over a batch at a time and each one
//! is written before the next is read, so memory is flat whether the table has
//! a thousand rows or a hundred million.
//!
//! A driver that has not implemented streaming still works â€” the trait's
//! default pushes the whole result as one batch â€” it simply does not get the
//! memory bound. `Capabilities::streaming` says which is which.

use serde::Serialize;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tablex_core::{
    driver::{FetchOptions, RowSink},
    error::{Error, Result},
    export::{Format, Writer},
    result::Column,
    value::Value,
};

use crate::state::AppState;

/// The event an export reports itself on.
pub const PROGRESS_EVENT: &str = "export-progress";

/// One progress report.
///
/// `total` is the planner's estimate for the table, not a count: an exact
/// `COUNT(*)` on a large table can cost as much as the export itself. The UI
/// says "about" for that reason, and a bar built from it can pass 100%.
#[derive(Clone, Serialize)]
pub struct Progress {
    pub id: String,
    pub table: String,
    pub rows: u64,
    pub total: Option<u64>,
    pub done: bool,
}

pub struct ExportRequest {
    /// Identifies this export in progress events and to `cancel`.
    pub id: String,
    pub connection_id: String,
    /// The table's name as SQL should refer to it, quoted by the driver.
    pub qualified: String,
    pub schema: Option<String>,
    pub table: String,
    pub format: Format,
    pub path: String,
}

/// Writes each batch out and reports what it has written.
///
/// Cancellation lives here rather than in the export loop: the sink is the one
/// thing the driver calls while it is reading, so returning an error from it is
/// what stops a stream mid-table instead of at the end of one.
struct FileSink<'a, W: std::io::Write + Send> {
    writer: Option<Writer<W>>,
    sink: Option<W>,
    format: Format,
    table: String,
    quote: char,
    rows: u64,
    cancel: &'a AtomicBool,
    // `Sync` because the sink crosses into the driver, and a driver is free to
    // read on a blocking thread â€” SQLite does exactly that.
    report: &'a (dyn Fn(u64, bool) + Sync),
}

impl<W: std::io::Write + Send> RowSink for FileSink<'_, W> {
    fn columns(&mut self, columns: &[Column]) -> Result<()> {
        let mut writer = Writer::new(
            self.sink.take().expect("columns is called once"),
            self.format,
            columns,
            &self.table,
            self.quote,
        );
        writer.begin().map_err(|e| Error::Io(e.to_string()))?;
        self.writer = Some(writer);
        Ok(())
    }

    fn rows(&mut self, rows: &[Vec<Value>]) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| Error::Other("rows arrived before columns".into()))?;
        writer
            .write_batch(rows)
            .map_err(|e| Error::Io(e.to_string()))?;
        self.rows += rows.len() as u64;
        (self.report)(self.rows, false);
        Ok(())
    }
}

/// Write a table to a file, returning how many rows were written.
///
/// `on_progress` takes a closure rather than a Tauri emitter so this whole path
/// can be tested without a running application.
pub async fn run(
    state: &AppState,
    request: ExportRequest,
    cancel: Arc<AtomicBool>,
    // `Sync` because the sink that calls it is handed to the driver, which may
    // read rows on a blocking thread â€” SQLite does exactly that.
    on_progress: impl Fn(Progress) + Sync,
) -> Result<u64> {
    let config = state.config_for(&request.connection_id).await?;
    let quote = state
        .drivers
        .get(&config.driver)?
        .info()
        .capabilities
        .identifier_quote;

    let session = state.sessions.get(&request.connection_id).await?;
    let mut guard = session.connection.lock().await;

    // An estimate, not a count. It exists to give the progress bar a
    // denominator; the export does not depend on it being right.
    let total = match guard
        .table_detail(request.schema.as_deref(), &request.table)
        .await
    {
        Ok(detail) => detail.estimated_rows.and_then(|n| u64::try_from(n).ok()),
        Err(_) => None,
    };

    let report = |rows: u64, done: bool| {
        on_progress(Progress {
            id: request.id.clone(),
            table: request.table.clone(),
            rows,
            total,
            done,
        });
    };
    // Announced before the query, because that query is often the slow part â€”
    // especially through a tunnel, where the first byte can be seconds away and
    // silence is indistinguishable from a hang.
    report(0, false);

    let file = std::fs::File::create(&request.path)
        .map_err(|e| Error::Io(format!("could not create {}: {}", request.path, e)))?;

    let mut sink = FileSink {
        writer: None,
        sink: Some(BufWriter::new(file)),
        format: request.format,
        table: request.table.clone(),
        quote,
        rows: 0,
        cancel: &cancel,
        report: &report,
    };

    // No ORDER BY and no paging: one statement, read to the end. The ordering
    // that OFFSET paging needed was a workaround for reading a table in pieces,
    // and streaming does not read it in pieces.
    let sql = format!("SELECT * FROM {}", request.qualified);
    let opts = FetchOptions {
        max_rows: None,
        offset: 0,
        timeout_secs: None,
    };

    let outcome = guard.stream(&sql, &opts, &mut sink).await;
    let written = sink.rows;

    match outcome {
        Ok(_) => {
            match sink.writer.take() {
                Some(w) => w.finish().map_err(|e| Error::Io(e.to_string()))?,
                // A statement that returned no columns at all wrote nothing;
                // an empty file is a better answer than a panic.
                None => written,
            };
            report(written, true);
            Ok(written)
        }
        Err(e) => {
            // Whatever went wrong â€” cancelled, disconnected, disk full â€” the
            // half-written file goes. It would look like a complete export, and
            // nothing about it would say which rows are missing.
            drop(sink);
            let _ = std::fs::remove_file(&request.path);
            report(written, true);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use indexmap::IndexMap;
    use std::sync::Mutex;
    use tablex_core::{
        config::TlsConfig,
        driver::{Connection, RowEdit, STREAM_BATCH},
        result::QueryOutcome,
        schema::{SchemaNode, TableDetail},
        ConnectionConfig,
    };

    /// A connection that streams a fixed number of rows in batches.
    ///
    /// It records the statements it was asked to run, which is how these tests
    /// check that the export stopped generating `ORDER BY` and `OFFSET` once it
    /// no longer reads the table in pieces.
    struct FakeTable {
        rows: usize,
        statements: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Connection for FakeTable {
        async fn execute(
            &mut self,
            _sql: &str,
            _opts: &tablex_core::driver::FetchOptions,
        ) -> Result<QueryOutcome> {
            Err(Error::Unsupported("this fake only streams".into()))
        }

        async fn stream(
            &mut self,
            sql: &str,
            _opts: &tablex_core::driver::FetchOptions,
            sink: &mut dyn RowSink,
        ) -> Result<u64> {
            self.statements.lock().unwrap().push(sql.to_string());

            sink.columns(&[Column {
                name: "id".into(),
                type_name: "int".into(),
                nullable: None,
                source: None,
            }])?;

            let mut sent = 0usize;
            while sent < self.rows {
                let size = STREAM_BATCH.min(self.rows - sent);
                let batch: Vec<Vec<Value>> = (0..size)
                    .map(|i| vec![Value::Int((sent + i) as i64)])
                    .collect();
                sink.rows(&batch)?;
                sent += size;
            }
            Ok(sent as u64)
        }

        async fn browse(&mut self, _parent: Option<&str>) -> Result<Vec<SchemaNode>> {
            Ok(vec![])
        }

        async fn table_detail(
            &mut self,
            _schema: Option<&str>,
            table: &str,
        ) -> Result<TableDetail> {
            Ok(TableDetail {
                schema: None,
                name: table.to_string(),
                columns: vec![],
                indexes: vec![],
                foreign_keys: vec![],
                primary_key: vec![],
                estimated_rows: Some(self.rows as i64),
                comment: None,
            })
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
        let dir = std::env::temp_dir().join(format!("tablex-export-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    async fn state_with(dir: &std::path::Path, rows: usize) -> (AppState, Arc<Mutex<Vec<String>>>) {
        let state = AppState::new(dir);
        let statements = Arc::new(Mutex::new(Vec::new()));

        state.connections.lock().await.push(ConnectionConfig {
            id: "c1".into(),
            name: "test".into(),
            // Any compiled-in driver: only its quote character is consulted.
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
            options: IndexMap::new(),
        });

        state
            .sessions
            .insert(
                "c1",
                Box::new(FakeTable {
                    rows,
                    statements: statements.clone(),
                }),
                None,
            )
            .await;

        (state, statements)
    }

    fn request(dir: &std::path::Path, name: &str) -> ExportRequest {
        ExportRequest {
            id: "e1".into(),
            connection_id: "c1".into(),
            qualified: "\"users\"".into(),
            schema: None,
            table: "users".into(),
            format: Format::Csv,
            path: dir.join(name).to_string_lossy().into_owned(),
        }
    }

    #[tokio::test]
    async fn writes_every_streamed_row_in_order() {
        let dir = temp_dir("streamed");
        // Several batches, the last one short, so the writer has to handle both.
        let rows = STREAM_BATCH * 2 + 7;
        let (state, statements) = state_with(&dir, rows).await;

        let written = run(
            &state,
            request(&dir, "out.csv"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("export");

        assert_eq!(written, rows as u64);

        let text = std::fs::read_to_string(dir.join("out.csv")).expect("read back");
        assert_eq!(text.lines().count(), rows + 1, "header plus every row");
        assert_eq!(text.lines().next(), Some("id"));
        assert_eq!(text.lines().nth(1), Some("0"));
        assert_eq!(text.lines().last(), Some((rows - 1).to_string().as_str()));

        // One statement, and a plain one. The ORDER BY that OFFSET paging
        // needed was a workaround for reading a table in pieces; streaming does
        // not read it in pieces, so it is gone.
        let seen = statements.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(!seen[0].contains("ORDER BY"), "{seen:?}");
        assert!(!seen[0].contains("LIMIT"), "{seen:?}");
    }

    #[tokio::test]
    async fn progress_is_reported_before_the_first_batch_and_after_the_last() {
        let dir = temp_dir("progress");
        let rows = STREAM_BATCH + 1;
        let (state, _) = state_with(&dir, rows).await;
        let seen = Arc::new(Mutex::new(Vec::new()));

        let collector = seen.clone();
        run(
            &state,
            request(&dir, "out.csv"),
            Arc::new(AtomicBool::new(false)),
            move |p| collector.lock().unwrap().push((p.rows, p.done, p.total)),
        )
        .await
        .expect("export");

        let reports = seen.lock().unwrap().clone();
        // The first report precedes the query, because that query is the part
        // most likely to be slow - especially through a tunnel.
        assert_eq!(reports.first(), Some(&(0, false, Some(rows as u64))));
        assert_eq!(
            reports.last(),
            Some(&(rows as u64, true, Some(rows as u64)))
        );
        // One per batch in between, so the bar actually moves.
        assert!(reports.len() >= 4, "{reports:?}");
    }

    #[tokio::test]
    async fn cancelling_stops_mid_stream_and_removes_the_partial_file() {
        let dir = temp_dir("cancelled");
        let (state, _) = state_with(&dir, STREAM_BATCH * 50).await;
        let cancel = Arc::new(AtomicBool::new(false));

        let flag = cancel.clone();
        let err = run(&state, request(&dir, "out.csv"), cancel, move |p| {
            if p.rows > 0 {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .await
        .expect_err("cancelled");

        // Cancellation reaches the driver through the sink, so it stops between
        // batches rather than after reading the whole table.
        assert!(matches!(err, Error::Cancelled), "{err}");
        // A half-written export left on disk is the dangerous outcome: it looks
        // like a complete file and nothing about it says which rows are missing.
        assert!(
            !dir.join("out.csv").exists(),
            "partial file must not survive"
        );
    }

    #[tokio::test]
    async fn an_empty_table_still_writes_a_readable_file() {
        let dir = temp_dir("empty");
        let (state, _) = state_with(&dir, 0).await;

        let written = run(
            &state,
            request(&dir, "out.csv"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("export");

        assert_eq!(written, 0);
        // The header alone: a valid CSV that says what the columns were.
        assert_eq!(
            std::fs::read_to_string(dir.join("out.csv")).expect("read back"),
            "id\n"
        );
    }
}
