//! SQLite driver.
//!
//! rusqlite is synchronous, so every call runs on Tokio's blocking pool with the
//! connection behind a mutex. An actor thread would also work, but the mutex keeps
//! the borrow pattern obvious and SQLite serializes writes internally regardless.

mod types;

use async_trait::async_trait;
use rusqlite::OptionalExtension;
use std::sync::{Arc, Mutex};
use tablex_core::{
    config::ConnectionConfig,
    diagram::{GraphTable, SchemaGraph},
    driver::{
        CancelHandle, Capabilities, CompletionScope, Connection, DdlSupport, Driver, DriverInfo,
        FetchOptions, PlaceholderStyle, RowDelete, RowEdit, RowInsert, RowSink, TxStatements,
        STREAM_BATCH,
    },
    error::{Error, Result},
    plan::{Plan, PlanRow},
    result::{Column, ColumnSource, QueryOutcome, ResultSet, StatementResult},
    schema::{decode_path, ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
};
use types::Affinity;

/// SQLite's own spelling. `END` is a synonym for `COMMIT` here, but the
/// explicit word is what the rest of the application scans for.
pub(crate) const TX: TxStatements = TxStatements {
    begin: "BEGIN",
    commit: "COMMIT",
    rollback: "ROLLBACK",
};

const QUOTE: char = '"';

pub struct SqliteDriver;

impl SqliteDriver {
    pub fn new() -> Self {
        SqliteDriver
    }
}

impl Default for SqliteDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "sqlite".into(),
            name: "SQLite".into(),
            default_port: None,
            file_based: true,
            capabilities: Capabilities {
                // Add, drop and index only. SQLite has no ALTER COLUMN and no
                // ADD CONSTRAINT: changing a type or attaching a foreign key
                // means building a new table, copying the rows across and
                // renaming, which is a procedure rather than a statement and
                // is not something to do behind a checkbox.
                ddl: DdlSupport {
                    add_column: true,
                    drop_column: true,
                    alter_column: false,
                    indexes: true,
                    foreign_keys: false,
                    transactional_ddl: true,
                },
                transactions: true,
                multi_statement: true,
                explain: true,
                explain_analyze: false,
                foreign_keys: true,
                views: true,
                // SQLite has no schemas; attached databases are a different
                // concept and are not modelled as one.
                schemas: false,
                // A SQLite connection is a file, and the file is the database.
                // ATTACH exists but is a session-local alias, not a catalogue to
                // browse, so there is nothing here to switch between.
                databases: false,
                // Available because this build compiles in
                // SQLITE_ENABLE_COLUMN_METADATA — see the rusqlite features in
                // Cargo.toml. Columns that are expressions rather than stored
                // values still report no origin, which is exactly right.
                // sqlite_master keeps the original CREATE text for every object.
                table_scripts: true,
                // The cursor is stepped a row at a time; nothing here holds a
                // whole result set.
                streaming: true,
                activity: false,
                privileges: false,
                column_provenance: true,
                stored_procedures: false,
                cancel: true,
                placeholder_style: PlaceholderStyle::Question,
                identifier_quote: QUOTE,
            },
        }
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        _secret: Option<&str>,
    ) -> Result<Box<dyn Connection>> {
        let path = config
            .file_path
            .clone()
            .ok_or_else(|| Error::Config("SQLite requires a database file path".into()))?;
        let read_only = config.read_only;

        let conn = tokio::task::spawn_blocking(move || {
            use rusqlite::OpenFlags;
            let mut flags = OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            if read_only {
                flags |= OpenFlags::SQLITE_OPEN_READ_ONLY;
            } else {
                flags |= OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            }
            rusqlite::Connection::open_with_flags(&path, flags)
        })
        .await
        .map_err(|e| Error::Other(e.to_string()))?
        .map_err(map_err)?;

        // Enforce foreign keys. SQLite defaults them OFF for backwards
        // compatibility, which means edits can silently orphan rows.
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(map_err)?;

        // Taken before the connection is locked away: `get_interrupt_handle`
        // needs `&Connection`, and the whole point of the handle is to be
        // reachable while something else holds that lock.
        let interrupt = Arc::new(conn.get_interrupt_handle());

        Ok(Box::new(SqliteConnection {
            conn: Arc::new(Mutex::new(conn)),
            interrupt,
        }))
    }
}

