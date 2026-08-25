//! Oracle Database driver.
//!
//! Three things shape this driver more than anything else.
//!
//! # The client library is not ours to ship
//!
//! Every other engine here is reached by a protocol implementation written in
//! Rust. Oracle has none: its wire protocol (TTC) is undocumented, and the only
//! way in is Oracle's own client library. The `oracle` crate wraps ODPI-C, whose
//! C source is vendored and compiled with the crate — so *building* Table X
//! needs nothing from Oracle — but at runtime ODPI loads `oci.dll` or
//! `libclntsh.so` from the machine, and without Oracle Instant Client installed
//! there is nothing to load.
//!
//! That failure is the first thing most people will meet, so [`map_err`] catches
//! it by name and returns instructions rather than an error code.
//!
//! # It is synchronous
//!
//! ODPI blocks. Every call therefore goes through [`OracleConnection::with_conn`]
//! onto the blocking pool, the same shape the SQLite driver uses, with the
//! connection behind a mutex so that two tabs on one session serialise rather
//! than corrupting the protocol stream.
//!
//! # A schema is a user
//!
//! Oracle has no databases in the sense MySQL means: one connection reaches one
//! service, and what would elsewhere be a database is a *user* who owns objects.
//! So the tree starts at schemas, `databases` is false, and the catalogue is
//! `ALL_TABLES` and its relatives — `ALL_` rather than `DBA_`, because a
//! developer's account very often cannot read `DBA_` and a driver that needs
//! privileges to draw a tree is a driver that shows an empty tree.

mod types;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use std::sync::Arc;
use tablex_core::{
    config::ConnectionConfig,
    diagram::{GraphTable, SchemaGraph},
    driver::{
        Capabilities, CompletionScope, Connection, DdlSupport, Driver, DriverInfo, FetchOptions,
        PlaceholderStyle, RowDelete, RowEdit, RowInsert, TxStatements,
    },
    error::{Error, Result},
    plan::{Plan, PlanRow},
    result::{Column, QueryOutcome, ResultSet, StatementResult},
    schema::{decode_path, ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
    Value,
};

/// Oracle quotes identifiers with double quotes, and folds unquoted ones to
/// upper case — which is why every catalogue lookup here compares against the
/// name as stored rather than as typed.
const QUOTE: char = '"';

/// The default service on a stock installation, used when none is given.
const DEFAULT_SERVICE: &str = "XEPDB1";

struct Folder {
    id: &'static str,
    label: &'static str,
    kind: NodeKind,
}

const FOLDERS: &[Folder] = &[
    Folder {
        id: "tables",
        label: "Tables",
        kind: NodeKind::Table,
    },
    Folder {
        id: "views",
        label: "Views",
        kind: NodeKind::View,
    },
    Folder {
        id: "functions",
        label: "Functions",
        kind: NodeKind::Function,
    },
    Folder {
        id: "procedures",
        label: "Procedures",
        kind: NodeKind::Procedure,
    },
    Folder {
        id: "triggers",
        label: "Triggers",
        kind: NodeKind::Trigger,
    },
];

pub struct OracleDriver;

impl OracleDriver {
    pub fn new() -> Self {
        OracleDriver
    }
}

impl Default for OracleDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for OracleDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "oracle".into(),
            name: "Oracle".into(),
            default_port: Some(1521),
            file_based: false,
            column_types: [
                "NUMBER",
                "NUMBER(10)",
                "NUMBER(10,2)",
                "VARCHAR2(255)",
                "VARCHAR2(4000)",
                "CHAR(1)",
                "DATE",
                "TIMESTAMP",
                "TIMESTAMP WITH TIME ZONE",
                "TIMESTAMP WITH LOCAL TIME ZONE",
                "CLOB",
                "BLOB",
                "NCLOB",
                "NVARCHAR2(255)",
                "NCHAR(1)",
                "BINARY_FLOAT",
                "BINARY_DOUBLE",
                "INTEGER",
                "FLOAT",
                "RAW(16)",
                "LONG",
                "INTERVAL YEAR TO MONTH",
                "INTERVAL DAY TO SECOND",
                "XMLTYPE",
                "JSON",
                "ROWID",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            capabilities: Capabilities {
                ddl: DdlSupport {
                    add_column: true,
                    drop_column: true,
                    alter_column: true,
                    indexes: true,
                    foreign_keys: true,
                    // Oracle commits implicitly before and after every DDL
                    // statement, so a set that fails halfway stays half
                    // applied -- the same caveat MySQL carries.
                    // Neither. A database here is the service the
                    // connection reached, and a schema is a user --
                    // `CREATE USER` needs a password and quotas, which
                    // is not a name in a box.
                    triggers: true,
                    create_database: false,
                    create_schema: false,
                    transactional_ddl: false,
                },
                transactions: true,
                // `break_execution` on a second handle to the same session --
                // see `OracleCancel`.
                cancel: true,
                // One submission is one statement here. ODPI prepares a single
                // statement at a time, and this driver splits before running,
                // so it never asks the server to take several at once.
                multi_statement: false,
                explain: true,
                // EXPLAIN PLAN never runs the statement, so there is nothing to
                // measure and nothing to take back.
                explain_analyze: false,
                schemas: true,
                // One connection reaches one service. What Oracle calls a
                // database is not something a session switches between.
                databases: false,
                foreign_keys: true,
                views: true,
                stored_procedures: true,
                // DBMS_METADATA.GET_DDL returns the statement Oracle would use
                // to recreate any object the session can see, tables included.
                table_scripts: true,
                // ODPI's query metadata carries a column's type but not the
                // table it came from, so an ad-hoc result cannot be traced back
                // to a table and stays read-only. Browsing a table still knows
                // what it asked for.
                column_provenance: false,
                // Rows are fetched in batches by ODPI, but this driver collects
                // them before returning, so it does not claim the memory bound
                // `streaming` promises.
                streaming: false,
                // Not yet: reading DBA_ROLE_PRIVS and its relatives is a
                // driver's worth of work on its own, and a panel that opens
                // empty is worse than one that is not offered.
                privileges: false,
                activity: true,
                placeholder_style: PlaceholderStyle::Colon,
                identifier_quote: QUOTE,
            },
        }
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        secret: Option<&str>,
    ) -> Result<Box<dyn Connection>> {
        let host = config.host.clone().unwrap_or_else(|| "localhost".into());
        let port = config.port.unwrap_or(1521);
        // Oracle calls it a service name, and it is what the `database` field
        // holds for this driver. An empty one would connect to whatever the
        // listener defaults to, which is rarely what somebody meant.
        let service = config
            .database
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SERVICE.to_string());

        let user = config.username.clone().unwrap_or_default();
        let password = secret.unwrap_or_default().to_string();
        // The easy connect syntax, which needs no tnsnames.ora on the machine.
        let target = format!("//{host}:{port}/{service}");

        let conn = tokio::task::spawn_blocking(move || {
            oracle::Connection::connect(&user, &password, &target)
        })
        .await
        .map_err(|e| Error::Other(format!("blocking task failed: {e}")))?
        .map_err(map_err)?;

        // Committed after every write, so that a grid edit behaves the way it
        // does on every other engine here. Oracle otherwise leaves a
        // transaction open until somebody says otherwise, which would make the
        // first edit somebody makes invisible to everybody else until they
        // happened to press commit.
        let mut conn = conn;
        conn.set_autocommit(true);

        let conn = Arc::new(conn);
        Ok(Box::new(OracleConnection {
            cancel: Arc::new(OracleCancel(Arc::clone(&conn))),
            conn,
            schema: config.username.clone().unwrap_or_default().to_uppercase(),
        }))
    }
}

