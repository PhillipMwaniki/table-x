//! Exporting a table to a file.
//!
//! The rows are fetched a page at a time and written as they arrive, so memory
//! is bounded by [`BATCH`] rather than by the size of the table.
//!
//! Paging needs a stable order or a page boundary can repeat one row and skip
//! another: `LIMIT/OFFSET` without `ORDER BY` leaves the order to the engine,
//! and engines are free to change it between statements. So the export orders
//! by the table's key when it has one, and falls back to a single unpaged fetch
//! when it does not — slower on memory, but correct, which is the trade this
//! codebase makes everywhere else.

use serde::Serialize;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tablex_core::{
    driver::FetchOptions,
    error::{Error, Result},
    export::{Format, Writer},
    result::StatementResult,
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

/// Rows per round trip. Large enough that the per-statement overhead disappears,
/// small enough that a page of wide rows is not itself a memory problem.
const BATCH: usize = 5_000;

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

/// Write a table to a file, returning how many rows were written.
///
/// Progress is emitted per batch rather than per row: an event per row would
/// cost more than the export, and a bar that updates every few thousand rows is
/// already smoother than the network underneath it.
/// `on_progress` takes a closure rather than a Tauri emitter so this whole path
/// can be tested without a running application.
pub async fn run(
    state: &AppState,
    request: ExportRequest,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(Progress),
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

    // The key doubles as the page order. A table without one is not an error —
    // plenty of tables have no primary key — it just cannot be paged safely.
    let (order, total) = match guard
        .table_detail(request.schema.as_deref(), &request.table)
        .await
    {
        Ok(detail) => (
            detail.edit_key(),
            detail.estimated_rows.and_then(|n| u64::try_from(n).ok()),
        ),
        // A view, or a table the catalogue will not describe: still exportable,
        // just not pageable, and with no estimate to show.
        Err(_) => (Vec::new(), None),
    };

    // Announced before the first query, because that query is often the slow
    // part — especially through a tunnel, where the first byte can be seconds
    // away and silence is indistinguishable from a hang.
    let report = |rows: u64, done: bool| {
        on_progress(Progress {
            id: request.id.clone(),
            table: request.table.clone(),
            rows,
            total,
            done,
        });
    };
    report(0, false);

    let file = std::fs::File::create(&request.path)
        .map_err(|e| Error::Io(format!("could not create {}: {}", request.path, e)))?;
    let sink = BufWriter::new(file);

    let mut writer: Option<Writer<BufWriter<std::fs::File>>> = None;
    let mut sink = Some(sink);
    let mut written = 0u64;
    let mut offset = 0usize;

    loop {
        let (sql, opts) = if order.is_empty() {
            (
                format!("SELECT * FROM {}", request.qualified),
                FetchOptions {
                    // One shot: without a stable order, paging could duplicate
                    // and drop rows, and a wrong export is worse than a large one.
                    max_rows: None,
                    offset: 0,
                    timeout_secs: None,
                },
            )
        } else {
            let by = order
                .iter()
                .map(|c| tablex_core::sql::quote_ident(c, quote))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!("SELECT * FROM {} ORDER BY {by}", request.qualified),
                FetchOptions {
                    max_rows: Some(BATCH),
                    offset,
                    timeout_secs: None,
                },
            )
        };

        if cancel.load(Ordering::Relaxed) {
            return cancelled(&request.path);
        }

        let outcome = guard.execute(&sql, &opts).await?;
        let Some(StatementResult::Rows(rows)) = outcome.statements.into_iter().next() else {
            return Err(Error::Unsupported(
                "that object returned no rows to export".into(),
            ));
        };

        // The header needs the column list, which only arrives with the first
        // page — so the writer is built here rather than before the loop.
        let writer = match writer.as_mut() {
            Some(w) => w,
            None => {
                let mut w = Writer::new(
                    sink.take().expect("sink is taken once"),
                    request.format,
                    &rows.columns,
                    &request.table,
                    quote,
                );
                w.begin().map_err(|e| Error::Io(e.to_string()))?;
                writer.insert(w)
            }
        };

        let count = rows.rows.len();
        writer
            .write_batch(&rows.rows)
            .map_err(|e| Error::Io(e.to_string()))?;
        written += count as u64;
        report(written, false);

        // Checked after writing rather than before: a batch already fetched is
        // paid for, and throwing it away would not make the cancel any faster.
        if cancel.load(Ordering::Relaxed) {
            return cancelled(&request.path);
        }

        // A short page is the last page: the driver stops early only when the
        // rows ran out.
        if order.is_empty() || count < BATCH {
            break;
        }
        offset += count;
    }

    let total_written = match writer {
        Some(w) => w.finish().map_err(|e| Error::Io(e.to_string()))?,
        // No pages at all should be impossible — a result set always reports
        // its columns — but an empty file is a better answer than a panic.
        None => written,
    };
    report(total_written, true);
    Ok(total_written)
}