pub struct SqliteConnection {
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// Interrupts whatever statement is running, from any thread.
    interrupt: Arc<rusqlite::InterruptHandle>,
}

/// Stops the current statement by asking SQLite to abandon it.
///
/// `sqlite3_interrupt` is safe to call from another thread and takes effect at
/// the next point the running statement checks — which for the scans and sorts
/// that make a query slow enough to want cancelling is very soon.
struct SqliteCancel(Arc<rusqlite::InterruptHandle>);

#[async_trait]
impl CancelHandle for SqliteCancel {
    async fn cancel(&self) -> Result<()> {
        self.0.interrupt();
        Ok(())
    }
}

impl SqliteConnection {
    /// Run a closure against the connection on the blocking pool.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| Error::Other("SQLite connection mutex poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| Error::Other(format!("blocking task failed: {e}")))?
    }
}

#[async_trait]
impl Connection for SqliteConnection {
    async fn execute(&mut self, sql: &str, opts: &FetchOptions) -> Result<QueryOutcome> {
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Err(Error::query("no statement to execute"));
        }
        let max_rows = opts.max_rows;
        let offset = opts.offset;

        self.with_conn(move |conn| {
            let started = std::time::Instant::now();
            let mut results = Vec::with_capacity(statements.len());

            for stmt_sql in &statements {
                results.push(run_one(conn, stmt_sql, max_rows, offset)?);
            }

            Ok(QueryOutcome {
                statements: results,
                elapsed_ms: started.elapsed().as_millis() as u64,
                notices: Vec::new(),
            })
        })
        .await
    }

    /// A file is one database, so the tree starts at the object folders rather
    /// than at a database level that would always have exactly one entry.
    ///
    /// Paths are `[folder]`, `[folder, object]`, `[folder, object, column]`.
    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
        let path = parent.map(decode_path).unwrap_or_default();
        self.with_conn(move |conn| match path.as_slice() {
            [] => Ok(FOLDERS
                .iter()
                .map(|f| SchemaNode::new(&[f.id], f.label, NodeKind::Folder).expandable())
                .collect()),

            [folder] => {
                let Some(spec) = FOLDERS.iter().find(|f| f.id == folder) else {
                    return Ok(Vec::new());
                };
                browse_folder(conn, spec)
            }

            // Only objects with columns expand further; a trigger or an index
            // has none, and is reported as a leaf above.
            [folder, object] => Ok(table_columns(conn, object)?
                .into_iter()
                .map(|c| {
                    SchemaNode::new(&[folder, object, &c.name], &c.name, NodeKind::Column).detail(
                        if c.nullable {
                            c.type_name.clone()
                        } else {
                            format!("{} NOT NULL", c.type_name)
                        },
                    )
                })
                .collect()),

            _ => Ok(Vec::new()),
        })
        .await
    }

    async fn current_database(&mut self) -> Result<Option<String>> {
        // The file path is the closest thing to a database name here, and the
        // UI already shows it. Reporting none keeps the database switcher off.
        Ok(None)
    }

    /// Every table, and the keys between them.
    ///
    /// SQLite has no catalog view of foreign keys — `PRAGMA foreign_key_list`
    /// answers for one table at a time and there is no bulk form. That is a
    /// query per table, which on a server engine would be the thing this method
    /// exists to avoid; here the database is a local file and the whole loop
    /// costs less than one network round trip would.
    async fn schema_graph(&mut self, _schema: Option<&str>) -> Result<SchemaGraph> {
        self.with_conn(move |conn| {
            let names: Vec<String> = conn
                .prepare(
                    "SELECT name FROM sqlite_master \
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(map_err)?
                .query_map([], |r| r.get(0))
                .map_err(map_err)?
                .collect::<std::result::Result<_, _>>()
                .map_err(map_err)?;

            let tables = names
                .into_iter()
                .map(|name| {
                    let foreign_keys = table_foreign_keys(conn, &name)?;
                    Ok(GraphTable {
                        schema: None,
                        name,
                        foreign_keys,
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(SchemaGraph { tables })
        })
        .await
    }

    /// `EXPLAIN QUERY PLAN` — rows carrying their own parent pointers.
    ///
    /// SQLite prints no costs and no row estimates, and this does not invent
    /// any. What it does print is a readable sentence per step — `SCAN users`,
    /// `SEARCH orders USING INDEX ix_user (user_id=?)` — which is kept whole
    /// rather than split into a label and a detail. Splitting it would mean
    /// guessing where the verb ends, and every guess reads worse than the
    /// sentence SQLite already wrote.
    async fn explain(&mut self, sql: &str, _analyze: bool) -> Result<Plan> {
        let statement = format!("EXPLAIN QUERY PLAN {sql}");
        let rows = self
            .with_conn(move |conn| {
                let mut stmt = conn.prepare(&statement).map_err(map_err)?;
                let mapped = stmt
                    .query_map([], |row| {
                        Ok(PlanRow {
                            id: row.get(0)?,
                            parent: row.get(1)?,
                            label: row.get::<_, String>(3)?,
                            detail: None,
                            rows: None,
                            cost: None,
                        })
                    })
                    .map_err(map_err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(map_err)?;
                Ok(mapped)
            })
            .await?;

        let raw = rows
            .iter()
            .map(|r| format!("{}|{}|{}", r.id, r.parent, r.label))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Plan {
            root: tablex_core::plan::from_parent_rows(rows, "Query"),
            analyzed: false,
            raw,
        })
    }

    /// Stream rows straight off the statement.
    ///
    /// SQLite steps a cursor one row at a time, so the only thing that ever
    /// held a whole result set in memory was this driver collecting it.
    ///
    /// rusqlite is synchronous, so the stepping happens on a blocking thread
    /// and batches come back over a channel of capacity one. That capacity is
    /// the point: the reader cannot run ahead of the writer, so a fast database
    /// and a slow disk cannot pile rows up in between — which is the unbounded
    /// buffer this whole exercise exists to remove.
    async fn stream(
        &mut self,
        sql: &str,
        opts: &FetchOptions,
        sink: &mut dyn RowSink,
    ) -> Result<u64> {
        let conn = Arc::clone(&self.conn);
        let sql = sql.to_string();
        let max_rows = opts.max_rows;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Chunk>(1);

        let worker = tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| Error::Other("SQLite connection mutex poisoned".into()))?;
            stream_rows(&guard, &sql, max_rows, &tx)
        });

        let mut total = 0u64;
        let mut failure: Option<Error> = None;
        while let Some(chunk) = rx.recv().await {
            let result = match chunk {
                Chunk::Columns(columns) => sink.columns(&columns),
                Chunk::Rows(rows) => {
                    total += rows.len() as u64;
                    sink.rows(&rows)
                }
            };
            if let Err(e) = result {
                // Dropping the receiver makes the next send fail, which is how
                // the worker learns to stop reading a table nobody wants.
                failure = Some(e);
                break;
            }
        }
        drop(rx);

        let worker = worker
            .await
            .map_err(|e| Error::Other(format!("sqlite worker failed: {e}")))?;

        match failure {
            // The sink's failure is the real one: the worker's is only ever
            // "the receiver went away", which is what the sink caused.
            Some(e) => Err(e),
            // The count the sink accepted, not the count the worker sent —
            // they agree, and this is the one the caller can act on.
            None => worker.map(|_| total),
        }
    }

    /// SQLite stores the original text of every object in `sqlite_master`, so
    /// this is the statement the user actually wrote, comments and all.
    async fn definition(&mut self, node_id: &str) -> Result<String> {
        let path = decode_path(node_id);
        let [_folder, name] = path.as_slice() else {
            return Err(Error::Unsupported(
                "only an object has a definition to show".into(),
            ));
        };
        let name = name.clone();

        self.with_conn(move |conn| {
            let sql: Option<Option<String>> = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name = ?1",
                    [&name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_err)?;

            match sql.flatten() {
                Some(text) => Ok(text),
                // Auto-created indexes for UNIQUE and PRIMARY KEY constraints
                // have a NULL sql: they are declared by the table, not by a
                // statement of their own.
                None => Err(Error::Unsupported(format!(
                    "{name} has no statement of its own — it was created implicitly by its table"
                ))),
            }
        })
        .await
    }

    async fn table_detail(&mut self, _schema: Option<&str>, table: &str) -> Result<TableDetail> {
        let table = table.to_string();
        self.with_conn(move |conn| {
            let columns = table_columns(conn, &table)?;
            let primary_key = primary_key(conn, &table);

            Ok(TableDetail {
                schema: None,
                name: table.clone(),
                columns,
                indexes: table_indexes(conn, &table)?,
                foreign_keys: table_foreign_keys(conn, &table)?,
                primary_key,
                estimated_rows: None,
                comment: None,
            })
        })
        .await
    }

    async fn apply_edit(&mut self, edit: &RowEdit) -> Result<()> {
        if edit.changes.is_empty() {
            return Ok(());
        }
        if edit.key.is_empty() {
            // Without a key the WHERE clause would match every row.
            return Err(Error::Unsupported(
                "cannot edit a row that has no unique key".into(),
            ));
        }
        let edit = edit.clone();

        self.with_conn(move |conn| {
            let assignments = edit
                .changes
                .iter()
                .map(|(c, _)| format!("{} = ?", quote_ident(c, QUOTE)))
                .collect::<Vec<_>>()
                .join(", ");

            // NULL never equals NULL, so a key column that is NULL needs IS NULL
            // rather than = ?. Getting this wrong silently matches zero rows.
            let predicate = edit
                .key
                .iter()
                .map(|(c, v)| {
                    if v.is_null() {
                        format!("{} IS NULL", quote_ident(c, QUOTE))
                    } else {
                        format!("{} = ?", quote_ident(c, QUOTE))
                    }
                })
                .collect::<Vec<_>>()
                .join(" AND ");

            let sql = format!(
                "UPDATE {} SET {} WHERE {}",
                quote_ident(&edit.table, QUOTE),
                assignments,
                predicate
            );

            let mut params: Vec<rusqlite::types::Value> =
                edit.changes.iter().map(|(_, v)| types::to_sql(v)).collect();
            params.extend(
                edit.key
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(_, v)| types::to_sql(v)),
            );

            let tx = conn.transaction().map_err(map_err)?;
            let affected = tx
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(map_err)?;

            // The guard that makes inline editing safe: if the row moved or was
            // deleted concurrently, or if the key was not actually unique, the
            // count will not be 1 and nothing is committed.
            if affected != 1 {
                tx.rollback().map_err(map_err)?;
                return Err(Error::Query {
                    message: format!(
                        "edit matched {affected} rows, expected exactly 1 — \
                         the row may have changed since it was loaded"
                    ),
                    position: None,
                    code: None,
                });
            }
            tx.commit().map_err(map_err)?;
            Ok(())
        })
        .await
    }

    async fn insert_row(&mut self, insert: &RowInsert) -> Result<()> {
        if insert.values.is_empty() {
            // `INSERT INTO t DEFAULT VALUES` is a different statement with
            // different failure modes; a form with nothing filled in is a
            // mistake, not a request for it.
            return Err(Error::Unsupported(
                "an inserted row needs at least one value".into(),
            ));
        }
        let insert = insert.clone();

        self.with_conn(move |conn| {
            let columns = insert
                .values
                .iter()
                .map(|(c, _)| quote_ident(c, QUOTE))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = vec!["?"; insert.values.len()].join(", ");

            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote_ident(&insert.table, QUOTE),
                columns,
                placeholders
            );

            let params: Vec<rusqlite::types::Value> = insert
                .values
                .iter()
                .map(|(_, v)| types::to_sql(v))
                .collect();

            conn.execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(map_err)?;
            Ok(())
        })
        .await
    }

    async fn delete_row(&mut self, delete: &RowDelete) -> Result<()> {
        if delete.key.is_empty() {
            // Without a key the WHERE clause would match every row, and this
            // statement does not get a second chance.
            return Err(Error::Unsupported(
                "cannot delete a row that has no unique key".into(),
            ));
        }
        let delete = delete.clone();

        self.with_conn(move |conn| {
            // NULL never equals NULL, so a key column that is NULL needs
            // IS NULL — the same trap as an edit, with a worse outcome.
            let predicate = delete
                .key
                .iter()
                .map(|(c, v)| {
                    if v.is_null() {
                        format!("{} IS NULL", quote_ident(c, QUOTE))
                    } else {
                        format!("{} = ?", quote_ident(c, QUOTE))
                    }
                })
                .collect::<Vec<_>>()
                .join(" AND ");

            let sql = format!(
                "DELETE FROM {} WHERE {}",
                quote_ident(&delete.table, QUOTE),
                predicate
            );

            let params: Vec<rusqlite::types::Value> = delete
                .key
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(_, v)| types::to_sql(v))
                .collect();

            let tx = conn.transaction().map_err(map_err)?;
            let affected = tx
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(map_err)?;

            // In a transaction on purpose: a delete that matched two rows has
            // already destroyed one too many by the time the count is read.
            if affected != 1 {
                tx.rollback().map_err(map_err)?;
                return Err(Error::Query {
                    message: format!(
                        "delete matched {affected} rows, expected exactly 1 —                          the row may have changed since it was loaded"
                    ),
                    position: None,
                    code: None,
                });
            }
            tx.commit().map_err(map_err)?;
            Ok(())
        })
        .await
    }

    fn cancel_handle(&self) -> Option<Arc<dyn CancelHandle>> {
        Some(Arc::new(SqliteCancel(Arc::clone(&self.interrupt))))
    }

    fn transaction_statements(&self) -> Option<TxStatements> {
        Some(TX)
    }

    async fn ping(&mut self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch("SELECT 1").map_err(map_err)?;
            Ok(())
        })
        .await
    }

    async fn close(&mut self) -> Result<()> {
        // rusqlite closes on drop; nothing to flush.
        Ok(())
    }

    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type IN ('table','view') \
                     AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(map_err)?;
            let names: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(map_err)?
                .collect::<std::result::Result<_, _>>()
                .map_err(map_err)?;

            let mut tables = Vec::with_capacity(names.len());
            for name in names {
                let cols = table_columns(conn, &name)?
                    .into_iter()
                    .map(|c| c.name)
                    .collect();
                tables.push((name, cols));
            }

            Ok(CompletionScope {
                schemas: Vec::new(),
                tables,
                functions: SQLITE_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
                keywords: Vec::new(),
            })
        })
        .await
    }
}