pub struct OracleConnection {
    /// Shared rather than held behind a mutex of this driver's own.
    ///
    /// Two things make that right. The session registry above already
    /// serialises every call on one connection, so a second lock here would
    /// guard nothing; and `oracle::Connection` is `Sync`, which is what lets
    /// the cancel handle reach the session *while* a statement is running on
    /// it. A mutex would put the cancel in a queue behind the thing it was
    /// cancelling.
    conn: Arc<oracle::Connection>,
    cancel: Arc<OracleCancel>,
    /// The session's own schema, which is where an unqualified name resolves.
    schema: String,
}

/// Stops the current statement by asking the server to abandon it.
///
/// `break_execution` is ODPI's out-of-band interrupt: it reaches the server on
/// its own channel, so it does not wait for the statement it is stopping. The
/// server answers the cancelled statement with ORA-01013, which `map_err` turns
/// back into `Cancelled` -- the user getting what they asked for rather than a
/// failure.
struct OracleCancel(Arc<oracle::Connection>);

#[async_trait]
impl tablex_core::driver::CancelHandle for OracleCancel {
    async fn cancel(&self) -> Result<()> {
        let conn = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || conn.break_execution().map_err(map_err))
            .await
            .map_err(|e| Error::Other(format!("blocking task failed: {e}")))?
    }
}

