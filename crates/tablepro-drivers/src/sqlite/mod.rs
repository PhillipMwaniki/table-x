//! SQLite driver.
//!
//! rusqlite is synchronous, so every call runs on Tokio's blocking pool with the
//! connection behind a mutex. An actor thread would also work, but the mutex keeps
//! the borrow pattern obvious and SQLite serializes writes internally regardless.

mod types;

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tablepro_core::{
    config::ConnectionConfig,
    driver::{
        Capabilities, CompletionScope, Connection, Driver, DriverInfo, FetchOptions,
        PlaceholderStyle, RowEdit,
    },
    error::{Error, Result},
    result::{Column, QueryOutcome, ResultSet, StatementResult},
    schema::{ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
};
use types::Affinity;

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
                transactions: true,
                multi_statement: true,
                explain: true,
                foreign_keys: true,
                views: true,
                // SQLite has no schemas (attached databases are a different concept),
                // and rusqlite does not expose per-column origin metadata without
                // SQLITE_ENABLE_COLUMN_METADATA, so arbitrary queries carry no
                // provenance and are not inline-editable. Browsing a table is,
                // because that path knows the table it asked for.
                schemas: false,
                column_provenance: false,
                stored_procedures: false,
                cancel: false,
                streaming: false,
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

        Ok(Box::new(SqliteConnection {
            conn: Arc::new(Mutex::new(conn)),
        }))
    }
}

pub struct SqliteConnection {
    conn: Arc<Mutex<rusqlite::Connection>>,
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

    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
        let parent = parent.map(str::to_string);
        self.with_conn(move |conn| match parent {
            // Roots: every table and view in the main database.
            None => {
                let mut stmt = conn
                    .prepare(
                        "SELECT name, type FROM sqlite_master \
                         WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' \
                         ORDER BY type, name",
                    )
                    .map_err(map_err)?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map_err(map_err)?;

                let mut nodes = Vec::new();
                for row in rows {
                    let (name, kind) = row.map_err(map_err)?;
                    nodes.push(SchemaNode {
                        id: name.clone(),
                        name,
                        kind: if kind == "view" {
                            NodeKind::View
                        } else {
                            NodeKind::Table
                        },
                        expandable: true,
                        children: None,
                        detail: None,
                    });
                }
                Ok(nodes)
            }

            // Expanding a table lists its columns.
            Some(table) => {
                let cols = table_columns(conn, &table)?;
                Ok(cols
                    .into_iter()
                    .map(|c| SchemaNode {
                        id: format!("{table}.{}", c.name),
                        name: c.name.clone(),
                        kind: NodeKind::Column,
                        expandable: false,
                        children: None,
                        detail: Some(if c.nullable {
                            c.type_name.clone()
                        } else {
                            format!("{} NOT NULL", c.type_name)
                        }),
                    })
                    .collect())
            }
        })
        .await
    }

    async fn table_detail(&mut self, _schema: Option<&str>, table: &str) -> Result<TableDetail> {
        let table = table.to_string();
        self.with_conn(move |conn| {
            let columns = table_columns(conn, &table)?;
            let primary_key = columns
                .iter()
                .filter(|c| c.ordinal > 0)
                .filter_map(|c| pk_position(conn, &table, &c.name).map(|pos| (pos, c.name.clone())))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_values()
                .collect::<Vec<_>>();

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

    let columns: Vec<Column> = stmt
        .columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_string(),
            type_name: c.decl_type().unwrap_or("").to_string(),
            nullable: None,
            // See `Capabilities::column_provenance` — not available here.
            source: None,
        })
        .collect();

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

    let mut rows_out: Vec<Vec<tablepro_core::Value>> = Vec::new();
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
        key_columns: Vec::new(),
    };
    rs.recompute_editable();
    Ok(StatementResult::Rows(rs))
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

/// Position of a column in the primary key, or `None` if it is not part of one.
fn pk_position(conn: &rusqlite::Connection, table: &str, column: &str) -> Option<i32> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table, QUOTE));
    let mut stmt = conn.prepare(&sql).ok()?;
    let mut rows = stmt.query([]).ok()?;
    while let Ok(Some(row)) = rows.next() {
        let name: String = row.get(1).ok()?;
        let pk: i32 = row.get(5).ok()?;
        if name == column && pk > 0 {
            return Some(pk);
        }
    }
    None
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
