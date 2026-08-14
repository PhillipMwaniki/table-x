//! The driver contract.
//!
//! Every supported database implements [`Driver`] (a factory, cheap and stateless)
//! and [`Connection`] (one live session). Keeping them separate lets the app list
//! and describe drivers without opening any sockets.

use crate::{
    config::ConnectionConfig,
    error::Result,
    result::{Column, QueryOutcome},
    schema::{SchemaNode, TableDetail},
    value::Value,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// What a driver can do. The UI reads this to hide affordances rather than
/// offering buttons that fail — no "Begin transaction" on a driver without one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub transactions: bool,
    /// Server-side cancellation of a running statement.
    pub cancel: bool,
    /// Multiple statements in one submission.
    pub multi_statement: bool,
    /// `EXPLAIN` or equivalent.
    pub explain: bool,
    pub schemas: bool,
    /// Whether one server holds several databases the user can move between.
    /// False for file-backed engines, where the connection *is* the database.
    pub databases: bool,
    pub foreign_keys: bool,
    pub views: bool,
    pub stored_procedures: bool,
    /// Whether `definition` can return a `CREATE` statement for a *table*.
    ///
    /// Routines and triggers are scriptable everywhere; tables are not.
    /// PostgreSQL has no catalogue function that renders one, and SQL Server's
    /// `OBJECT_DEFINITION` returns NULL for anything that is not a module. The
    /// menu hides the option rather than offering one that fails.
    pub table_scripts: bool,

    /// Whether the driver reports which table each result column came from.
    /// Without it, inline editing must stay disabled.
    pub column_provenance: bool,
    /// Whether streaming avoids buffering the whole result set in memory.
    pub streaming: bool,
    /// Placeholder syntax, so the query builder emits `$1` or `?` correctly.
    pub placeholder_style: PlaceholderStyle,
    /// Quote character(s) for identifiers.
    pub identifier_quote: char,
}

impl Default for Capabilities {
    /// Conservative defaults: a new driver advertises nothing until it proves it.
    fn default() -> Self {
        Capabilities {
            transactions: false,
            cancel: false,
            multi_statement: false,
            explain: false,
            schemas: false,
            databases: false,
            foreign_keys: false,
            views: false,
            stored_procedures: false,
            table_scripts: false,
            column_provenance: false,
            streaming: false,
            placeholder_style: PlaceholderStyle::Question,
            identifier_quote: '"',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderStyle {
    /// `?` — MySQL, SQLite.
    Question,
    /// `$1`, `$2` — PostgreSQL.
    Dollar,
    /// `@p1`, `@p2` — SQL Server.
    AtP,
}

/// Static description of a driver, used to render the "new connection" form
/// without instantiating anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverInfo {
    /// Stable machine id, e.g. `postgres`.
    pub id: String,
    /// Display name, e.g. `PostgreSQL`.
    pub name: String,
    pub default_port: Option<u16>,
    /// True for embedded databases that take a file path instead of host/port.
    pub file_based: bool,
    pub capabilities: Capabilities,
}

/// Receives a result set in pieces as a driver reads it.
///
/// Both methods return [`Result`], and that is the point: returning an error
/// stops the stream at the next batch. It is how an export that the user
/// cancelled stops without the driver knowing anything about cancellation, and
/// how a full disk stops one without reading the rest of the table first.
pub trait RowSink: Send {
    /// Called once, before any rows. A driver knows its columns only after the
    /// server has answered, which is why this is not a constructor argument.
    fn columns(&mut self, columns: &[Column]) -> Result<()>;

    /// Called repeatedly, with whatever the driver has read since last time.
    fn rows(&mut self, rows: &[Vec<Value>]) -> Result<()>;
}

/// Rows a streaming driver gathers before handing them over.
///
/// Small enough that memory stays flat and progress moves visibly; large enough
/// that the per-batch overhead disappears against the cost of reading rows.
pub const STREAM_BATCH: usize = 1_000;

/// How many rows to fetch, and from where. The row cap is what keeps a careless
/// `SELECT * FROM events` from pulling 40 million rows into memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOptions {
    /// Maximum rows to materialize. `None` means unlimited — used by exports,
    /// never by the grid.
    pub max_rows: Option<usize>,
    /// Offset for paged fetches.
    #[serde(default)]
    pub offset: usize,
    /// Abort the statement after this many seconds.
    pub timeout_secs: Option<u64>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            max_rows: Some(1_000),
            offset: 0,
            timeout_secs: Some(60),
        }
    }
}

/// A single cell edit, applied as a targeted `UPDATE`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowEdit {
    pub schema: Option<String>,
    pub table: String,
    /// Column-name → new value.
    pub changes: Vec<(String, Value)>,
    /// Column-name → original value, forming the `WHERE` clause. Sending the
    /// full original key (rather than a row index) is what makes the update
    /// safe against concurrent modification.
    pub key: Vec<(String, Value)>,
}