impl OracleConnection {
    /// Run a closure against the connection on the blocking pool.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&oracle::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || f(&conn))
            .await
            .map_err(|e| Error::Other(format!("blocking task failed: {e}")))?
    }

    /// Every row of a query, as text-ish values.
    async fn rows_of(&self, sql: String) -> Result<Vec<Vec<Value>>> {
        self.with_conn(move |conn| {
            let rows = conn.query(&sql, &[]).map_err(map_err)?;
            let types: Vec<oracle::sql_type::OracleType> = rows
                .column_info()
                .iter()
                .map(|c| c.oracle_type().clone())
                .collect();

            let mut out = Vec::new();
            for row in rows {
                let row = row.map_err(map_err)?;
                out.push(
                    row.sql_values()
                        .iter()
                        .zip(types.iter())
                        .map(|(value, ty)| types::decode(value, ty))
                        .collect(),
                );
            }
            Ok(out)
        })
        .await
    }

    /// The first column of every row, as strings — the shape most catalogue
    /// lookups here need.
    async fn strings(&self, sql: String) -> Result<Vec<String>> {
        Ok(self
            .rows_of(sql)
            .await?
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|value| match value {
                Value::Text(s) => s,
                other => other.to_string(),
            })
            .collect())
    }

    async fn run_one(&self, sql: &str, opts: &FetchOptions) -> Result<StatementResult> {
        let statement = sql.to_string();
        let offset = opts.offset;
        let cap = opts.max_rows.unwrap_or(usize::MAX);

        self.with_conn(move |conn| {
            // Asked of the statement rather than guessed from its text: a
            // `WITH … SELECT` is a query and an anonymous PL/SQL block is not,
            // and no amount of looking at the first keyword settles it.
            let mut prepared = conn.statement(&statement).build().map_err(map_err)?;

            if !prepared.is_query() {
                prepared.execute(&[]).map_err(map_err)?;
                return Ok(StatementResult::Affected {
                    rows_affected: prepared.row_count().map_err(map_err)?,
                    // Oracle has no equivalent: a generated key comes back
                    // through RETURNING, which the statement has to ask for.
                    last_insert_id: None,
                });
            }

            let rows = prepared.query(&[]).map_err(map_err)?;
            let info = rows.column_info();
            let columns: Vec<Column> = info
                .iter()
                .map(|c| Column {
                    name: c.name().to_string(),
                    type_name: types::type_name(c.oracle_type()),
                    nullable: Some(c.nullable()),
                    // See `column_provenance` in the capabilities.
                    source: None,
                })
                .collect();
            let oracle_types: Vec<oracle::sql_type::OracleType> =
                info.iter().map(|c| c.oracle_type().clone()).collect();

            let mut decoded = Vec::new();
            let mut seen = 0usize;
            let mut truncated = false;
            for row in rows {
                let row = row.map_err(map_err)?;
                seen += 1;
                if seen <= offset {
                    continue;
                }
                if decoded.len() >= cap {
                    // One row past the cap is enough to know there are more,
                    // and stopping here is what keeps a `SELECT *` on a large
                    // table from reading the whole thing.
                    truncated = true;
                    break;
                }
                decoded.push(
                    row.sql_values()
                        .iter()
                        .zip(oracle_types.iter())
                        .map(|(value, ty)| types::decode(value, ty))
                        .collect(),
                );
            }

            let mut rs = ResultSet {
                columns,
                rows: decoded,
                truncated,
                editable: false,
                key_columns: Vec::new(),
            };
            rs.recompute_editable();
            Ok(StatementResult::Rows(rs))
        })
        .await
    }

    /// The schemas that own something worth browsing.
    async fn browse_schema_names(&self) -> Result<Vec<String>> {
        // Owners of objects rather than every user: a stock installation has
        // dozens of Oracle's own accounts, and a tree that opens on thirty
        // empty schemas is a tree nobody scrolls.
        self.strings(
            "SELECT DISTINCT owner FROM all_objects \
             WHERE object_type IN ('TABLE', 'VIEW', 'PROCEDURE', 'FUNCTION', 'TRIGGER') \
             ORDER BY owner"
                .to_string(),
        )
        .await
    }

    async fn browse_schemas(&self) -> Result<Vec<SchemaNode>> {
        Ok(self
            .browse_schema_names()
            .await?
            .into_iter()
            .map(|owner| {
                let node = SchemaNode::new(&[&owner], owner.clone(), NodeKind::Schema).expandable();
                // The session's own schema first, since it is the one whose
                // objects an unqualified name resolves to.
                if owner == self.schema {
                    node.detail("current")
                } else {
                    node
                }
            })
            .collect())
    }

    async fn browse_folder(&self, schema: &str, folder: &Folder) -> Result<Vec<SchemaNode>> {
        let owner = literal(schema);
        let sql = match folder.id {
            "tables" => format!(
                "SELECT table_name FROM all_tables WHERE owner = {owner} ORDER BY table_name"
            ),
            "views" => {
                format!("SELECT view_name FROM all_views WHERE owner = {owner} ORDER BY view_name")
            }
            "functions" => format!(
                "SELECT object_name FROM all_objects WHERE owner = {owner} \
                 AND object_type = 'FUNCTION' ORDER BY object_name"
            ),
            "procedures" => format!(
                "SELECT object_name FROM all_objects WHERE owner = {owner} \
                 AND object_type = 'PROCEDURE' ORDER BY object_name"
            ),
            "triggers" => format!(
                "SELECT trigger_name FROM all_triggers WHERE owner = {owner} ORDER BY trigger_name"
            ),
            _ => return Ok(Vec::new()),
        };

        let names = self.strings(sql).await?;
        let kind = folder.kind.clone();
        Ok(names
            .into_iter()
            .map(|name| {
                let node = SchemaNode::new(&[schema, folder.id, &name], name.clone(), kind.clone())
                    .qualified(format!(
                        "{}.{}",
                        quote_ident(schema, QUOTE),
                        quote_ident(&name, QUOTE)
                    ));
                // Only the ones that have columns to show.
                if matches!(kind, NodeKind::Table | NodeKind::View) {
                    node.expandable()
                } else {
                    node
                }
            })
            .collect())
    }

    async fn browse_columns(
        &self,
        schema: &str,
        folder: &str,
        object: &str,
    ) -> Result<Vec<SchemaNode>> {
        let sql = format!(
            "SELECT column_name, data_type, nullable FROM all_tab_columns \
             WHERE owner = {} AND table_name = {} ORDER BY column_id",
            literal(schema),
            literal(object)
        );

        Ok(self
            .rows_of(sql)
            .await?
            .into_iter()
            .map(|row| {
                let name = cell(&row, 0);
                let type_name = cell(&row, 1);
                let nullable = cell(&row, 2) == "Y";
                SchemaNode::new(
                    &[schema, folder, object, &name],
                    name.clone(),
                    NodeKind::Column,
                )
                .detail(if nullable {
                    type_name
                } else {
                    format!("{type_name} not null")
                })
            })
            .collect())
    }
}

