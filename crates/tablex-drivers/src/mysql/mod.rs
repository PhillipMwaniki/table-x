//! MySQL and MariaDB driver.
//!
//! Like PostgreSQL and unlike SQLite, the wire protocol reports the originating
//! table and column for every result column, so ad-hoc query results can be
//! edited in place.
//!
//! MariaDB speaks the same protocol and is served by this driver; the two are
//! distinguished only where their catalogs differ, which for the queries here
//! they do not.

mod types;

use async_trait::async_trait;
use tablex_core::{
    config::{ConnectionConfig, TlsMode},
    driver::{
        Capabilities, CompletionScope, Connection, Driver, DriverInfo, FetchOptions,
        PlaceholderStyle, RowEdit,
    },
    error::{is_connection_refused, root_cause, Error, Result},
    result::{Column, ColumnSource, QueryOutcome, ResultSet, StatementResult},
    schema::{decode_path, ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
};

use mysql_async::prelude::*;

const QUOTE: char = '`';

/// One folder in the object tree.
struct Folder {
    id: &'static str,
    label: &'static str,
    kind: NodeKind,
}

/// MySQL keeps functions and procedures in one `ROUTINES` view but they are
/// different things to call, so they are listed apart. There are no sequences
/// and no materialized views to offer.
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

/// `` `db`.`name` `` — MySQL qualifies by database, since it has no schema level.
fn qualify(db: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(db, QUOTE), quote_ident(name, QUOTE))
}

/// Catalogs that exist on every server and are noise for the user.
const HIDDEN_SCHEMAS: &str = "'information_schema', 'performance_schema', 'mysql', 'sys'";

pub struct MysqlDriver;

impl MysqlDriver {
    pub fn new() -> Self {
        MysqlDriver
    }
}

impl Default for MysqlDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for MysqlDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mysql".into(),
            name: "MySQL / MariaDB".into(),
            default_port: Some(3306),
            file_based: false,
            capabilities: Capabilities {
                transactions: true,
                multi_statement: true,
                explain: true,
                foreign_keys: true,
                views: true,
                stored_procedures: true,
                // MySQL's "schema" *is* its database — `information_schema`
                // exposes one `SCHEMATA` view for both — so there is no
                // intermediate level between a database and its tables.
                schemas: false,
                databases: true,
                // `org_table` and `org_name` come back in the column definition
                // packet, so results can be traced to their source.
                column_provenance: true,
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
        secret: Option<&str>,
    ) -> Result<Box<dyn Connection>> {
        let mut builder = mysql_async::OptsBuilder::default()
            .ip_or_hostname(config.host.clone().unwrap_or_else(|| "localhost".into()))
            .tcp_port(config.port.unwrap_or(3306))
            .user(config.username.clone())
            .pass(secret.map(str::to_string));

        if let Some(db) = &config.database {
            builder = builder.db_name(Some(db.clone()));
        }

        builder = match config.tls.mode {
            TlsMode::Disable => builder.ssl_opts(None),
            // MySQL servers very often present a self-signed certificate, so
            // "prefer" encrypts without asserting the chain. "Require and
            // verify" is the mode that actually authenticates the server.
            TlsMode::Prefer => builder.ssl_opts(Some(
                mysql_async::SslOpts::default()
                    .with_danger_accept_invalid_certs(true)
                    .with_danger_skip_domain_validation(true),
            )),
            TlsMode::VerifyFull => builder.ssl_opts(Some(mysql_async::SslOpts::default())),
        };

        let conn = mysql_async::Conn::new(builder).await.map_err(map_err)?;
        Ok(Box::new(MysqlConnection {
            conn,
            default_db: config.database.clone(),
        }))
    }
}

pub struct MysqlConnection {
    conn: mysql_async::Conn,
    /// The database named in the connection, used to qualify unqualified names.
    default_db: Option<String>,
}