/// What the blocking reader sends back.
enum Chunk {
    Columns(Vec<Column>),
    Rows(Vec<Vec<tablex_core::Value>>),
}

/// Step a statement, sending a batch at a time.
///
/// A failed send means the receiver is gone — the consumer stopped, or errored
/// — so reading stops there rather than finishing a table nobody is reading.
fn stream_rows(
    conn: &rusqlite::Connection,
    sql: &str,
    max_rows: Option<usize>,
    tx: &tokio::sync::mpsc::Sender<Chunk>,
) -> Result<u64> {
    let mut stmt = conn.prepare(sql).map_err(map_err)?;
    if stmt.column_count() == 0 {
        return Err(Error::Unsupported(
            "that statement returns no rows to stream".into(),
        ));
    }

    let columns = describe(&stmt);
    let affinities = affinities_of(&columns);
    if tx.blocking_send(Chunk::Columns(columns.clone())).is_err() {
        return Ok(0);
    }

    let mut rows = stmt.query([]).map_err(map_err)?;
    let mut batch: Vec<Vec<tablex_core::Value>> = Vec::with_capacity(STREAM_BATCH);
    let mut total = 0u64;

    while let Some(row) = rows.next().map_err(map_err)? {
        if max_rows.is_some_and(|cap| total as usize >= cap) {
            break;
        }
        let mut values = Vec::with_capacity(columns.len());
        for (i, col) in columns.iter().enumerate() {
            let raw = row.get_ref(i).map_err(map_err)?;
            values.push(types::decode(raw, affinities[i], &col.type_name));
        }
        batch.push(values);
        total += 1;

        if batch.len() >= STREAM_BATCH {
            if tx
                .blocking_send(Chunk::Rows(std::mem::take(&mut batch)))
                .is_err()
            {
                return Ok(total);
            }
            batch = Vec::with_capacity(STREAM_BATCH);
        }
    }

    if !batch.is_empty() {
        let _ = tx.blocking_send(Chunk::Rows(batch));
    }
    Ok(total)
}

