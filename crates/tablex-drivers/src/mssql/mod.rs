//! Microsoft SQL Server driver.
//!
//! Two things shape this driver more than anything else:
//!
//! - **No column provenance.** The TDS `COLMETADATA` token carries a table name,
//!   but tiberius does not surface it, so a result set cannot be traced back to
//!   its source table. Ad-hoc query results are therefore read-only, as they are
//!   for SQLite. Browsing a table still knows what it asked for.
//! - **No affected-row count on the query path.** `QueryItem` yields only
//!   metadata and rows, so a statement that returns no rows arrives with no
//!   count attached. Re-running it through `execute` would run a write twice,
//!   which is unacceptable, so the count comes from `SELECT @@ROWCOUNT` on the
//!   same session instead.

mod activity;
mod types;

use async_trait::async_trait;
use tablex_core::{
    activity::ServerActivity,
    config::{ConnectionConfig, TlsMode},
    driver::{
        Capabilities, CompletionScope, Connection, Driver, DriverInfo, FetchOptions,
        PlaceholderStyle, RowEdit, RowSink, STREAM_BATCH,
    },
    error::{Error, Result},
    result::{Column, QueryOutcome, ResultSet, StatementResult},
    schema::{decode_path, ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
};
use tiberius::{AuthMethod, Client, Config, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use futures_util::StreamExt;

/// SQL Server quotes identifiers with brackets; the closing bracket doubles.
const QUOTE: char = '[';

/// Schemas that ship with every database and are noise for the user.
const HIDDEN_SCHEMAS: &str = "'sys', 'INFORMATION_SCHEMA', 'guest', 'db_owner', \
     'db_accessadmin', 'db_securityadmin', 'db_ddladmin', 'db_backupoperator', \
     'db_datareader', 'db_datawriter', 'db_denydatareader', 'db_denydatawriter'";

/// One folder in the object tree.
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

/// Column metadata as TDS reports it.
///
/// No source table: see the module docs — tiberius does not surface the one the
/// protocol carries, which is why results here are read-only.
fn describe(meta: &tiberius::ResultMetadata) -> Vec<Column> {
    meta.columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_string(),
            type_name: types::type_name(c.column_type()),
            nullable: None,
            source: None,
        })
        .collect()
}

/// `[db].[schema].[name]` — the three-part name that also works from another
/// database, which is exactly the case the tree makes reachable.
fn qualify(db: &str, schema: &str, name: &str) -> String {
    format!(
        "{}.{}.{}",
        quote_ident(db, QUOTE),
        quote_ident(schema, QUOTE),
        quote_ident(name, QUOTE)
    )
}

pub struct MssqlDriver;

impl MssqlDriver {
    pub fn new() -> Self {
        MssqlDriver
    }
}

impl Default for MssqlDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for MssqlDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mssql".into(),
            name: "SQL Server".into(),
            default_port: Some(1433),
            file_based: false,
            capabilities: Capabilities {
                transactions: true,
                multi_statement: true,
                explain: true,
                schemas: true,
                databases: true,
                foreign_keys: true,
                views: true,
                stored_procedures: true,
                // See the module docs: tiberius does not expose the originating
                // table, so ad-hoc results cannot be safely edited.
                table_scripts: false,
                column_provenance: false,
                cancel: false,
                // QueryItems arrive as the server sends them.
                streaming: true,
                activity: true,
                placeholder_style: PlaceholderStyle::AtP,
                identifier_quote: QUOTE,
            },
        }
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        secret: Option<&str>,
    ) -> Result<Box<dyn Connection>> {
        let mut tds = Config::new();
        tds.host(config.host.clone().unwrap_or_else(|| "localhost".into()));
        tds.port(config.port.unwrap_or(1433));
        if let Some(db) = &config.database {
            tds.database(db);
        }
        tds.authentication(AuthMethod::sql_server(
            config.username.clone().unwrap_or_default(),
            secret.unwrap_or_default(),
        ));

        match config.tls.mode {
            // SQL Server encrypts the login packet even when encryption is
            // "off", so there is no true plaintext mode to select here.
            TlsMode::Disable | TlsMode::Prefer => tds.trust_cert(),
            TlsMode::VerifyFull => {}
        }

        let addr = tds.get_addr();
        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| Error::Connection(format!("could not reach {addr}: {e}")))?;
        // Nagle's algorithm adds latency to the small round trips a query
        // client makes constantly.
        tcp.set_nodelay(true)
            .map_err(|e| Error::Network(e.to_string()))?;

        let client = Client::connect(tds, tcp.compat_write())
            .await
            .map_err(map_err)?;

        let mut connection = MssqlConnection {
            client,
            default_schema: "dbo".to_string(),
            database: String::new(),
        };
        // Asked rather than taken from the config: with no database named, the
        // server puts you in the login's default, and the tree has to mark the
        // one you are actually in.
        connection.database = connection
            .scalar_string("SELECT DB_NAME()")
            .await?
            .unwrap_or_else(|| "master".to_string());
        Ok(Box::new(connection))
    }
}