#[async_trait]
impl Connection for OracleConnection {
    async fn execute(&mut self, sql: &str, opts: &FetchOptions) -> Result<QueryOutcome> {
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Err(Error::query("no statement to execute"));
        }

        let started = std::time::Instant::now();
        let mut out = Vec::with_capacity(statements.len());
        for statement in &statements {
            out.push(self.run_one(statement, opts).await?);
        }

        Ok(QueryOutcome {
            statements: out,
            elapsed_ms: started.elapsed().as_millis() as u64,
            notices: Vec::new(),
        })
    }

    /// Paths are `[]`, `[schema]`, `[schema, folder]`, `[schema, folder, object]`.
    ///
    /// One level shallower than the engines with databases, because a
    /// connection here reaches exactly one service.
    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
        let path = parent.map(decode_path).unwrap_or_default();
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();

        match segments.as_slice() {
            [] => self.browse_schemas().await,
            [schema] => Ok(FOLDERS
                .iter()
                .map(|f| SchemaNode::new(&[schema, f.id], f.label, NodeKind::Folder).expandable())
                .collect()),
            [schema, folder] => match FOLDERS.iter().find(|f| f.id == *folder) {
                Some(spec) => self.browse_folder(schema, spec).await,
                None => Ok(Vec::new()),
            },
            [schema, folder, object] => self.browse_columns(schema, folder, object).await,
            _ => Ok(Vec::new()),
        }
    }

    async fn table_detail(&mut self, schema: Option<&str>, table: &str) -> Result<TableDetail> {
        let owner = schema.unwrap_or(&self.schema).to_string();
        let columns = self.read_columns(&owner, table).await?;
        let primary_key = self.read_primary_key(&owner, table).await?;
        let indexes = self.read_indexes(&owner, table).await?;
        let foreign_keys = self.read_foreign_keys(&owner, table).await?;

        // The optimiser's estimate, which is what `num_rows` is: it is whatever
        // the last gather of statistics found, and saying so is the caller's
        // job — the field is named `estimated_rows` for that reason.
        let estimated = self
            .rows_of(format!(
                "SELECT num_rows FROM all_tables WHERE owner = {} AND table_name = {}",
                literal(&owner),
                literal(table)
            ))
            .await?
            .first()
            .and_then(|row| row.first())
            .and_then(|value| match value {
                Value::Int(n) => Some(*n),
                Value::Numeric(text) => text.parse().ok(),
                _ => None,
            });

        Ok(TableDetail {
            schema: Some(owner),
            name: table.to_string(),
            columns,
            indexes,
            foreign_keys,
            triggers: Vec::new(),
            primary_key,
            estimated_rows: estimated,
            comment: None,
        })
    }

    async fn apply_edit(&mut self, edit: &RowEdit) -> Result<()> {
        if edit.changes.is_empty() {
            return Ok(());
        }
        if edit.key.is_empty() {
            return Err(Error::Unsupported(
                "cannot edit a row that has no unique key".into(),
            ));
        }

        let assignments = edit
            .changes
            .iter()
            .map(|(col, val)| format!("{} = {}", quote_ident(col, QUOTE), types::literal(val)))
            .collect::<Vec<_>>()
            .join(", ");

        let qualified = self.qualify(edit.schema.as_deref(), &edit.table);
        let predicate = predicate(&edit.key);

        // A PL/SQL block rather than a bare UPDATE, so a key that turns out not
        // to be unique leaves nothing applied: the count is checked on the
        // server, between the update and the commit, where no round trip can
        // come between them.
        let block = format!(
            "BEGIN \
               UPDATE {qualified} SET {assignments} WHERE {predicate}; \
               IF SQL%ROWCOUNT > 1 THEN \
                 ROLLBACK; \
                 RAISE_APPLICATION_ERROR(-20001, \
                   ''edit matched more than one row, expected at most 1 - the key is not unique''); \
               END IF; \
             END;"
        );
        self.run_write(block).await
    }

    async fn insert_row(&mut self, insert: &RowInsert) -> Result<()> {
        if insert.values.is_empty() {
            return Err(Error::Unsupported(
                "an inserted row needs at least one value".into(),
            ));
        }

        let columns = insert
            .values
            .iter()
            .map(|(col, _)| quote_ident(col, QUOTE))
            .collect::<Vec<_>>()
            .join(", ");
        let values = insert
            .values
            .iter()
            .map(|(_, val)| types::literal(val))
            .collect::<Vec<_>>()
            .join(", ");

        let qualified = self.qualify(insert.schema.as_deref(), &insert.table);
        self.run_write(format!(
            "INSERT INTO {qualified} ({columns}) VALUES ({values})"
        ))
        .await
    }

    async fn delete_row(&mut self, delete: &RowDelete) -> Result<()> {
        if delete.key.is_empty() {
            return Err(Error::Unsupported(
                "cannot delete a row that has no unique key".into(),
            ));
        }

        let qualified = self.qualify(delete.schema.as_deref(), &delete.table);
        let predicate = predicate(&delete.key);

        // Guarded the same way an edit is, and for a worse reason: a key that
        // matches two rows deletes both, and there is no undo for that.
        let block = format!(
            "BEGIN \
               DELETE FROM {qualified} WHERE {predicate}; \
               IF SQL%ROWCOUNT > 1 THEN \
                 ROLLBACK; \
                 RAISE_APPLICATION_ERROR(-20001, \
                   ''delete matched more than one row, expected at most 1 - the key is not unique''); \
               END IF; \
             END;"
        );
        self.run_write(block).await
    }

    fn transaction_statements(&self) -> Option<TxStatements> {
        // Reported so the controls appear; the three methods below are
        // overridden, so these strings are never what runs. Oracle has no
        // statement that starts a transaction -- DML starts one -- so a `begin`
        // string here would be a claim about something that was never sent.
        Some(TxStatements {
            begin: "SET TRANSACTION READ WRITE",
            commit: "COMMIT",
            rollback: "ROLLBACK",
        })
    }

    /// Start a transaction by turning autocommit off.
    ///
    /// Oracle never commits by itself; this driver does, after every write, so
    /// that a grid edit behaves the way it does on every other engine here. An
    /// explicit transaction is therefore exactly the absence of that: the
    /// writes accumulate until somebody says what to do with them.
    async fn begin(&mut self) -> Result<()> {
        // `SET TRANSACTION` is a real Oracle statement and it does start one;
        // what it cannot do is stop the driver committing after each write, so
        // autocommit goes off first where that is possible.
        let _ = self.set_autocommit(false);
        self.with_conn(|conn| {
            conn.execute("SET TRANSACTION READ WRITE", &[])
                .map_err(map_err)?;
            Ok(())
        })
        .await
    }

    async fn commit(&mut self) -> Result<()> {
        self.with_conn(|conn| conn.commit().map_err(map_err))
            .await?;
        let _ = self.set_autocommit(true);
        Ok(())
    }

    async fn rollback(&mut self) -> Result<()> {
        self.with_conn(|conn| conn.rollback().map_err(map_err))
            .await?;
        let _ = self.set_autocommit(true);
        Ok(())
    }

    async fn ping(&mut self) -> Result<()> {
        self.with_conn(|conn| conn.ping().map_err(map_err)).await
    }

    fn cancel_handle(&self) -> Option<Arc<dyn tablex_core::driver::CancelHandle>> {
        Some(Arc::clone(&self.cancel) as Arc<dyn tablex_core::driver::CancelHandle>)
    }

    async fn close(&mut self) -> Result<()> {
        // A close that fails is a socket the server will reap anyway; failing
        // the call would leave the UI showing a connection that is gone.
        let _ = self.with_conn(|conn| conn.close().map_err(map_err)).await;
        Ok(())
    }

    async fn current_database(&mut self) -> Result<Option<String>> {
        // The session's schema, which is the nearest thing Oracle has to the
        // "current database" the UI shows.
        Ok(Some(self.schema.clone()))
    }

    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        let owner = literal(&self.schema);

        // One query for both, rather than one per table: a schema with three
        // hundred tables would otherwise be three hundred round trips before
        // the first keystroke is answered.
        let rows = self
            .rows_of(format!(
                "SELECT table_name, column_name FROM all_tab_columns WHERE owner = {owner} \
                 ORDER BY table_name, column_id"
            ))
            .await
            .unwrap_or_default();

        let mut tables: Vec<(String, Vec<String>)> = Vec::new();
        for row in rows {
            let table = cell(&row, 0);
            let column = cell(&row, 1);
            match tables.last_mut() {
                // The rows arrive grouped by table, so only the last one can
                // match; scanning the whole list per row would be quadratic on
                // exactly the schemas where this matters.
                Some((name, columns)) if *name == table => columns.push(column),
                _ => tables.push((table, vec![column])),
            }
        }

        let functions = self
            .strings(format!(
                "SELECT object_name FROM all_objects WHERE owner = {owner} \
                 AND object_type = 'FUNCTION' ORDER BY object_name"
            ))
            .await
            .unwrap_or_default();

        Ok(CompletionScope {
            schemas: self.browse_schema_names().await.unwrap_or_default(),
            tables,
            functions,
            keywords: Vec::new(),
        })
    }

    async fn definition(&mut self, node_id: &str) -> Result<String> {
        let path = decode_path(node_id);
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();
        let [schema, folder, object] = segments.as_slice() else {
            return Err(Error::Unsupported(
                "only an object has a definition to show".into(),
            ));
        };

        let object_type = match *folder {
            "tables" => "TABLE",
            "views" => "VIEW",
            "functions" => "FUNCTION",
            "procedures" => "PROCEDURE",
            "triggers" => "TRIGGER",
            _ => return Err(Error::Unsupported("no definition for this kind".into())),
        };

        // DBMS_METADATA returns the statement Oracle would use to recreate the
        // object, which is a better answer than reassembling one from the
        // catalogue and getting the storage clauses wrong.
        let sql = format!(
            "SELECT DBMS_METADATA.GET_DDL('{object_type}', {}, {}) FROM dual",
            literal(object),
            literal(schema)
        );

        self.rows_of(sql)
            .await?
            .first()
            .and_then(|row| row.first())
            .map(|value| match value {
                Value::Text(text) => text.clone(),
                other => other.to_string(),
            })
            .ok_or_else(|| Error::Other("the object has no definition".into()))
    }

    async fn schema_graph(&mut self, schema: Option<&str>) -> Result<SchemaGraph> {
        let owner = schema.unwrap_or(&self.schema).to_string();
        let names = self
            .strings(format!(
                "SELECT table_name FROM all_tables WHERE owner = {} ORDER BY table_name",
                literal(&owner)
            ))
            .await?;

        let mut tables = Vec::with_capacity(names.len());
        for name in names {
            let foreign_keys = self
                .read_foreign_keys(&owner, &name)
                .await
                .unwrap_or_default();
            tables.push(GraphTable {
                schema: Some(owner.clone()),
                name,
                foreign_keys,
                // The browsing diagram draws only the columns that carry a
                // relation — see `GraphTable::columns`.
                columns: Vec::new(),
            });
        }
        Ok(SchemaGraph { tables })
    }

    async fn explain(&mut self, sql: &str, _analyze: bool) -> Result<Plan> {
        // Two statements: Oracle writes the plan into PLAN_TABLE and it is read
        // back from there. The statement itself is never run — EXPLAIN PLAN
        // parses it and stops — which is why `explain_analyze` is false rather
        // than an option that would quietly perform the DELETE being explained.
        let statement = sql.trim().trim_end_matches(';').to_string();
        let tag = format!("tablex_{}", uuid::Uuid::new_v4().simple());

        let prepare = format!(
            "EXPLAIN PLAN SET STATEMENT_ID = {} FOR {statement}",
            literal(&tag)
        );
        self.run_write(prepare).await?;

        let rows = self
            .rows_of(format!(
                "SELECT id, NVL(parent_id, -1), operation, options, object_name, \
                        cardinality, cost \
                 FROM plan_table WHERE statement_id = {} ORDER BY id",
                literal(&tag)
            ))
            .await?;

        // The plan is kept in an ordinary table, so it stays there until it is
        // removed. Best effort: a plan that could not be cleaned up is untidy,
        // and failing the explain over it would be worse.
        let _ = self
            .run_write(format!(
                "DELETE FROM plan_table WHERE statement_id = {}",
                literal(&tag)
            ))
            .await;

        let plan_rows: Vec<PlanRow> = rows
            .iter()
            .map(|row| {
                let operation = cell(row, 2);
                let options = cell(row, 3);
                PlanRow {
                    id: number(row, 0).unwrap_or_default() as i64,
                    parent: number(row, 1).unwrap_or(-1.0) as i64,
                    // "TABLE ACCESS FULL" rather than "TABLE ACCESS": the
                    // option is the half that says whether it is a scan.
                    label: if options.is_empty() {
                        operation
                    } else {
                        format!("{operation} {options}")
                    },
                    detail: optional(row, 4),
                    rows: number(row, 5),
                    cost: number(row, 6),
                }
            })
            .collect();

        if plan_rows.is_empty() {
            return Err(Error::Unsupported(
                "the server produced no plan for that statement".into(),
            ));
        }

        // The raw text as Oracle would print it, so the view can show the
        // engine's own words beside the tree it built from them.
        let raw = plan_rows
            .iter()
            .map(|row| match (&row.detail, row.cost) {
                (Some(detail), Some(cost)) => format!("{} {detail} (cost {cost})", row.label),
                (Some(detail), None) => format!("{} {detail}", row.label),
                (None, Some(cost)) => format!("{} (cost {cost})", row.label),
                (None, None) => row.label.clone(),
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            );

        Ok(Plan {
            root: tablex_core::plan::from_parent_rows(plan_rows, "Statement"),
            // EXPLAIN PLAN parses the statement and stops, so every number here
            // is the optimiser's estimate rather than a measurement.
            analyzed: false,
            raw,
        })
    }

    async fn activity(&mut self) -> Result<tablex_core::activity::ServerActivity> {
        // V$SESSION needs SELECT on the view, which a developer account often
        // has and a locked-down one does not. The error says which.
        let rows = self
            .rows_of(
                "SELECT s.sid || ',' || s.serial# AS id, s.username, s.machine, s.status, \
                 s.sql_id, q.sql_text, s.seconds_in_wait \
                 FROM v$session s LEFT JOIN v$sql q ON q.sql_id = s.sql_id \
                 WHERE s.type = 'USER' ORDER BY s.sid"
                    .to_string(),
            )
            .await?;

        let sessions = rows
            .into_iter()
            .map(|row| tablex_core::activity::ServerSession {
                id: cell(&row, 0),
                user: optional(&row, 1),
                client: optional(&row, 2),
                database: None,
                state: optional(&row, 3),
                query: optional(&row, 5),
                seconds: match row.get(6) {
                    Some(Value::Int(n)) => Some(*n as f64),
                    Some(Value::Numeric(text)) => text.parse().ok(),
                    _ => None,
                },
                // Oracle can say, through `blocking_session`, but only when
                // the blocker is on the same instance and the view is
                // readable. Left out rather than half-answered.
                blocked_by: None,
                is_self: false,
            })
            .collect();

        Ok(tablex_core::activity::ServerActivity {
            sessions,
            stats: Vec::new(),
        })
    }

    async fn kill_session(&mut self, id: &str) -> Result<()> {
        // `sid,serial#`, exactly as the activity panel listed it. Quoted as a
        // literal because that is the syntax, not because it is a string.
        let statement = format!("ALTER SYSTEM KILL SESSION {} IMMEDIATE", literal(id));
        self.run_write(statement).await
    }
}