/// A database driver: a stateless factory for connections.
#[async_trait]
pub trait Driver: Send + Sync {
    fn info(&self) -> DriverInfo;

    /// Open a session. Any SSH tunnel has already been established by the caller,
    /// and `config` points at the local end of it.
    async fn connect(
        &self,
        config: &ConnectionConfig,
        secret: Option<&str>,
    ) -> Result<Box<dyn Connection>>;
}

/// One live database session.
///
/// `&mut self` throughout: a session is not safe to use concurrently, and the
/// borrow checker enforces that the connection registry serializes access
/// instead of leaving it to convention.
#[async_trait]
pub trait Connection: Send + Sync {
    /// Execute user SQL and materialize results subject to `opts`.
    async fn execute(&mut self, sql: &str, opts: &FetchOptions) -> Result<QueryOutcome>;

    /// Children of `parent`, or the roots when `parent` is `None`. One level only —
    /// the tree is expanded lazily.
    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>>;

    /// Full structure of one table.
    async fn table_detail(&mut self, schema: Option<&str>, table: &str) -> Result<TableDetail>;

    /// Apply an inline grid edit. Implementations must verify exactly one row was
    /// affected and roll back otherwise.
    async fn apply_edit(&mut self, edit: &RowEdit) -> Result<()>;

    /// Cheap liveness check used before reusing a pooled session.
    async fn ping(&mut self) -> Result<()>;

    /// Close cleanly. Best-effort: a dropped connection must not leak either way.
    async fn close(&mut self) -> Result<()>;

    /// Identifiers for autocomplete, gathered once per connection.
    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        Ok(CompletionScope::default())
    }

    /// The database this session is currently pointed at, when the engine has
    /// such a concept.
    async fn current_database(&mut self) -> Result<Option<String>> {
        Ok(None)
    }

    /// Read a result set in pieces, handing each piece to `sink` as it arrives.
    ///
    /// This is what an export uses. `execute` materializes a whole result set,
    /// which is right for the grid — it shows a capped page and needs it all at
    /// once — and wrong for writing a hundred million rows to a file.
    ///
    /// The default implementation calls `execute` and pushes the result as a
    /// single batch, so a driver that has not implemented streaming still works,
    /// just without the memory bound. `Capabilities::streaming` says which is
    /// which.
    ///
    /// Returns the number of rows handed to the sink.
    async fn stream(
        &mut self,
        sql: &str,
        opts: &FetchOptions,
        sink: &mut dyn RowSink,
    ) -> Result<u64> {
        let outcome = self.execute(sql, opts).await?;
        let Some(crate::result::StatementResult::Rows(set)) = outcome.statements.into_iter().next()
        else {
            return Err(crate::error::Error::Unsupported(
                "that statement returned no rows to stream".into(),
            ));
        };
        sink.columns(&set.columns)?;
        sink.rows(&set.rows)?;
        Ok(set.rows.len() as u64)
    }

    /// The statement that would recreate the object at `node_id`.
    ///
    /// Takes a tree path rather than a name and a kind: the driver built that
    /// path, so it already knows which catalogue to ask and how many levels its
    /// engine qualifies by. The text comes back as the server stores it, which
    /// is what makes it worth editing rather than reading — a re-rendered
    /// approximation of someone's procedure is not something to run.
    async fn definition(&mut self, _node_id: &str) -> Result<String> {
        Err(crate::error::Error::Unsupported(
            "this driver cannot show object definitions".into(),
        ))
    }

    /// Point this session at another database on the same server.
    ///
    /// Engines that can do it in-session (`USE`, or a default that is just a
    /// request parameter) implement this. PostgreSQL cannot: a connection is
    /// bound to one database for its lifetime, so its driver leaves this
    /// unsupported and the app layer reconnects instead. Returning an error
    /// here is what tells it to.
    async fn use_database(&mut self, _database: &str) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "this driver cannot switch database on an open connection".into(),
        ))
    }
}

/// Identifiers offered by the SQL editor's autocomplete.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletionScope {
    pub schemas: Vec<String>,
    /// Qualified table name → its column names.
    pub tables: Vec<(String, Vec<String>)>,
    pub functions: Vec<String>,
    pub keywords: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_to_nothing_supported() {
        // A driver must opt in to every feature, so a half-finished driver
        // cannot accidentally advertise transactions it does not implement.
        let c = Capabilities::default();
        assert!(!c.transactions);
        assert!(!c.cancel);
        assert!(!c.column_provenance);
        assert!(!c.streaming);
    }

    #[test]
    fn fetch_defaults_are_capped() {
        // The default must never be unlimited: the grid uses it for every query.
        let o = FetchOptions::default();
        assert_eq!(o.max_rows, Some(1_000));
        assert!(o.timeout_secs.is_some());
    }
}