pub struct MssqlConnection {
    client: Client<Compat<TcpStream>>,
    /// SQL Server's default schema; used when a caller does not name one.
    default_schema: String,
    /// The database `USE` last selected, and what unqualified names resolve in.
    database: String,
}

#[async_trait]
impl Connection for MssqlConnection {
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

    /// Paths are `[]`, `[database]`, `[database, schema]`,
    /// `[database, schema, folder]`, and `[database, schema, folder, object]`.
    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
        let path = parent.map(decode_path).unwrap_or_default();
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();

        match segments.as_slice() {
            [] => self.browse_databases().await,
            [db] => self.browse_schemas(db).await,
            [db, schema] => Ok(FOLDERS
                .iter()
                .map(|f| {
                    SchemaNode::new(&[db, schema, f.id], f.label, NodeKind::Folder).expandable()
                })
                .collect()),
            [db, schema, folder] => match FOLDERS.iter().find(|f| f.id == *folder) {
                Some(spec) => self.browse_folder(db, schema, spec).await,
                None => Ok(Vec::new()),
            },
            [db, schema, folder, object] => self.browse_columns(db, schema, folder, object).await,
            _ => Ok(Vec::new()),
        }
    }

    async fn current_database(&mut self) -> Result<Option<String>> {
        Ok(Some(self.database.clone()))
    }

    async fn activity(&mut self) -> Result<ServerActivity> {
        activity::activity(&mut self.client).await
    }

    async fn kill_session(&mut self, id: &str) -> Result<()> {
        activity::kill(&mut self.client, id).await
    }

    /// Stream rows off the TDS stream.
    ///
    /// tiberius already yields `QueryItem`s as they arrive; the buffered path
    /// collects them only because the grid wants a whole page at once. Here
    /// they are decoded and handed on in batches, so nothing accumulates.
    async fn stream(
        &mut self,
        sql: &str,
        opts: &FetchOptions,
        sink: &mut dyn RowSink,
    ) -> Result<u64> {
        let mut stream = self.client.simple_query(sql).await.map_err(map_err)?;

        let cap = opts.max_rows.unwrap_or(usize::MAX);
        let mut described = false;
        let mut batch: Vec<Vec<tablex_core::Value>> = Vec::with_capacity(STREAM_BATCH);
        let mut total = 0u64;

        while let Some(item) = stream.next().await {
            match item.map_err(map_err)? {
                QueryItem::Metadata(meta) => {
                    // A batch of statements can carry several metadata tokens;
                    // the first describes the result being streamed.
                    if !described {
                        sink.columns(&describe(&meta))?;
                        described = true;
                    }
                }
                QueryItem::Row(row) => {
                    if total as usize >= cap {
                        break;
                    }
                    batch.push(row.cells().map(|(_, data)| types::decode(data)).collect());
                    total += 1;
                    if batch.len() >= STREAM_BATCH {
                        sink.rows(&batch)?;
                        batch.clear();
                    }
                }
            }
        }

        if !described {
            return Err(Error::Unsupported(
                "that statement returns no rows to stream".into(),
            ));
        }
        if !batch.is_empty() {
            sink.rows(&batch)?;
        }
        Ok(total)
    }

    /// `OBJECT_DEFINITION` returns the module text as it was submitted.
    ///
    /// Encrypted modules (`WITH ENCRYPTION`) return NULL rather than an error,
    /// which is worth saying plainly: the text is not missing, the server is
    /// refusing to hand it over.
    async fn definition(&mut self, node_id: &str) -> Result<String> {
        let path = decode_path(node_id);
        let [db, schema, folder, name] = path.as_slice() else {
            return Err(Error::Unsupported(
                "only an object has a definition to show".into(),
            ));
        };
        if !matches!(
            folder.as_str(),
            "functions" | "procedures" | "triggers" | "views"
        ) {
            return Err(Error::Unsupported(format!(
                "no definition available for {folder}"
            )));
        }

        // OBJECT_ID resolves a three-part name, so this reads another database's
        // module without a USE — the same property the catalogue queries rely on.
        let target = escape_literal(&qualify(db, schema, name));
        let sql = format!("SELECT OBJECT_DEFINITION(OBJECT_ID('{target}'))");
        match self.scalar_string(&sql).await? {
            Some(text) => Ok(text),
            None => Err(Error::Unsupported(format!(
                "{name} has no readable definition — it may be encrypted, or a system object"
            ))),
        }
    }

    /// `USE` switches the session, so no reconnection is needed.
    ///
    /// Browsing does not depend on this — the catalogue queries name their
    /// database explicitly — but the editor's unqualified statements do, and
    /// that is what the user means by selecting a database.
    async fn use_database(&mut self, database: &str) -> Result<()> {
        self.client
            .simple_query(format!("USE {}", quote_ident(database, QUOTE)))
            .await
            .map_err(map_err)?;
        self.database = database.to_string();
        Ok(())
    }

    async fn table_detail(&mut self, schema: Option<&str>, table: &str) -> Result<TableDetail> {
        let schema = schema.unwrap_or(&self.default_schema).to_string();

        let columns = self.columns(&schema, table).await?;
        if columns.is_empty() {
            return Err(Error::Unsupported(format!(
                "table {schema}.{table} not found or not visible"
            )));
        }

        let indexes = self.indexes(&schema, table).await?;
        let primary_key = indexes
            .iter()
            .find(|i| i.primary)
            .map(|i| i.columns.clone())
            .unwrap_or_default();

        // sys.dm_db_partition_stats is an estimate maintained by the engine;
        // an exact COUNT(*) on a large table is far too expensive for a sidebar.
        let estimated = self
            .scalar_i64(&format!(
                "SELECT SUM(p.row_count) FROM sys.dm_db_partition_stats p \
                 JOIN sys.objects o ON o.object_id = p.object_id \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE s.name = '{}' AND o.name = '{}' AND p.index_id IN (0, 1)",
                escape_literal(&schema),
                escape_literal(table)
            ))
            .await
            .ok()
            .flatten();

        Ok(TableDetail {
            schema: Some(schema.clone()),
            name: table.to_string(),
            columns,
            indexes,
            foreign_keys: self.foreign_keys(&schema, table).await?,
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

        let predicate = edit
            .key
            .iter()
            .map(|(col, val)| {
                // `= NULL` is never true in SQL; only `IS NULL` matches.
                if val.is_null() {
                    format!("{} IS NULL", quote_ident(col, QUOTE))
                } else {
                    format!("{} = {}", quote_ident(col, QUOTE), types::literal(val))
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ");

        let schema = edit.schema.as_deref().unwrap_or(&self.default_schema);
        let qualified = format!(
            "{}.{}",
            quote_ident(schema, QUOTE),
            quote_ident(&edit.table, QUOTE)
        );

        // The whole edit runs as one batch so the rollback is atomic: if the key
        // turns out not to be unique, nothing is left applied.
        let batch = format!(
            "BEGIN TRANSACTION; \
             UPDATE {qualified} SET {assignments} WHERE {predicate}; \
             IF @@ROWCOUNT > 1 BEGIN ROLLBACK TRANSACTION; \
                 THROW 51000, 'edit matched more than one row, expected at most 1 - the key is not unique', 1; \
             END \
             COMMIT TRANSACTION;"
        );

        self.client
            .simple_query(batch)
            .await
            .map_err(map_err)?
            .into_results()
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn ping(&mut self) -> Result<()> {
        self.client
            .simple_query("SELECT 1")
            .await
            .map_err(map_err)?
            .into_results()
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        // The socket closes on drop; `Client::close` consumes self.
        Ok(())
    }

    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        let schemas = self
            .strings(&format!(
                "SELECT name FROM sys.schemas WHERE name NOT IN ({HIDDEN_SCHEMAS}) ORDER BY name"
            ))
            .await?;

        // One query for every column rather than one per table.
        let rows = self
            .pairs(
                "SELECT s.name + '.' + t.name, c.name \
                 FROM sys.columns c \
                 JOIN sys.tables t ON t.object_id = c.object_id \
                 JOIN sys.schemas s ON s.schema_id = t.schema_id \
                 ORDER BY s.name, t.name, c.column_id",
            )
            .await?;

        let mut tables: Vec<(String, Vec<String>)> = Vec::new();
        for (table, column) in rows {
            match tables.last_mut() {
                Some((name, cols)) if *name == table => cols.push(column),
                _ => tables.push((table, vec![column])),
            }
        }

        Ok(CompletionScope {
            schemas,
            tables,
            functions: MSSQL_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
            keywords: Vec::new(),
        })
    }
}

impl MssqlConnection {
    async fn run_one(&mut self, sql: &str, opts: &FetchOptions) -> Result<StatementResult> {
        let mut stream = self.client.simple_query(sql).await.map_err(map_err)?;

        let mut columns: Option<Vec<Column>> = None;
        let mut rows: Vec<tiberius::Row> = Vec::new();

        while let Some(item) = stream.next().await {
            match item.map_err(map_err)? {
                QueryItem::Metadata(meta) => {
                    if columns.is_none() {
                        columns = Some(describe(&meta));
                    }
                }
                QueryItem::Row(row) => rows.push(row),
            }
        }
        // The stream borrows `self.client` until it is dropped, and the
        // @@ROWCOUNT lookup below needs the client back.
        drop(stream);

        let Some(columns) = columns else {
            // No metadata means the statement returned no result set, so it was
            // a write or DDL. `@@ROWCOUNT` on the same session reports what the
            // previous statement affected — re-running the statement through
            // `execute` to get a count would apply the write twice.
            let affected = self.scalar_i64("SELECT @@ROWCOUNT").await?.unwrap_or(0);
            return Ok(StatementResult::Affected {
                rows_affected: affected.max(0) as u64,
                last_insert_id: None,
            });
        };

        let cap = opts.max_rows.unwrap_or(usize::MAX);
        let truncated = rows.len().saturating_sub(opts.offset) > cap;

        let decoded: Vec<Vec<tablex_core::Value>> = rows
            .iter()
            .skip(opts.offset)
            .take(cap)
            .map(|row| row.cells().map(|(_, data)| types::decode(data)).collect())
            .collect();

        let mut rs = ResultSet {
            columns,
            rows: decoded,
            truncated,
            editable: false,
            key_columns: Vec::new(),
        };
        rs.recompute_editable();
        Ok(StatementResult::Rows(rs))
    }

    /// Run a query returning a single nullable string.
    async fn scalar_string(&mut self, sql: &str) -> Result<Option<String>> {
        let row = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_row()
            .await
            .map_err(map_err)?;
        Ok(row.and_then(|r| r.get::<&str, _>(0).map(str::to_string)))
    }

    /// Run a query returning a single nullable integer.
    async fn scalar_i64(&mut self, sql: &str) -> Result<Option<i64>> {
        let row = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_row()
            .await
            .map_err(map_err)?;
        Ok(row.and_then(|r| {
            r.get::<i64, _>(0)
                .or_else(|| r.get::<i32, _>(0).map(i64::from))
        }))
    }

    async fn strings(&mut self, sql: &str) -> Result<Vec<String>> {
        let rows = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_first_result()
            .await
            .map_err(map_err)?;
        Ok(rows
            .iter()
            .filter_map(|r| r.get::<&str, _>(0).map(str::to_string))
            .collect())
    }

    async fn pairs(&mut self, sql: &str) -> Result<Vec<(String, String)>> {
        let rows = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_first_result()
            .await
            .map_err(map_err)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some((
                    r.get::<&str, _>(0)?.to_string(),
                    r.get::<&str, _>(1)?.to_string(),
                ))
            })
            .collect())
    }

    async fn browse_databases(&mut self) -> Result<Vec<SchemaNode>> {
        // state = 0 is ONLINE. A database being restored or taken offline
        // cannot be read, and listing it as browsable only produces an error
        // when the user clicks it.
        let names = self
            .strings(
                "SELECT name FROM sys.databases                  WHERE state = 0 AND name NOT IN ('master', 'tempdb', 'model', 'msdb')                  ORDER BY name",
            )
            .await?;

        let current = self.database.clone();
        Ok(names
            .into_iter()
            .map(|name| {
                let node = SchemaNode::new(&[&name], &name, NodeKind::Database)
                    .expandable()
                    .qualified(quote_ident(&name, QUOTE));
                if name == current {
                    node.detail("in use")
                } else {
                    node
                }
            })
            .collect())
    }

    /// Schemas of `db`.
    ///
    /// Every catalogue query here is prefixed with the database name rather
    /// than issuing `USE` first: three-part naming reads another database's
    /// catalogue without disturbing the session the editor is running in.
    async fn browse_schemas(&mut self, db: &str) -> Result<Vec<SchemaNode>> {
        Ok(self
            .strings(&format!(
                "SELECT name FROM {}.sys.schemas WHERE name NOT IN ({HIDDEN_SCHEMAS}) ORDER BY name",
                quote_ident(db, QUOTE)
            ))
            .await?
            .into_iter()
            .map(|name| {
                SchemaNode::new(&[db, &name], &name, NodeKind::Schema)
                    .expandable()
                    .qualified(quote_ident(&name, QUOTE))
            })
            .collect())
    }

    async fn browse_folder(
        &mut self,
        db: &str,
        schema: &str,
        spec: &Folder,
    ) -> Result<Vec<SchemaNode>> {
        match spec.id {
            "triggers" => self.browse_triggers(db, schema, spec).await,
            _ => self.browse_objects(db, schema, spec).await,
        }
    }

    /// Tables, views, functions, and procedures all live in `sys.objects`,
    /// separated by `type`.
    async fn browse_objects(
        &mut self,
        db: &str,
        schema: &str,
        spec: &Folder,
    ) -> Result<Vec<SchemaNode>> {
        // FN/IF/TF are scalar, inline table-valued, and multi-statement
        // table-valued functions; all three are things you call.
        let types = match spec.id {
            "tables" => "'U'",
            "views" => "'V'",
            "functions" => "'FN','IF','TF'",
            "procedures" => "'P'",
            _ => return Ok(Vec::new()),
        };

        let rows = self
            .strings(&format!(
                "SELECT o.name FROM {db}.sys.objects o                  JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id                  WHERE s.name = '{schema}' AND o.type IN ({types}) ORDER BY o.name",
                db = quote_ident(db, QUOTE),
                schema = escape_literal(schema),
                types = types
            ))
            .await?;

        Ok(rows
            .into_iter()
            .map(|name| {
                let node = SchemaNode::new(&[db, schema, spec.id, &name], &name, spec.kind.clone())
                    .qualified(qualify(db, schema, &name));
                // Only relations have columns to expand into.
                if matches!(spec.id, "tables" | "views") {
                    node.expandable()
                } else {
                    node
                }
            })
            .collect())
    }

    async fn browse_triggers(
        &mut self,
        db: &str,
        schema: &str,
        spec: &Folder,
    ) -> Result<Vec<SchemaNode>> {
        let rows = self
            .pairs(&format!(
                "SELECT tr.name, t.name FROM {db}.sys.triggers tr                  JOIN {db}.sys.tables t ON t.object_id = tr.parent_id                  JOIN {db}.sys.schemas s ON s.schema_id = t.schema_id                  WHERE s.name = '{schema}' AND tr.is_ms_shipped = 0                  ORDER BY t.name, tr.name",
                db = quote_ident(db, QUOTE),
                schema = escape_literal(schema)
            ))
            .await?;

        Ok(rows
            .into_iter()
            .map(|(name, table)| {
                SchemaNode::new(&[db, schema, spec.id, &name], &name, spec.kind.clone())
                    .detail(format!("on {table}"))
            })
            .collect())
    }

    async fn browse_columns(
        &mut self,
        db: &str,
        schema: &str,
        folder: &str,
        table: &str,
    ) -> Result<Vec<SchemaNode>> {
        Ok(self
            .columns(schema, table)
            .await?
            .into_iter()
            .map(|c| {
                SchemaNode::new(
                    &[db, schema, folder, table, &c.name],
                    &c.name,
                    NodeKind::Column,
                )
                .detail(if c.nullable {
                    c.type_name
                } else {
                    format!("{} NOT NULL", c.type_name)
                })
            })
            .collect())
    }

    async fn columns(&mut self, schema: &str, table: &str) -> Result<Vec<ColumnDef>> {
        let sql = format!(
            "SELECT c.COLUMN_NAME, \
                    c.DATA_TYPE + COALESCE('(' + \
                        CASE WHEN c.CHARACTER_MAXIMUM_LENGTH = -1 THEN 'max' \
                             ELSE CAST(c.CHARACTER_MAXIMUM_LENGTH AS varchar(12)) END + ')', ''), \
                    c.IS_NULLABLE, \
                    COALESCE(c.COLUMN_DEFAULT, ''), \
                    CAST(COLUMNPROPERTY(OBJECT_ID(c.TABLE_SCHEMA + '.' + c.TABLE_NAME), \
                                        c.COLUMN_NAME, 'IsIdentity') AS varchar(4)), \
                    CAST(c.ORDINAL_POSITION AS varchar(12)) \
             FROM INFORMATION_SCHEMA.COLUMNS c \
             WHERE c.TABLE_SCHEMA = '{}' AND c.TABLE_NAME = '{}' \
             ORDER BY c.ORDINAL_POSITION",
            escape_literal(schema),
            escape_literal(table)
        );

        let rows = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_first_result()
            .await
            .map_err(map_err)?;

        Ok(rows
            .iter()
            .filter_map(|r| {
                let default: &str = r.get(3).unwrap_or("");
                Some(ColumnDef {
                    name: r.get::<&str, _>(0)?.to_string(),
                    type_name: r.get::<&str, _>(1)?.to_string(),
                    nullable: r.get::<&str, _>(2)? == "YES",
                    default: (!default.is_empty()).then(|| default.to_string()),
                    auto_increment: r.get::<&str, _>(4).unwrap_or("0") == "1",
                    ordinal: r
                        .get::<&str, _>(5)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    comment: None,
                })
            })
            .collect())
    }

    async fn indexes(&mut self, schema: &str, table: &str) -> Result<Vec<IndexDef>> {
        let sql = format!(
            "SELECT i.name, \
                    CAST(i.is_unique AS varchar(4)), \
                    CAST(i.is_primary_key AS varchar(4)), \
                    i.type_desc, \
                    c.name \
             FROM sys.indexes i \
             JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
             JOIN sys.columns c ON c.object_id = i.object_id AND c.column_id = ic.column_id \
             JOIN sys.tables t ON t.object_id = i.object_id \
             JOIN sys.schemas s ON s.schema_id = t.schema_id \
             WHERE s.name = '{}' AND t.name = '{}' AND i.name IS NOT NULL \
             ORDER BY i.name, ic.key_ordinal",
            escape_literal(schema),
            escape_literal(table)
        );

        let rows = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_first_result()
            .await
            .map_err(map_err)?;

        let mut grouped: Vec<IndexDef> = Vec::new();
        for r in rows.iter() {
            let Some(name) = r.get::<&str, _>(0) else {
                continue;
            };
            let Some(column) = r.get::<&str, _>(4) else {
                continue;
            };
            match grouped.last_mut() {
                Some(last) if last.name == name => last.columns.push(column.to_string()),
                _ => grouped.push(IndexDef {
                    name: name.to_string(),
                    columns: vec![column.to_string()],
                    unique: r.get::<&str, _>(1) == Some("1"),
                    primary: r.get::<&str, _>(2) == Some("1"),
                    method: r.get::<&str, _>(3).map(str::to_string),
                }),
            }
        }
        Ok(grouped)
    }

    async fn foreign_keys(&mut self, schema: &str, table: &str) -> Result<Vec<ForeignKeyDef>> {
        let sql = format!(
            "SELECT fk.name, pc.name, rs.name, rt.name, rc.name, \
                    fk.delete_referential_action_desc, fk.update_referential_action_desc \
             FROM sys.foreign_keys fk \
             JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN sys.tables pt ON pt.object_id = fk.parent_object_id \
             JOIN sys.schemas ps ON ps.schema_id = pt.schema_id \
             JOIN sys.columns pc ON pc.object_id = pt.object_id \
                                 AND pc.column_id = fkc.parent_column_id \
             JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id \
             JOIN sys.schemas rs ON rs.schema_id = rt.schema_id \
             JOIN sys.columns rc ON rc.object_id = rt.object_id \
                                 AND rc.column_id = fkc.referenced_column_id \
             WHERE ps.name = '{}' AND pt.name = '{}' \
             ORDER BY fk.name, fkc.constraint_column_id",
            escape_literal(schema),
            escape_literal(table)
        );

        let rows = self
            .client
            .simple_query(sql)
            .await
            .map_err(map_err)?
            .into_first_result()
            .await
            .map_err(map_err)?;

        let mut grouped: Vec<ForeignKeyDef> = Vec::new();
        for r in rows.iter() {
            let Some(name) = r.get::<&str, _>(0) else {
                continue;
            };
            let column = r.get::<&str, _>(1).unwrap_or_default().to_string();
            let ref_column = r.get::<&str, _>(4).unwrap_or_default().to_string();
            match grouped.last_mut() {
                Some(last) if last.name == name => {
                    last.columns.push(column);
                    last.referenced_columns.push(ref_column);
                }
                _ => grouped.push(ForeignKeyDef {
                    name: name.to_string(),
                    columns: vec![column],
                    referenced_schema: r.get::<&str, _>(2).map(str::to_string),
                    referenced_table: r.get::<&str, _>(3).unwrap_or_default().to_string(),
                    referenced_columns: vec![ref_column],
                    // SQL Server spells these NO_ACTION / SET_NULL; normalize to
                    // the spacing every other driver reports.
                    on_delete: r.get::<&str, _>(5).map(|s| s.replace('_', " ")),
                    on_update: r.get::<&str, _>(6).map(|s| s.replace('_', " ")),
                }),
            }
        }
        Ok(grouped)
    }
}

/// Escape a string for a T-SQL literal by doubling single quotes.
///
/// Used only for catalog names that come from the catalog itself or from a
/// schema-tree node, never from free-typed user input, but doubling costs
/// nothing and keeps a name containing a quote from breaking the query.
pub(crate) fn escape_literal(s: &str) -> String {
    s.replace('\'', "''")
}

/// Normalize driver errors into the shared taxonomy.
pub(crate) fn map_err(e: tiberius::error::Error) -> Error {
    use tiberius::error::Error as E;
    match &e {
        E::Server(token) => {
            // 18456 login failed; 4060 cannot open database; 916 no access.
            if matches!(token.code(), 18456 | 4060 | 916) {
                return Error::Auth(token.message().to_string());
            }
            Error::Query {
                message: token.message().to_string(),
                // TDS reports a line number rather than a character offset, so
                // the editor cannot underline the exact token the way it can
                // for PostgreSQL.
                position: None,
                code: Some(token.code().to_string()),
            }
        }
        E::Io { .. } => Error::Network(e.to_string()),
        E::Tls(msg) => Error::Tls(msg.clone()),
        _ => Error::query(e.to_string()),
    }
}

const MSSQL_FUNCTIONS: &[&str] = &[
    "abs",
    "avg",
    "cast",
    "ceiling",
    "charindex",
    "coalesce",
    "concat",
    "convert",
    "count",
    "dateadd",
    "datediff",
    "datepart",
    "getdate",
    "getutcdate",
    "isnull",
    "iif",
    "json_value",
    "left",
    "len",
    "lower",
    "ltrim",
    "max",
    "min",
    "newid",
    "nullif",
    "object_id",
    "replace",
    "right",
    "round",
    "row_number",
    "rtrim",
    "string_agg",
    "substring",
    "sum",
    "try_cast",
    "try_convert",
    "upper",
];

#[cfg(test)]
mod tests;