#[async_trait]
impl Connection for MysqlConnection {
    async fn execute(&mut self, sql: &str, opts: &FetchOptions) -> Result<QueryOutcome> {
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Err(Error::query("no statement to execute"));
        }

        let started = std::time::Instant::now();
        let mut out = Vec::with_capacity(statements.len());
        for stmt in &statements {
            out.push(self.run_one(stmt, opts).await?);
        }

        Ok(QueryOutcome {
            statements: out,
            elapsed_ms: started.elapsed().as_millis() as u64,
            notices: Vec::new(),
        })
    }

    /// Paths are `[]`, `[database]`, `[database, folder]`, and
    /// `[database, folder, table]` for that table's columns.
    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
        let path = parent.map(decode_path).unwrap_or_default();
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();

        match segments.as_slice() {
            [] => self.browse_databases().await,
            [db] => Ok(FOLDERS
                .iter()
                .map(|f| SchemaNode::new(&[db, f.id], f.label, NodeKind::Folder).expandable())
                .collect()),
            [db, folder] => match FOLDERS.iter().find(|f| f.id == *folder) {
                Some(spec) => self.browse_folder(db, spec).await,
                None => Ok(Vec::new()),
            },
            [db, folder, table] => self.browse_columns(db, folder, table).await,
            _ => Ok(Vec::new()),
        }
    }

    async fn current_database(&mut self) -> Result<Option<String>> {
        Ok(self.default_db.clone())
    }

    /// `SHOW CREATE ...`, which returns the statement as the server stored it.
    ///
    /// `information_schema.ROUTINES.ROUTINE_DEFINITION` holds only the body —
    /// no `CREATE`, no parameter list, no `DETERMINISTIC` — so it cannot be run
    /// back, which is the whole point of showing it.
    async fn definition(&mut self, node_id: &str) -> Result<String> {
        let path = decode_path(node_id);
        let [db, folder, name] = path.as_slice() else {
            return Err(Error::Unsupported(
                "only an object has a definition to show".into(),
            ));
        };

        // The statement text is not in the same column for every object type:
        // routines and triggers report sql_mode first, views and tables do not.
        let (keyword, column) = match folder.as_str() {
            "functions" => ("FUNCTION", 2),
            "procedures" => ("PROCEDURE", 2),
            "triggers" => ("TRIGGER", 2),
            "views" => ("VIEW", 1),
            "tables" => ("TABLE", 1),
            other => {
                return Err(Error::Unsupported(format!(
                    "no definition available for {other}"
                )))
            }
        };

        let sql = format!("SHOW CREATE {keyword} {}", qualify(db, name));
        let row: Option<mysql_async::Row> = self.conn.query_first(sql).await.map_err(map_err)?;
        row.and_then(|r| r.get::<String, _>(column))
            .ok_or_else(|| Error::Unsupported(format!("{name} reported no definition")))
    }

    /// MySQL switches in-session, so no reconnection is needed.
    ///
    /// The name is quoted rather than bound: `USE` takes an identifier, and
    /// identifiers cannot be parameters in the protocol.
    async fn use_database(&mut self, database: &str) -> Result<()> {
        self.conn
            .query_drop(format!("USE {}", quote_ident(database, QUOTE)))
            .await
            .map_err(map_err)?;
        self.default_db = Some(database.to_string());
        Ok(())
    }

    async fn table_detail(&mut self, schema: Option<&str>, table: &str) -> Result<TableDetail> {
        let db = self.database_or_default(schema)?;

        let columns = self.columns(&db, table).await?;
        if columns.is_empty() {
            return Err(Error::Unsupported(format!(
                "table {db}.{table} not found or not visible"
            )));
        }

        let estimated: Option<Option<i64>> = self
            .conn
            .exec_first(
                "SELECT TABLE_ROWS FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
                (&db, table),
            )
            .await
            .map_err(map_err)?;

        let comment: Option<Option<String>> = self
            .conn
            .exec_first(
                "SELECT NULLIF(TABLE_COMMENT, '') FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
                (&db, table),
            )
            .await
            .map_err(map_err)?;

        let indexes = self.indexes(&db, table).await?;
        let primary_key = indexes
            .iter()
            .find(|i| i.primary)
            .map(|i| i.columns.clone())
            .unwrap_or_default();

        Ok(TableDetail {
            schema: Some(db.clone()),
            name: table.to_string(),
            columns,
            indexes,
            foreign_keys: self.foreign_keys(&db, table).await?,
            primary_key,
            // TABLE_ROWS is a storage-engine estimate on InnoDB, not a count.
            estimated_rows: estimated.flatten(),
            comment: comment.flatten(),
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

        let mut params: Vec<mysql_async::Value> = Vec::new();

        let assignments = edit
            .changes
            .iter()
            .map(|(col, val)| {
                params.push(param(val));
                format!("{} = ?", quote_ident(col, QUOTE))
            })
            .collect::<Vec<_>>()
            .join(", ");

        let predicate = edit
            .key
            .iter()
            .map(|(col, val)| {
                // `= NULL` is never true; only `IS NULL` matches.
                if val.is_null() {
                    return format!("{} IS NULL", quote_ident(col, QUOTE));
                }
                params.push(param(val));
                format!("{} = ?", quote_ident(col, QUOTE))
            })
            .collect::<Vec<_>>()
            .join(" AND ");

        let qualified = match &edit.schema {
            Some(db) => format!(
                "{}.{}",
                quote_ident(db, QUOTE),
                quote_ident(&edit.table, QUOTE)
            ),
            None => quote_ident(&edit.table, QUOTE),
        };
        let sql = format!("UPDATE {qualified} SET {assignments} WHERE {predicate}");

        // A transaction so a mismatch can be rolled back rather than left applied.
        let mut tx = self
            .conn
            .start_transaction(mysql_async::TxOpts::default())
            .await
            .map_err(map_err)?;

        tx.exec_drop(&sql, mysql_async::Params::Positional(params))
            .await
            .map_err(map_err)?;
        let affected = tx.affected_rows();

        // MySQL reports 0 affected when the new value equals the old one, so a
        // no-op edit is accepted rather than reported as a lost row.
        if affected > 1 {
            tx.rollback().await.map_err(map_err)?;
            return Err(Error::Query {
                message: format!(
                    "edit matched {affected} rows, expected at most 1 — \
                     the key is not unique"
                ),
                position: None,
                code: None,
            });
        }
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn ping(&mut self) -> Result<()> {
        self.conn.ping().await.map_err(map_err)
    }

    async fn close(&mut self) -> Result<()> {
        // The connection closes on drop; an explicit quit would need ownership.
        Ok(())
    }

    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        let schemas: Vec<String> = self
            .conn
            .query(format!(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
                 WHERE SCHEMA_NAME NOT IN ({HIDDEN_SCHEMAS}) ORDER BY SCHEMA_NAME"
            ))
            .await
            .map_err(map_err)?;

        // Columns of the *current* database only.
        //
        // This used to read every column of every database on the server. On a
        // development machine with a couple of dozen schemas that is hundreds of
        // thousands of rows out of `information_schema.COLUMNS`, and because a
        // session is serialized behind one mutex, the schema tree's own queries
        // queued behind it — expanding a database appeared to take seconds when
        // what it was actually doing was waiting for autocomplete.
        //
        // Scoping it is also the more correct answer: an unqualified name in the
        // editor resolves in the current database, so those are the identifiers
        // completion should offer first.
        let Some(database) = self.default_db.clone() else {
            // No database selected yet: the schema list is still useful, and
            // there is no sensible set of tables to complete against.
            return Ok(CompletionScope {
                schemas,
                tables: Vec::new(),
                functions: MYSQL_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
                keywords: Vec::new(),
            });
        };

        // One query for every column rather than one per table: within a
        // database the round trips would still dominate.
        let rows: Vec<(String, String)> = self
            .conn
            .exec(
                "SELECT TABLE_NAME, COLUMN_NAME FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, ORDINAL_POSITION",
                (&database,),
            )
            .await
            .map_err(map_err)?;

        let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
        for (table, column) in rows {
            match grouped.last_mut() {
                Some((name, cols)) if *name == table => cols.push(column),
                _ => grouped.push((table, vec![column])),
            }
        }

        Ok(CompletionScope {
            schemas,
            tables: grouped,
            functions: MYSQL_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
            keywords: Vec::new(),
        })
    }
}

impl MysqlConnection {
    fn database_or_default(&self, schema: Option<&str>) -> Result<String> {
        schema
            .map(str::to_string)
            .or_else(|| self.default_db.clone())
            .ok_or_else(|| {
                Error::Config(
                    "no database selected; qualify the name or set one on the connection".into(),
                )
            })
    }

    async fn run_one(&mut self, sql: &str, opts: &FetchOptions) -> Result<StatementResult> {
        let mut result = self.conn.query_iter(sql).await.map_err(map_err)?;

        // An empty column set means the statement returned no rows — a write or
        // DDL. More reliable than sniffing the leading keyword, which misjudges
        // CTEs and `INSERT ... RETURNING` on MariaDB.
        let meta = result.columns();
        let Some(meta) = meta else {
            let affected = result.affected_rows();
            let last_insert_id = result.last_insert_id();
            result.drop_result().await.map_err(map_err)?;
            return Ok(StatementResult::Affected {
                rows_affected: affected,
                last_insert_id: last_insert_id.and_then(|id| i64::try_from(id).ok()),
            });
        };
        let meta = meta.to_vec();

        let columns: Vec<Column> = meta
            .iter()
            .map(|c| {
                let table = c.org_table_str().to_string();
                Column {
                    name: c.name_str().to_string(),
                    type_name: type_name(c),
                    nullable: Some(
                        !c.flags()
                            .contains(mysql_async::consts::ColumnFlags::NOT_NULL_FLAG),
                    ),
                    // An empty org_table means the column is computed, so there
                    // is nothing to edit.
                    source: (!table.is_empty()).then(|| ColumnSource {
                        schema: Some(c.schema_str().to_string()).filter(|s| !s.is_empty()),
                        table,
                        column: c.org_name_str().to_string(),
                    }),
                }
            })
            .collect();

        let rows: Vec<mysql_async::Row> = result.collect().await.map_err(map_err)?;

        let cap = opts.max_rows.unwrap_or(usize::MAX);
        let truncated = rows.len().saturating_sub(opts.offset) > cap;

        let decoded: Vec<Vec<tablex_core::Value>> = rows
            .iter()
            .skip(opts.offset)
            .take(cap)
            .map(|row| {
                meta.iter()
                    .enumerate()
                    .map(|(i, column)| {
                        let raw = row.as_ref(i).cloned().unwrap_or(mysql_async::Value::NULL);
                        types::decode(&raw, column)
                    })
                    .collect()
            })
            .collect();

        let key_columns = self.edit_key_for(&columns).await;

        let mut rs = ResultSet {
            columns,
            rows: decoded,
            truncated,
            editable: false,
            key_columns,
        };
        rs.recompute_editable();
        Ok(StatementResult::Rows(rs))
    }

    /// The primary key of the single source table, when the projection contains
    /// all of it. Empty otherwise, which forces the grid read-only.
    async fn edit_key_for(&mut self, columns: &[Column]) -> Vec<String> {
        let mut sources = columns.iter().filter_map(|c| c.source.as_ref());
        let Some(first) = sources.next() else {
            return Vec::new();
        };
        if !sources.all(|s| s.table == first.table && s.schema == first.schema) {
            return Vec::new();
        }

        let Ok(db) = self.database_or_default(first.schema.as_deref()) else {
            return Vec::new();
        };
        let Ok(indexes) = self.indexes(&db, &first.table).await else {
            return Vec::new();
        };
        let Some(pk) = indexes.iter().find(|i| i.primary) else {
            return Vec::new();
        };

        // A key the user cannot see is a key we cannot put in a WHERE clause.
        if pk.columns.iter().all(|k| {
            columns
                .iter()
                .any(|c| c.source.as_ref().is_some_and(|s| &s.column == k))
        }) {
            pk.columns.clone()
        } else {
            Vec::new()
        }
    }

    async fn browse_databases(&mut self) -> Result<Vec<SchemaNode>> {
        let names: Vec<String> = self
            .conn
            .query(format!(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA                  WHERE SCHEMA_NAME NOT IN ({HIDDEN_SCHEMAS}) ORDER BY SCHEMA_NAME"
            ))
            .await
            .map_err(map_err)?;

        let current = self.default_db.clone();
        Ok(names
            .into_iter()
            .map(|name| {
                let node = SchemaNode::new(&[&name], &name, NodeKind::Database)
                    .expandable()
                    .qualified(quote_ident(&name, QUOTE));
                if current.as_deref() == Some(name.as_str()) {
                    node.detail("in use")
                } else {
                    node
                }
            })
            .collect())
    }

    /// Objects of one kind within one database.
    ///
    /// MySQL has no level between database and table — its `SCHEMATA` view is
    /// the database list — so folders hang directly off the database.
    async fn browse_folder(&mut self, db: &str, spec: &Folder) -> Result<Vec<SchemaNode>> {
        match spec.id {
            "tables" | "views" => self.browse_tables(db, spec).await,
            "functions" | "procedures" => self.browse_routines(db, spec).await,
            "triggers" => self.browse_triggers(db, spec).await,
            _ => Ok(Vec::new()),
        }
    }

    async fn browse_tables(&mut self, db: &str, spec: &Folder) -> Result<Vec<SchemaNode>> {
        // BASE TABLE and VIEW are the two values that matter; SYSTEM VIEW
        // appears only in the schemas already hidden from the database list.
        let wanted = if spec.id == "views" {
            "VIEW"
        } else {
            "BASE TABLE"
        };
        let rows: Vec<(String, Option<i64>)> = self
            .conn
            .exec(
                "SELECT TABLE_NAME, TABLE_ROWS FROM information_schema.TABLES                  WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = ? ORDER BY TABLE_NAME",
                (db, wanted),
            )
            .await
            .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|(name, count)| {
                let node = SchemaNode::new(&[db, spec.id, &name], &name, spec.kind.clone())
                    .expandable()
                    .qualified(qualify(db, &name));
                // An estimate from the engine, not a COUNT(*).
                match count {
                    Some(n) => node.detail(format!("~{n} rows")),
                    None => node,
                }
            })
            .collect())
    }

    async fn browse_routines(&mut self, db: &str, spec: &Folder) -> Result<Vec<SchemaNode>> {
        let wanted = if spec.id == "procedures" {
            "PROCEDURE"
        } else {
            "FUNCTION"
        };
        let rows: Vec<(String, Option<String>)> = self
            .conn
            .exec(
                "SELECT ROUTINE_NAME, DTD_IDENTIFIER FROM information_schema.ROUTINES                  WHERE ROUTINE_SCHEMA = ? AND ROUTINE_TYPE = ? ORDER BY ROUTINE_NAME",
                (db, wanted),
            )
            .await
            .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|(name, returns)| {
                let node = SchemaNode::new(&[db, spec.id, &name], &name, spec.kind.clone())
                    .qualified(qualify(db, &name));
                // What a function gives back is the thing you need to know
                // before calling it; a procedure returns nothing to report.
                match returns {
                    Some(ty) if spec.id == "functions" => node.detail(format!("returns {ty}")),
                    _ => node,
                }
            })
            .collect())
    }

    async fn browse_triggers(&mut self, db: &str, spec: &Folder) -> Result<Vec<SchemaNode>> {
        let rows: Vec<(String, String, String, String)> = self
            .conn
            .exec(
                "SELECT TRIGGER_NAME, EVENT_OBJECT_TABLE, ACTION_TIMING, EVENT_MANIPULATION                  FROM information_schema.TRIGGERS                  WHERE TRIGGER_SCHEMA = ? ORDER BY EVENT_OBJECT_TABLE, TRIGGER_NAME",
                (db,),
            )
            .await
            .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|(name, table, timing, event)| {
                // "BEFORE INSERT on orders" - when it fires and on what, which
                // is the whole of what a trigger listing can usefully say.
                SchemaNode::new(&[db, spec.id, &name], &name, spec.kind.clone())
                    .detail(format!("{timing} {event} on {table}"))
            })
            .collect())
    }

    async fn browse_columns(
        &mut self,
        db: &str,
        folder: &str,
        table: &str,
    ) -> Result<Vec<SchemaNode>> {
        Ok(self
            .columns(db, table)
            .await?
            .into_iter()
            .map(|c| {
                SchemaNode::new(&[db, folder, table, &c.name], &c.name, NodeKind::Column).detail(
                    if c.nullable {
                        c.type_name
                    } else {
                        format!("{} NOT NULL", c.type_name)
                    },
                )
            })
            .collect())
    }

    async fn columns(&mut self, db: &str, table: &str) -> Result<Vec<ColumnDef>> {
        let rows: Vec<ColumnRow> = self
            .conn
            .exec(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, \
                        ORDINAL_POSITION, NULLIF(COLUMN_COMMENT, '') \
                 FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                 ORDER BY ORDINAL_POSITION",
                (db, table),
            )
            .await
            .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(
                |(name, type_name, nullable, default, extra, ordinal, comment)| ColumnDef {
                    name,
                    type_name,
                    nullable: nullable == "YES",
                    default,
                    auto_increment: extra.contains("auto_increment"),
                    ordinal,
                    comment,
                },
            )
            .collect())
    }

    async fn indexes(&mut self, db: &str, table: &str) -> Result<Vec<IndexDef>> {
        // information_schema.STATISTICS returns one row per indexed column;
        // SEQ_IN_INDEX gives the order within a composite key.
        let rows: Vec<(String, u8, String, String, u32)> = self
            .conn
            .exec(
                "SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME, INDEX_TYPE, SEQ_IN_INDEX \
                 FROM information_schema.STATISTICS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                 ORDER BY INDEX_NAME, SEQ_IN_INDEX",
                (db, table),
            )
            .await
            .map_err(map_err)?;

        let mut grouped: Vec<IndexDef> = Vec::new();
        for (name, non_unique, column, method, _seq) in rows {
            match grouped.last_mut() {
                Some(last) if last.name == name => last.columns.push(column),
                _ => grouped.push(IndexDef {
                    primary: name == "PRIMARY",
                    unique: non_unique == 0,
                    name,
                    columns: vec![column],
                    method: Some(method),
                }),
            }
        }
        Ok(grouped)
    }

    async fn foreign_keys(&mut self, db: &str, table: &str) -> Result<Vec<ForeignKeyDef>> {
        let rows: Vec<(String, String, String, String, String, String)> = self
            .conn
            .exec(
                "SELECT k.CONSTRAINT_NAME, k.COLUMN_NAME, \
                        k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, \
                        k.REFERENCED_COLUMN_NAME, \
                        CONCAT(r.DELETE_RULE, '|', r.UPDATE_RULE) \
                 FROM information_schema.KEY_COLUMN_USAGE k \
                 JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
                   ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA \
                  AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
                 WHERE k.TABLE_SCHEMA = ? AND k.TABLE_NAME = ? \
                   AND k.REFERENCED_TABLE_NAME IS NOT NULL \
                 ORDER BY k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
                (db, table),
            )
            .await
            .map_err(map_err)?;

        let mut grouped: Vec<ForeignKeyDef> = Vec::new();
        for (name, column, ref_schema, ref_table, ref_column, rules) in rows {
            let (on_delete, on_update) = rules.split_once('|').unwrap_or(("", ""));
            match grouped.last_mut() {
                Some(last) if last.name == name => {
                    last.columns.push(column);
                    last.referenced_columns.push(ref_column);
                }
                _ => grouped.push(ForeignKeyDef {
                    name,
                    columns: vec![column],
                    referenced_schema: Some(ref_schema),
                    referenced_table: ref_table,
                    referenced_columns: vec![ref_column],
                    on_delete: Some(on_delete.to_string()),
                    on_update: Some(on_update.to_string()),
                }),
            }
        }
        Ok(grouped)
    }
}