/// Column metadata for a prepared statement.
fn describe(stmt: &rusqlite::Statement<'_>) -> Vec<Column> {
    let decls = stmt.columns();
    stmt.columns_with_metadata()
        .into_iter()
        .enumerate()
        .map(|(i, meta)| Column {
            name: meta.name().to_string(),
            type_name: decls
                .get(i)
                .and_then(|c| c.decl_type())
                .unwrap_or("")
                .to_string(),
            nullable: None,
            source: meta
                .table_name()
                .zip(meta.origin_name())
                .map(|(t, c)| ColumnSource {
                    schema: None,
                    table: t.to_string(),
                    column: c.to_string(),
                }),
        })
        .collect()
}

/// Type affinities, computed once rather than per cell.
fn affinities_of(columns: &[Column]) -> Vec<Affinity> {
    columns
        .iter()
        .map(|c| {
            Affinity::from_decltype(if c.type_name.is_empty() {
                None
            } else {
                Some(&c.type_name)
            })
        })
        .collect()
}

/// Execute one statement and shape its result.
fn run_one(
    conn: &rusqlite::Connection,
    sql: &str,
    max_rows: Option<usize>,
    offset: usize,
) -> Result<StatementResult> {
    let mut stmt = conn.prepare(sql).map_err(map_err)?;

    // A statement with zero result columns is a write or DDL; anything else
    // returns rows. This is more reliable than sniffing the leading keyword,
    // which breaks on CTEs, PRAGMA, and INSERT ... RETURNING.
    if stmt.column_count() == 0 {
        drop(stmt);
        let affected = conn.execute(sql, []).map_err(map_err)?;
        return Ok(StatementResult::Affected {
            rows_affected: affected as u64,
            last_insert_id: Some(conn.last_insert_rowid()),
        });
    }

    // Declared types and origins come from two different rusqlite calls over the
    // same prepared statement, zipped by column position.
    let columns: Vec<Column> = {
        let decls = stmt.columns();
        stmt.columns_with_metadata()
            .into_iter()
            .enumerate()
            .map(|(i, meta)| Column {
                name: meta.name().to_string(),
                type_name: decls
                    .get(i)
                    .and_then(|c| c.decl_type())
                    .unwrap_or("")
                    .to_string(),
                nullable: None,
                // SQLite reports an origin only for columns that are stored
                // values. An expression, a literal, or an aggregate has none,
                // which is what keeps computed columns out of an UPDATE.
                //
                // Through a view this is the *base* table, which is the honest
                // answer: that is where the value actually lives, and it is the
                // row an edit has to reach. A view over a join reports several
                // tables and so stays read-only.
                source: meta.table_name().zip(meta.origin_name()).map(|(t, c)| {
                    ColumnSource {
                        // Attached databases are not modelled as schemas here,
                        // so the database name is deliberately dropped.
                        schema: None,
                        table: t.to_string(),
                        column: c.to_string(),
                    }
                }),
            })
            .collect()
    };

    // An edit needs a key as well as an origin: without one the UPDATE's WHERE
    // clause cannot address exactly one row.
    let key_columns = edit_key_for(conn, &columns);

    // Precompute affinities once rather than per cell.
    let affinities: Vec<Affinity> = columns
        .iter()
        .map(|c| {
            Affinity::from_decltype(if c.type_name.is_empty() {
                None
            } else {
                Some(&c.type_name)
            })
        })
        .collect();

    let mut rows_out: Vec<Vec<tablex_core::Value>> = Vec::new();
    let mut truncated = false;
    let mut seen = 0usize;

    let mut rows = stmt.query([]).map_err(map_err)?;
    while let Some(row) = rows.next().map_err(map_err)? {
        if seen < offset {
            seen += 1;
            continue;
        }
        if let Some(cap) = max_rows {
            if rows_out.len() >= cap {
                // Stop at the cap but record that more exist, so the UI shows
                // "load more" instead of implying the result was complete.
                truncated = true;
                break;
            }
        }
        let mut values = Vec::with_capacity(columns.len());
        for (i, col) in columns.iter().enumerate() {
            let raw = row.get_ref(i).map_err(map_err)?;
            values.push(types::decode(raw, affinities[i], &col.type_name));
        }
        rows_out.push(values);
        seen += 1;
    }

    let mut rs = ResultSet {
        columns,
        rows: rows_out,
        truncated,
        editable: false,
        key_columns,
    };
    rs.recompute_editable();
    Ok(StatementResult::Rows(rs))
}