impl OracleConnection {
    /// A table as SQL should refer to it, in the session's own schema when the
    /// caller did not say which.
    fn qualify(&self, schema: Option<&str>, table: &str) -> String {
        format!(
            "{}.{}",
            quote_ident(schema.unwrap_or(&self.schema), QUOTE),
            quote_ident(table, QUOTE)
        )
    }

    /// Turn the driver's automatic commit on or off.
    ///
    /// Takes `&mut self` through the `Arc`, which is only safe because nothing
    /// else holds a reference at the time: the session registry serialises
    /// every call on this connection, and the cancel handle only ever calls
    /// `break_execution`, which takes `&self`.
    fn set_autocommit(&mut self, on: bool) -> Result<()> {
        match Arc::get_mut(&mut self.conn) {
            Some(conn) => {
                conn.set_autocommit(on);
                Ok(())
            }
            // The cancel handle holds the other reference, so this is the
            // ordinary case rather than an error; it is handled by dropping to
            // the statement form below.
            None => Err(Error::Unsupported(
                "the connection is shared, so autocommit cannot be changed".into(),
            )),
        }
    }

    /// Run a statement for its effect, not its rows.
    async fn run_write(&self, statement: String) -> Result<()> {
        self.with_conn(move |conn| {
            conn.execute(&statement, &[]).map_err(map_err)?;
            Ok(())
        })
        .await
    }

