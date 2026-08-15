//! Reading a whole schema's structure, so two of them can be compared.
//!
//! Built from the same `table_detail` the structure view uses rather than from
//! five new bulk queries. That is slower — one round trip per table — and it is
//! the right trade here: two code paths describing the same table would
//! eventually disagree, and a schema comparison that is wrong is worse than one
//! that takes a moment. The table *list* still comes from the bulk
//! `schema_graph` query, so the per-table cost is the only cost.
//!
//! Progress is reported per table, because on a large schema over a tunnel this
//! is long enough that silence looks like a hang.

use crate::export::Progress;
use crate::state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tablex_core::{
    diff::SchemaSnapshot,
    error::{Error, Result},
};

/// Read every table's structure in one schema.
pub async fn capture(
    state: &AppState,
    id: &str,
    connection_id: &str,
    schema: Option<&str>,
    label: String,
    cancel: &Arc<AtomicBool>,
    on_progress: &(impl Fn(Progress) + Sync),
) -> Result<SchemaSnapshot> {
    let session = state.sessions.get(connection_id).await?;
    let mut guard = session.connection.lock().await;

    let graph = guard.schema_graph(schema).await?;
    let total = graph.tables.len() as u64;

    let report = |done: u64, finished: bool| {
        on_progress(Progress {
            id: id.to_string(),
            label: label.clone(),
            unit: "tables".into(),
            rows: done,
            total: Some(total),
            done: finished,
        });
    };
    report(0, false);

    let mut tables = Vec::with_capacity(graph.tables.len());
    for (index, table) in graph.tables.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // A table that vanished between the list and the read is skipped rather
        // than failing the comparison: it is a real thing that happens on a
        // live schema, and one missing table is better than no answer.
        if let Ok(detail) = guard
            .table_detail(table.schema.as_deref(), &table.name)
            .await
        {
            tables.push(detail);
        }
        report(index as u64 + 1, false);
    }

    report(total, true);
    Ok(SchemaSnapshot { label, tables })
}