/// One folder in the object tree, and the `sqlite_master` rows it holds.
struct Folder {
    id: &'static str,
    label: &'static str,
    /// `sqlite_master.type` values that belong here.
    types: &'static str,
    kind: NodeKind,
    /// Whether its objects expand to show columns.
    expandable: bool,
}

/// Only what SQLite has. There is no stored-procedure or user-function catalog
/// to browse — functions are registered by the host program, not the database —
/// so no empty folder is offered for them.
const FOLDERS: &[Folder] = &[
    Folder {
        id: "tables",
        label: "Tables",
        types: "'table'",
        kind: NodeKind::Table,
        expandable: true,
    },
    Folder {
        id: "views",
        label: "Views",
        types: "'view'",
        kind: NodeKind::View,
        expandable: true,
    },
    Folder {
        id: "triggers",
        label: "Triggers",
        types: "'trigger'",
        kind: NodeKind::Trigger,
        expandable: false,
    },
    Folder {
        id: "indexes",
        label: "Indexes",
        types: "'index'",
        kind: NodeKind::Index,
        expandable: false,
    },
];

fn browse_folder(conn: &rusqlite::Connection, spec: &Folder) -> Result<Vec<SchemaNode>> {
    // `tbl_name` is what a trigger or an index is attached to, which is the one
    // piece of context that makes those lists readable.
    let sql = format!(
        "SELECT name, tbl_name FROM sqlite_master \
         WHERE type IN ({}) AND name NOT LIKE 'sqlite_%' ORDER BY name",
        spec.types
    );
    let mut stmt = conn.prepare(&sql).map_err(map_err)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(map_err)?;

    let mut nodes = Vec::new();
    for row in rows {
        let (name, owner) = row.map_err(map_err)?;
        let mut node = SchemaNode::new(&[spec.id, &name], &name, spec.kind.clone())
            .qualified(quote_ident(&name, QUOTE));
        if spec.expandable {
            node = node.expandable();
        } else if owner != name {
            node = node.detail(format!("on {owner}"));
        }
        nodes.push(node);
    }
    Ok(nodes)
}