    async fn read_columns(&self, owner: &str, table: &str) -> Result<Vec<ColumnDef>> {
        let sql = format!(
            "SELECT column_name, data_type, data_length, data_precision, data_scale, \
             nullable, data_default, column_id, identity_column \
             FROM all_tab_columns WHERE owner = {} AND table_name = {} ORDER BY column_id",
            literal(owner),
            literal(table)
        );

        Ok(self
            .rows_of(sql)
            .await?
            .into_iter()
            .enumerate()
            .map(|(index, row)| ColumnDef {
                name: cell(&row, 0),
                type_name: declared_type(&row),
                nullable: cell(&row, 5) == "Y",
                default: optional(&row, 6)
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty()),
                // Identity columns arrived in 12c; before that a generated key
                // is a sequence and a trigger, which the catalogue cannot tell
                // apart from any other default.
                auto_increment: cell(&row, 8) == "YES",
                ordinal: index as i32,
                comment: None,
            })
            .collect())
    }

    async fn read_primary_key(&self, owner: &str, table: &str) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT c.column_name FROM all_constraints k \
             JOIN all_cons_columns c ON c.owner = k.owner AND c.constraint_name = k.constraint_name \
             WHERE k.owner = {} AND k.table_name = {} AND k.constraint_type = 'P' \
             ORDER BY c.position",
            literal(owner),
            literal(table)
        );
        self.strings(sql).await
    }

    async fn read_indexes(&self, owner: &str, table: &str) -> Result<Vec<IndexDef>> {
        let sql = format!(
            "SELECT i.index_name, i.uniqueness, c.column_name, i.index_type \
             FROM all_indexes i \
             JOIN all_ind_columns c ON c.index_owner = i.owner AND c.index_name = i.index_name \
             WHERE i.table_owner = {} AND i.table_name = {} \
             ORDER BY i.index_name, c.column_position",
            literal(owner),
            literal(table)
        );

        let mut indexes: Vec<IndexDef> = Vec::new();
        for row in self.rows_of(sql).await? {
            let name = cell(&row, 0);
            let unique = cell(&row, 1) == "UNIQUE";
            let column = cell(&row, 2);
            let method = cell(&row, 3);

            match indexes.iter_mut().find(|i| i.name == name) {
                // A composite index arrives one row per column, in key order.
                Some(existing) => existing.columns.push(column),
                None => indexes.push(IndexDef {
                    name,
                    columns: vec![column],
                    unique,
                    primary: false,
                    method: Some(method),
                }),
            }
        }

        // The primary key's index is reported like any other; marking it is
        // what lets the UI stop offering to drop it.
        let primary = self
            .read_primary_key(owner, table)
            .await
            .unwrap_or_default();
        for index in &mut indexes {
            if !primary.is_empty() && index.columns == primary {
                index.primary = true;
            }
        }
        Ok(indexes)
    }

    async fn read_foreign_keys(&self, owner: &str, table: &str) -> Result<Vec<ForeignKeyDef>> {
        // The referenced side is reached through the constraint the key points
        // at: Oracle records a foreign key as "this constraint refers to that
        // primary or unique constraint", not as a table and column pair.
        let sql = format!(
            "SELECT k.constraint_name, c.column_name, r.owner, r.table_name, rc.column_name, \
             k.delete_rule \
             FROM all_constraints k \
             JOIN all_cons_columns c ON c.owner = k.owner AND c.constraint_name = k.constraint_name \
             JOIN all_constraints r ON r.owner = k.r_owner AND r.constraint_name = k.r_constraint_name \
             JOIN all_cons_columns rc ON rc.owner = r.owner AND rc.constraint_name = r.constraint_name \
             AND rc.position = c.position \
             WHERE k.owner = {} AND k.table_name = {} AND k.constraint_type = 'R' \
             ORDER BY k.constraint_name, c.position",
            literal(owner),
            literal(table)
        );

        let mut keys: Vec<ForeignKeyDef> = Vec::new();
        for row in self.rows_of(sql).await? {
            let name = cell(&row, 0);
            let column = cell(&row, 1);
            let referenced_column = cell(&row, 4);

            match keys.iter_mut().find(|k| k.name == name) {
                Some(existing) => {
                    existing.columns.push(column);
                    existing.referenced_columns.push(referenced_column);
                }
                None => keys.push(ForeignKeyDef {
                    name,
                    columns: vec![column],
                    referenced_schema: Some(cell(&row, 2)),
                    referenced_table: cell(&row, 3),
                    referenced_columns: vec![referenced_column],
                    // Oracle has no ON UPDATE at all, and only NO ACTION and
                    // CASCADE on delete. Reporting the absent one as absent is
                    // more useful than inventing "NO ACTION".
                    on_delete: optional(&row, 5).filter(|r| r != "NO ACTION"),
                    on_update: None,
                }),
            }
        }
        Ok(keys)
    }
}