/// One row of the `information_schema.COLUMNS` projection:
/// `(name, type, is_nullable, default, extra, ordinal, comment)`.
type ColumnRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    i32,
    Option<String>,
);

fn param(v: &tablex_core::Value) -> mysql_async::Value {
    match types::to_param(v) {
        Some(text) => mysql_async::Value::Bytes(text.into_bytes()),
        None => mysql_async::Value::NULL,
    }
}

/// Display name for a result column. The protocol reports a type code rather
/// than the declared type, so this is the closest honest rendering.
fn type_name(column: &mysql_async::Column) -> String {
    format!("{:?}", column.column_type())
        .trim_start_matches("MYSQL_TYPE_")
        .to_lowercase()
}

/// Normalize driver errors into the shared taxonomy.
pub(crate) fn map_err(e: mysql_async::Error) -> Error {
    use mysql_async::Error as E;
    match &e {
        E::Server(server) => {
            // 1045 access denied, 1044 access denied for database,
            // 1698 auth plugin rejection.
            if matches!(server.code, 1045 | 1044 | 1698) {
                return Error::Auth(server.message.clone());
            }
            // 1049 unknown database, 2002/2003 cannot reach the server.
            if matches!(server.code, 1049 | 2002 | 2003) {
                return Error::Connection(server.message.clone());
            }
            Error::Query {
                message: server.message.clone(),
                // MySQL does not report a character offset for syntax errors,
                // so the editor cannot underline the token the way it can for
                // PostgreSQL.
                position: None,
                code: Some(server.state.clone()),
            }
        }
        // Report what actually failed rather than the two layers of
        // "Input/output error:" the crate wraps around it.
        E::Io(_) if is_connection_refused(&e) => Error::Connection(
            "nothing is listening on that host and port — check the server is \
             running and that the port is the one it listens on"
                .into(),
        ),
        E::Io(_) => Error::Network(root_cause(&e)),
        E::Driver(_) => Error::Connection(root_cause(&e)),
        _ => Error::query(root_cause(&e)),
    }
}

const MYSQL_FUNCTIONS: &[&str] = &[
    "abs",
    "avg",
    "cast",
    "ceil",
    "char_length",
    "coalesce",
    "concat",
    "concat_ws",
    "count",
    "curdate",
    "current_timestamp",
    "date_add",
    "date_format",
    "date_sub",
    "datediff",
    "floor",
    "greatest",
    "group_concat",
    "if",
    "ifnull",
    "json_extract",
    "json_object",
    "last_insert_id",
    "least",
    "left",
    "length",
    "lower",
    "lpad",
    "ltrim",
    "max",
    "min",
    "now",
    "nullif",
    "rand",
    "replace",
    "right",
    "round",
    "rpad",
    "rtrim",
    "substring",
    "sum",
    "trim",
    "unix_timestamp",
    "upper",
    "uuid",
];

#[cfg(test)]
mod tests;