/// The key an inline edit would use, or empty when the result cannot be edited.
///
/// Two conditions, both necessary. Every column that has an origin must share
/// one source table, since an UPDATE has a single target. And the whole primary
/// key must appear in the projection under its own name: the WHERE clause is
/// built from the key values the user can see, so a key that was aliased away or
/// never selected cannot be sent back.
fn edit_key_for(conn: &rusqlite::Connection, columns: &[Column]) -> Vec<String> {
    let mut sources = columns.iter().filter_map(|c| c.source.as_ref());
    let Some(first) = sources.next() else {
        return Vec::new();
    };
    if !sources.all(|s| s.table == first.table) {
        return Vec::new();
    }

    let pk = primary_key(conn, &first.table);
    if !pk.is_empty() && pk.iter().all(|k| columns.iter().any(|c| &c.name == k)) {
        pk
    } else {
        // No declared primary key. SQLite would still have a rowid for most
        // tables, but it is not in the projection and selecting it behind the
        // user's back would key an UPDATE on a value they never saw.
        Vec::new()
    }
}

/// Primary key column names in key order, empty when the table declares none.
fn primary_key(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table, QUOTE));
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| {
        // Column 5 is the 1-based position within the primary key, 0 when the
        // column is not part of one.
        Ok((r.get::<_, i32>(5)?, r.get::<_, String>(1)?))
    }) else {
        return Vec::new();
    };

    rows.filter_map(|r| r.ok())
        .filter(|(position, _)| *position > 0)
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect()
}