/// Abandon a cancelled export, taking its half-written file with it.
///
/// A partial export left on disk is the dangerous outcome: it looks like a
/// complete file, and nothing about it says which rows are missing.
fn cancelled(path: &str) -> Result<u64> {
    let _ = std::fs::remove_file(path);
    Err(Error::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use indexmap::IndexMap;
    use std::sync::Mutex;
    use tablex_core::{
        config::TlsConfig,
        driver::{Connection, FetchOptions, RowEdit},
        result::{Column, QueryOutcome, ResultSet},
        schema::{SchemaNode, TableDetail},
        ConnectionConfig, Value,
    };

    /// A connection that serves a fixed number of rows, a page at a time.
    ///
    /// It records the statements it was asked to run, which is how the paging
    /// tests check that the export ordered its pages rather than trusting the
    /// engine to return them consistently.
    struct FakeTable {
        rows: usize,
        keyed: bool,
        statements: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Connection for FakeTable {
        async fn execute(&mut self, sql: &str, opts: &FetchOptions) -> Result<QueryOutcome> {
            self.statements.lock().unwrap().push(sql.to_string());

            let cap = opts.max_rows.unwrap_or(usize::MAX);
            let remaining = self.rows.saturating_sub(opts.offset);
            let count = remaining.min(cap);
            let rows = (0..count)
                .map(|i| vec![Value::Int((opts.offset + i) as i64)])
                .collect();

            Ok(QueryOutcome {
                statements: vec![StatementResult::Rows(ResultSet {
                    columns: vec![Column {
                        name: "id".into(),
                        type_name: "int".into(),
                        nullable: None,
                        source: None,
                    }],
                    rows,
                    truncated: false,
                    editable: false,
                    key_columns: vec![],
                })],
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
            table: &str,
        ) -> Result<TableDetail> {
            Ok(TableDetail {
                schema: None,
                name: table.to_string(),
                columns: vec![],
                indexes: vec![],
                foreign_keys: vec![],
                primary_key: if self.keyed {
                    vec!["id".to_string()]
                } else {
                    vec![]
                },
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

    /// State holding one session backed by `FakeTable`.
    async fn state_with(
        dir: &std::path::Path,
        rows: usize,
        keyed: bool,
    ) -> (AppState, Arc<Mutex<Vec<String>>>) {
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
                    keyed,
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
    async fn writes_every_row_across_page_boundaries() {
        let dir = temp_dir("paged");
        // Two full pages and a short one, so the loop has to notice both that a
        // full page is not the end and that a short page is.
        let rows = BATCH * 2 + 7;
        let (state, statements) = state_with(&dir, rows, true).await;

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
        // Header plus every row, and no row written twice.
        assert_eq!(text.lines().count(), rows + 1);
        assert_eq!(text.lines().next(), Some("id"));
        assert_eq!(text.lines().nth(1), Some("0"));
        assert_eq!(text.lines().last(), Some((rows - 1).to_string().as_str()));

        // Every page asked for a deterministic order. Without it, two pages can
        // overlap and a row goes missing with nothing to show for it.
        let seen = statements.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(
            seen.iter().all(|s| s.contains("ORDER BY \"id\"")),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn a_table_with_no_key_is_fetched_in_one_go() {
        let dir = temp_dir("unkeyed");
        let (state, statements) = state_with(&dir, BATCH * 2, false).await;

        let written = run(
            &state,
            request(&dir, "out.csv"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("export");

        assert_eq!(written, (BATCH * 2) as u64);
        let seen = statements.lock().unwrap().clone();
        // One statement, no ORDER BY: with no key there is nothing to order by,
        // and paging without an order can drop rows silently.
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(!seen[0].contains("ORDER BY"), "{seen:?}");
    }

    #[tokio::test]
    async fn progress_is_reported_before_the_first_page_and_after_the_last() {
        let dir = temp_dir("progress");
        let (state, _) = state_with(&dir, BATCH + 1, true).await;
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
        // The first report precedes the first query, because that query is the
        // part most likely to be slow - especially through a tunnel.
        assert_eq!(reports.first(), Some(&(0, false, Some((BATCH + 1) as u64))));
        assert_eq!(
            reports.last(),
            Some(&((BATCH + 1) as u64, true, Some((BATCH + 1) as u64)))
        );
        assert!(
            reports.len() >= 3,
            "one report per page at least: {reports:?}"
        );
    }

    #[tokio::test]
    async fn cancelling_stops_and_removes_the_partial_file() {
        let dir = temp_dir("cancelled");
        let (state, _) = state_with(&dir, BATCH * 3, true).await;
        let cancel = Arc::new(AtomicBool::new(false));

        // Cancel as soon as the first page has been written.
        let flag = cancel.clone();
        let err = run(&state, request(&dir, "out.csv"), cancel, move |p| {
            if p.rows > 0 {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .await
        .expect_err("cancelled");

        assert!(matches!(err, Error::Cancelled), "{err}");
        // A half-written export left on disk is the dangerous outcome: it looks
        // like a complete file and nothing about it says which rows are missing.
        assert!(
            !dir.join("out.csv").exists(),
            "partial file must not survive"
        );
    }
}
