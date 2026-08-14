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

use std::io::BufWriter;
use tablex_core::{
    driver::FetchOptions,
    error::{Error, Result},
    export::{Format, Writer},
    result::StatementResult,
};

use crate::state::AppState;

/// Rows per round trip. Large enough that the per-statement overhead disappears,
/// small enough that a page of wide rows is not itself a memory problem.
const BATCH: usize = 5_000;

pub struct ExportRequest {
    pub connection_id: String,
    /// The table's name as SQL should refer to it, quoted by the driver.
    pub qualified: String,
    pub schema: Option<String>,
    pub table: String,
    pub format: Format,
    pub path: String,
}

/// Write a table to a file, returning how many rows were written.
pub async fn run(state: &AppState, request: ExportRequest) -> Result<u64> {
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
    let order = match guard
        .table_detail(request.schema.as_deref(), &request.table)
        .await
    {
        Ok(detail) => detail.edit_key(),
        // A view, or a table the catalogue will not describe: still exportable,
        // just not pageable.
        Err(_) => Vec::new(),
    };

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

        // A short page is the last page: the driver stops early only when the
        // rows ran out.
        if order.is_empty() || count < BATCH {
            break;
        }
        offset += count;
    }

    match writer {
        Some(w) => w.finish().map_err(|e| Error::Io(e.to_string())),
        // No pages at all should be impossible — a result set always reports
        // its columns — but an empty file is a better answer than a panic.
        None => Ok(written),
    }
}