fn table_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<ColumnDef>> {
    // PRAGMA does not accept bound parameters for the table name, so the
    // identifier is quoted instead. `table` comes from the catalog, not from
    // user input, but quoting keeps it correct for names needing escapes.
    let sql = format!("PRAGMA table_info({})", quote_ident(table, QUOTE));
    let mut stmt = conn.prepare(&sql).map_err(map_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ColumnDef {
                ordinal: r.get::<_, i32>(0)? + 1,
                name: r.get(1)?,
                type_name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                nullable: r.get::<_, i32>(3)? == 0,
                default: r.get::<_, Option<String>>(4)?,
                auto_increment: false,
                comment: None,
            })
        })
        .map_err(map_err)?;
    rows.collect::<std::result::Result<_, _>>().map_err(map_err)
}

fn table_indexes(conn: &rusqlite::Connection, table: &str) -> Result<Vec<IndexDef>> {
    let sql = format!("PRAGMA index_list({})", quote_ident(table, QUOTE));
    let mut stmt = conn.prepare(&sql).map_err(map_err)?;
    let listed: Vec<(String, bool, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, i32>(2)? == 1,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(map_err)?
        .collect::<std::result::Result<_, _>>()
        .map_err(map_err)?;

    let mut out = Vec::with_capacity(listed.len());
    for (name, unique, origin) in listed {
        let info_sql = format!("PRAGMA index_info({})", quote_ident(&name, QUOTE));
        let mut info = conn.prepare(&info_sql).map_err(map_err)?;
        let columns: Vec<String> = info
            .query_map([], |r| r.get::<_, Option<String>>(2))
            .map_err(map_err)?
            .filter_map(|c| c.ok().flatten())
            .collect();

        out.push(IndexDef {
            name,
            columns,
            unique,
            // origin "pk" means SQLite created this index for the primary key.
            primary: origin == "pk",
            method: None,
        });
    }
    Ok(out)
}

/// One row of `PRAGMA foreign_key_list`:
/// `(id, referenced_table, from_column, to_column, on_update, on_delete)`.
type FkRow = (i32, String, Option<String>, Option<String>, String, String);

fn table_foreign_keys(conn: &rusqlite::Connection, table: &str) -> Result<Vec<ForeignKeyDef>> {
    let sql = format!("PRAGMA foreign_key_list({})", quote_ident(table, QUOTE));
    let mut stmt = conn.prepare(&sql).map_err(map_err)?;

    // PRAGMA returns one row per column; rows sharing an `id` form one composite key.
    let rows: Vec<FkRow> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .map_err(map_err)?
        .collect::<std::result::Result<_, _>>()
        .map_err(map_err)?;

    let mut grouped: std::collections::BTreeMap<i32, ForeignKeyDef> = Default::default();
    for (id, ref_table, from, to, on_update, on_delete) in rows {
        let entry = grouped.entry(id).or_insert_with(|| ForeignKeyDef {
            name: format!("fk_{table}_{id}"),
            columns: Vec::new(),
            referenced_schema: None,
            referenced_table: ref_table.clone(),
            referenced_columns: Vec::new(),
            on_delete: Some(on_delete.clone()),
            on_update: Some(on_update.clone()),
        });
        if let Some(from) = from {
            entry.columns.push(from);
        }
        if let Some(to) = to {
            entry.referenced_columns.push(to);
        }
    }
    Ok(grouped.into_values().collect())
}

/// Translate rusqlite errors into the shared taxonomy so the UI can react to
/// categories rather than parsing message text.
fn map_err(e: rusqlite::Error) -> Error {
    use rusqlite::Error as R;
    match &e {
        R::SqliteFailure(err, msg) => {
            let text = msg.clone().unwrap_or_else(|| e.to_string());
            match err.code {
                rusqlite::ErrorCode::CannotOpen | rusqlite::ErrorCode::NotADatabase => {
                    Error::Connection(text)
                }
                rusqlite::ErrorCode::PermissionDenied | rusqlite::ErrorCode::ReadOnly => {
                    Error::Auth(text)
                }
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                    Error::Network(text)
                }
                // The user asked for this. Reporting it as a query error would
                // put a red message on screen for something that worked.
                rusqlite::ErrorCode::OperationInterrupted => Error::Cancelled,
                _ => Error::Query {
                    message: text,
                    position: None,
                    code: Some(format!("{:?}", err.code)),
                },
            }
        }
        _ => Error::query(e.to_string()),
    }
}

const SQLITE_FUNCTIONS: &[&str] = &[
    "abs",
    "changes",
    "char",
    "coalesce",
    "count",
    "date",
    "datetime",
    "group_concat",
    "hex",
    "ifnull",
    "instr",
    "json",
    "json_array",
    "json_extract",
    "json_object",
    "last_insert_rowid",
    "length",
    "lower",
    "ltrim",
    "max",
    "min",
    "nullif",
    "printf",
    "random",
    "replace",
    "round",
    "rtrim",
    "strftime",
    "substr",
    "sum",
    "time",
    "total",
    "trim",
    "typeof",
    "unixepoch",
    "upper",
];

#[cfg(test)]
mod tests;
