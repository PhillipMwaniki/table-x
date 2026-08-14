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
    /// What is being worked on, ready to show: a table, a database, a file.
    pub label: String,
    /// What `rows` counts. An import applies statements, not rows, and a bar
    /// that says "rows" over a restore is simply wrong.
    pub unit: String,
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
            label: request.table.clone(),
            unit: "rows".into(),
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

/// Everything needed to dump a whole database.
pub struct DatabaseExportRequest {
    pub id: String,
    pub connection_id: String,
    /// The database being dumped, for the header and the progress label.
    pub database: String,
    pub path: String,
}

/// Dump every table in a database to one SQL file.
///
/// Schema first, then data, table by table — the order a restore needs, since
/// an INSERT into a table that does not exist yet is an error rather than a
/// deferred write.
///
/// Only tables whose driver can render a CREATE statement get one; the rest are
/// dumped as data alone, with a note in the file saying so. That is the honest
/// position: PostgreSQL has no catalogue function that renders a table, and
/// pretending otherwise would produce a file that restores into nothing.
pub async fn run_database(
    state: &AppState,
    request: DatabaseExportRequest,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(Progress) + Sync,
) -> Result<u64> {
    let config = state.config_for(&request.connection_id).await?;
    let info = state.drivers.get(&config.driver)?.info();
    let quote = info.capabilities.identifier_quote;
    let scriptable = info.capabilities.table_scripts;

    let session = state.sessions.get(&request.connection_id).await?;
    let mut guard = session.connection.lock().await;

    let tables = collect_tables(&mut **guard, &request.database).await?;
    let total = Some(tables.len() as u64);

    let report = |done_tables: u64, label: &str, done: bool| {
        on_progress(Progress {
            id: request.id.clone(),
            label: label.to_string(),
            unit: "tables".into(),
            rows: done_tables,
            total,
            done,
        });
    };
    report(0, &request.database, false);

    let file = std::fs::File::create(&request.path)
        .map_err(|e| Error::Io(format!("could not create {}: {}", request.path, e)))?;
    let mut out = BufWriter::new(file);

    let header = format!(
        "-- Table X dump of {}\n-- {} tables\n\n",
        request.database,
        tables.len()
    );
    use std::io::Write as _;
    out.write_all(header.as_bytes())
        .map_err(|e| Error::Io(e.to_string()))?;

    let mut written = 0u64;
    for (index, table) in tables.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = std::fs::remove_file(&request.path);
            return Err(Error::Cancelled);
        }
        report(index as u64, &table.name, false);

        writeln!(out, "--\n-- {}\n--", table.name).map_err(|e| Error::Io(e.to_string()))?;

        if scriptable {
            match guard.definition(&table.id).await {
                Ok(ddl) => {
                    // Dumps end statements with a semicolon; SHOW CREATE and
                    // friends do not include one.
                    writeln!(out, "{};\n", ddl.trim_end_matches(';'))
                        .map_err(|e| Error::Io(e.to_string()))?;
                }
                Err(e) => {
                    writeln!(out, "-- no CREATE statement available: {e}\n")
                        .map_err(|e| Error::Io(e.to_string()))?;
                }
            }
        } else {
            writeln!(
                out,
                "-- {} cannot render a CREATE statement for a table; data only\n",
                info.name
            )
            .map_err(|e| Error::Io(e.to_string()))?;
        }

        let mut sink = FileSink {
            writer: None,
            sink: Some(&mut out),
            format: Format::Sql,
            table: table.name.clone(),
            quote,
            rows: 0,
            cancel: &cancel,
            report: &|_, _| {},
        };

        let sql = format!("SELECT * FROM {}", table.qualified);
        let opts = FetchOptions {
            max_rows: None,
            offset: 0,
            timeout_secs: None,
        };
        guard.stream(&sql, &opts, &mut sink).await?;
        written += sink.rows;
        // The writer borrows `out`; it has to be finished before the next table
        // can write its own header through the same handle.
        if let Some(w) = sink.writer.take() {
            w.finish().map_err(|e| Error::Io(e.to_string()))?;
        }
        writeln!(out).map_err(|e| Error::Io(e.to_string()))?;
    }

    out.flush().map_err(|e| Error::Io(e.to_string()))?;
    report(tables.len() as u64, &request.database, true);
    Ok(written)
}

/// One table found in the tree.
struct FoundTable {
    /// Tree path, for asking the driver to script it.
    id: String,
    name: String,
    /// Quoted and qualified by the driver.
    qualified: String,
}

/// Walk the object tree and collect the tables of one database.
///
/// The tree is walked rather than queried directly because its shape differs by
/// engine — PostgreSQL and SQL Server put schemas between a database and its
/// tables, MySQL and ClickHouse do not — and the walk does not need to know
/// which is which. It descends only into the named database, and stops at the
/// tables themselves rather than expanding their columns.
async fn collect_tables(
    conn: &mut dyn tablex_core::driver::Connection,
    database: &str,
) -> Result<Vec<FoundTable>> {
    use tablex_core::schema::NodeKind;

    let mut found = Vec::new();
    let roots = conn.browse(None).await?;

    // A file-backed engine has no database level: its roots are already the
    // folders of the one database there is.
    let mut frontier: Vec<String> = if roots.iter().any(|n| matches!(n.kind, NodeKind::Database)) {
        roots
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Database) && n.name == database)
            .map(|n| n.id.clone())
            .collect()
    } else {
        roots.iter().map(|n| n.id.clone()).collect()
    };

    while let Some(parent) = frontier.pop() {
        for node in conn.browse(Some(&parent)).await? {
            match node.kind {
                NodeKind::Table | NodeKind::View | NodeKind::MaterializedView => {
                    if let Some(qualified) = node.qualified.clone() {
                        found.push(FoundTable {
                            id: node.id,
                            name: node.name,
                            qualified,
                        });
                    }
                }
                // Schemas and folders are containers; anything else — a
                // function, a trigger — has no rows to dump.
                NodeKind::Schema | NodeKind::Folder => frontier.push(node.id),
                _ => {}
            }
        }
    }

    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
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