/// The `WHERE` clause that matches one row by its key.
fn predicate(key: &[(String, Value)]) -> String {
    key.iter()
        .map(|(col, val)| {
            // `= NULL` is never true in SQL; only `IS NULL` matches.
            if val.is_null() {
                format!("{} IS NULL", quote_ident(col, QUOTE))
            } else {
                format!("{} = {}", quote_ident(col, QUOTE), types::literal(val))
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The type as it was declared, with the size or precision that was given.
///
/// `ALL_TAB_COLUMNS` stores the parts separately, so `VARCHAR2` and `NUMBER`
/// come back without the `(255)` or `(10,2)` that is half of what the column
/// is. A schema comparison built on the bare name would call two different
/// columns the same.
fn declared_type(row: &[Value]) -> String {
    let base = cell(row, 1);
    let number = |index: usize| -> Option<i64> {
        match row.get(index) {
            Some(Value::Int(n)) => Some(*n),
            Some(Value::Numeric(text)) => text.parse().ok(),
            _ => None,
        }
    };

    match base.as_str() {
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" | "RAW" => match number(2) {
            Some(length) => format!("{base}({length})"),
            None => base,
        },
        "NUMBER" => match (number(3), number(4)) {
            (Some(precision), Some(scale)) if scale != 0 => format!("NUMBER({precision},{scale})"),
            (Some(precision), _) => format!("NUMBER({precision})"),
            // No precision at all is the unconstrained NUMBER, which is written
            // without parentheses.
            _ => base,
        },
        _ => base,
    }
}

/// One cell as a string, empty when absent or NULL.
fn cell(row: &[Value], index: usize) -> String {
    match row.get(index) {
        Some(Value::Null) | None => String::new(),
        Some(Value::Text(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// One cell as a number, whatever numeric shape it arrived in.
fn number(row: &[Value], index: usize) -> Option<f64> {
    match row.get(index) {
        Some(Value::Int(n)) => Some(*n as f64),
        Some(Value::Float(f)) => Some(*f),
        Some(Value::Numeric(text)) => text.parse().ok(),
        _ => None,
    }
}

/// One cell as a string, or `None` when it is NULL or empty.
fn optional(row: &[Value], index: usize) -> Option<String> {
    let text = cell(row, index);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// A string literal for a catalogue lookup.
///
/// These are built rather than bound because they go into `SELECT` statements
/// this driver writes about names it was given; doubling the quote is the whole
/// of the escaping Oracle needs for a literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Map an Oracle error onto the shared error model.
///
/// The first case is the one most people will meet, and an ORA code is no help
/// with it: without Oracle's client library there is nothing to load, and what
/// the reader needs is the name of the thing to install.
fn map_err(error: oracle::Error) -> Error {
    classify(error.to_string())
}

/// The message alone, so the mapping can be tested without an Oracle error.
///
/// Split out because building one of those means either a live failure or a
/// deprecated constructor, and neither is a reason to leave the branch that
/// decides whether a user is offered a retry untested.
fn classify(text: String) -> Error {
    if text.contains("DPI-1047") || text.contains("Cannot locate a 64-bit Oracle Client library") {
        return Error::Config(
            "Oracle Instant Client is not installed, or is not on the library path. \
             Oracle has no open protocol implementation, so its own client library is the only \
             way in — download Instant Client Basic from Oracle and put it on PATH (Windows), \
             LD_LIBRARY_PATH (Linux), or DYLD_LIBRARY_PATH (macOS)."
                .into(),
        );
    }

    // ORA-01017 invalid username/password; ORA-28000 account locked;
    // ORA-28001 password expired.
    if ["ORA-01017", "ORA-28000", "ORA-28001"]
        .iter()
        .any(|code| text.contains(code))
    {
        return Error::Auth(text);
    }

    // ORA-12154 could not resolve the connect identifier; ORA-12541 no
    // listener; ORA-12514 the listener does not know the service.
    if ["ORA-12154", "ORA-12541", "ORA-12514", "ORA-12170"]
        .iter()
        .any(|code| text.contains(code))
    {
        return Error::Connection(text);
    }

    // ORA-01013 is the server acknowledging a cancel, which is the user getting
    // what they asked for rather than a failure.
    if text.contains("ORA-01013") {
        return Error::Cancelled;
    }

    Error::query(text)
}
