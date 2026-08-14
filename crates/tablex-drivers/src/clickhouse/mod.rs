//! ClickHouse driver, over the HTTP interface.
//!
//! # Why HTTP rather than the native protocol
//!
//! The `clickhouse` crate is row-oriented around `serde` derives, which need the
//! row shape at compile time — exactly what a client running arbitrary user SQL
//! does not have. The HTTP endpoint's `JSONCompact` format returns column names
//! and types alongside the data, which is the dynamic shape this client needs.
//!
//! # Why results are read-only
//!
//! Two independent reasons, either of which alone would be enough:
//!
//! - `JSONCompact` metadata carries a column's name and type but not its source
//!   table, so a result cannot be traced back to one.
//! - ClickHouse has no row-level `UPDATE`. `ALTER TABLE … UPDATE` schedules an
//!   asynchronous *mutation* that rewrites whole data parts and completes at
//!   some later point. Presenting that as an inline cell edit would imply a
//!   guarantee — this row, now, exactly once — that the engine does not make.

mod types;

use async_trait::async_trait;
use tablex_core::{
    config::{ConnectionConfig, TlsMode},
    driver::{
        Capabilities, CompletionScope, Connection, Driver, DriverInfo, FetchOptions,
        PlaceholderStyle, RowEdit,
    },
    error::{Error, Result},
    result::{Column, QueryOutcome, ResultSet, StatementResult},
    schema::{decode_path, ColumnDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
};

const QUOTE: char = '`';

/// One folder in the object tree.
struct Folder {
    id: &'static str,
    label: &'static str,
    kind: NodeKind,
}

/// ClickHouse has no triggers and no stored procedures, so neither is offered.
/// Functions are server-wide rather than per-database, but they are listed under
/// each database anyway: the alternative is a level of the tree that exists for
/// one folder.
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
];

/// `` `db`.`name` ``.
fn qualify(db: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(db, QUOTE), quote_ident(name, QUOTE))
}

/// Databases that ship with every server and are noise for the user.
const HIDDEN_DATABASES: &str = "'system', 'INFORMATION_SCHEMA', 'information_schema'";

pub struct ClickhouseDriver;

impl ClickhouseDriver {
    pub fn new() -> Self {
        ClickhouseDriver
    }
}

impl Default for ClickhouseDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for ClickhouseDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "clickhouse".into(),
            name: "ClickHouse".into(),
            // The HTTP interface, not the native protocol on 9000.
            default_port: Some(8123),
            file_based: false,
            capabilities: Capabilities {
                // ClickHouse transactions are experimental and limited to a
                // single partition, so they are not advertised.
                transactions: false,
                explain: true,
                // ClickHouse's database is its only container level.
                schemas: false,
                databases: true,
                views: true,
                // SHOW CREATE TABLE answers for tables and views alike.
                table_scripts: true,
                // No foreign keys, and no row-level UPDATE — see the module docs.
                foreign_keys: false,
                column_provenance: false,
                stored_procedures: false,
                // Each statement is its own HTTP request; the driver splits them.
                multi_statement: true,
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
        let scheme = match config.tls.mode {
            TlsMode::Disable => "http",
            // ClickHouse serves HTTPS on a different port (8443 by default), so
            // enabling TLS without changing the port is a common mistake; the
            // error from a TLS handshake against 8123 is at least explicit.
            TlsMode::Prefer | TlsMode::VerifyFull => "https",
        };
        let host = config.host.clone().unwrap_or_else(|| "localhost".into());
        let port = config.port.unwrap_or(8123);
        let base = format!("{scheme}://{host}:{port}/");

        ensure_crypto_provider();

        let mut builder = reqwest::Client::builder()
            // Long analytical queries are normal here, so the ceiling is high;
            // FetchOptions::timeout_secs is what actually bounds a query.
            .timeout(std::time::Duration::from_secs(600))
            .gzip(true);

        if matches!(config.tls.mode, TlsMode::Prefer) {
            // "Prefer" means encrypt without asserting the chain, matching how
            // the other drivers read that setting.
            builder = builder.danger_accept_invalid_certs(true);
        }

        let client = builder
            .build()
            .map_err(|e| Error::Connection(format!("could not build an HTTP client: {e}")))?;

        let mut conn = ClickhouseConnection {
            client,
            base,
            user: config.username.clone().unwrap_or_else(|| "default".into()),
            password: secret.unwrap_or_default().to_string(),
            database: config
                .database
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        };

        // Fail at connect time rather than on the user's first query.
        conn.ping().await?;
        Ok(Box::new(conn))
    }
}

pub struct ClickhouseConnection {
    client: reqwest::Client,
    base: String,
    user: String,
    password: String,
    database: String,
}

#[async_trait]
impl Connection for ClickhouseConnection {
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
            [db, folder, table] => Ok(self
                .columns(db, table)
                .await?
                .into_iter()
                .map(|c| {
                    SchemaNode::new(&[db, folder, table, &c.name], &c.name, NodeKind::Column)
                        .detail(c.type_name)
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    async fn current_database(&mut self) -> Result<Option<String>> {
        Ok(Some(self.database.clone()))
    }

    /// `SHOW CREATE`, which ClickHouse answers for both tables and functions.
    async fn definition(&mut self, node_id: &str) -> Result<String> {
        let path = decode_path(node_id);
        let [db, folder, name] = path.as_slice() else {
            return Err(Error::Unsupported(
                "only an object has a definition to show".into(),
            ));
        };

        // A user-defined function is server-wide, so it is named on its own; a
        // view or table is named with its database.
        let statement = match folder.as_str() {
            "functions" => format!("SHOW CREATE FUNCTION {}", quote_ident(name, QUOTE)),
            "views" | "tables" => format!("SHOW CREATE TABLE {}", qualify(db, name)),
            other => {
                return Err(Error::Unsupported(format!(
                    "no definition available for {other}"
                )))
            }
        };

        self.column_of(&statement)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Unsupported(format!("{name} reported no definition")))
    }

    /// The database is a request parameter here, not session state, so
    /// switching is a local change that the next request carries with it.
    async fn use_database(&mut self, database: &str) -> Result<()> {
        // Checked rather than trusted: a typo would otherwise fail later, on an
        // unrelated query, with an error that names the wrong thing.
        let found = self
            .column_of(&format!(
                "SELECT name FROM system.databases WHERE name = {}",
                types::literal_str(database)
            ))
            .await?;
        if found.is_empty() {
            return Err(Error::Config(format!("no database named {database}")));
        }
        self.database = database.to_string();
        Ok(())
    }

    async fn table_detail(&mut self, schema: Option<&str>, table: &str) -> Result<TableDetail> {
        let db = schema.unwrap_or(&self.database).to_string();
        let columns = self.columns(&db, table).await?;
        if columns.is_empty() {
            return Err(Error::Unsupported(format!(
                "table {db}.{table} not found or not visible"
            )));
        }

        // ClickHouse has no primary key in the relational sense. The sorting key
        // orders data within a part and is not unique, so it is reported as an
        // index rather than as a primary key — treating it as one would let the
        // grid believe a row is uniquely addressable when it is not.
        let sorting_key = self
            .rows_of(&format!(
                "SELECT sorting_key, primary_key FROM system.tables \
                 WHERE database = {} AND name = {}",
                types::literal_str(&db),
                types::literal_str(table)
            ))
            .await?;

        let mut indexes = Vec::new();
        if let Some(row) = sorting_key.first() {
            if let Some(key) = row.first().filter(|k| !k.is_empty()) {
                indexes.push(IndexDef {
                    name: "sorting key".into(),
                    columns: key.split(',').map(|s| s.trim().to_string()).collect(),
                    // Explicitly not unique: ClickHouse does not enforce it.
                    unique: false,
                    primary: false,
                    method: Some("ORDER BY".into()),
                });
            }
        }

        let total = self
            .column_of(&format!(
                "SELECT toString(total_rows) FROM system.tables \
                 WHERE database = {} AND name = {}",
                types::literal_str(&db),
                types::literal_str(table)
            ))
            .await?;

        Ok(TableDetail {
            schema: Some(db),
            name: table.to_string(),
            columns,
            indexes,
            foreign_keys: Vec::new(),
            primary_key: Vec::new(),
            estimated_rows: total.first().and_then(|s| s.parse().ok()),
            comment: None,
        })
    }

    async fn apply_edit(&mut self, _edit: &RowEdit) -> Result<()> {
        // Refused rather than translated into `ALTER TABLE … UPDATE`. That
        // statement schedules an asynchronous mutation over whole data parts; it
        // returns before the change is visible and cannot report that exactly one
        // row was affected. Presenting it as a cell edit would promise semantics
        // the engine does not provide.
        Err(Error::Unsupported(
            "ClickHouse has no row-level UPDATE. Changing data means an ALTER TABLE … UPDATE \
             mutation, which is asynchronous and rewrites whole parts — run it explicitly \
             rather than as a cell edit."
                .into(),
        ))
    }

    async fn ping(&mut self) -> Result<()> {
        self.query_raw("SELECT 1", None).await.map(|_| ())
    }

    async fn close(&mut self) -> Result<()> {
        // HTTP is stateless; there is no session to end.
        Ok(())
    }

    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        let schemas = self
            .column_of(&format!(
                "SELECT name FROM system.databases \
                 WHERE name NOT IN ({HIDDEN_DATABASES}) ORDER BY name"
            ))
            .await?;

        // The current database only, for the same reason as the other drivers:
        // every column on the server is a great deal of data to fetch for a
        // dropdown, and unqualified names resolve here anyway.
        let rows = self
            .rows_of(&format!(
                "SELECT table, name FROM system.columns \
                 WHERE database = {} ORDER BY table, position",
                types::literal_str(&self.database)
            ))
            .await?;

        let mut tables: Vec<(String, Vec<String>)> = Vec::new();
        for r in rows {
            let (Some(table), Some(column)) = (r.first(), r.get(1)) else {
                continue;
            };
            match tables.last_mut() {
                Some((name, cols)) if name == table => cols.push(column.clone()),
                _ => tables.push((table.clone(), vec![column.clone()])),
            }
        }

        let functions = self
            .column_of("SELECT name FROM system.functions ORDER BY name LIMIT 2000")
            .await
            .unwrap_or_default();

        Ok(CompletionScope {
            schemas,
            tables,
            functions,
            keywords: Vec::new(),
        })
    }
}

impl ClickhouseConnection {
    async fn run_one(&mut self, sql: &str, opts: &FetchOptions) -> Result<StatementResult> {
        let (body, summary) = self.query_raw(sql, opts.timeout_secs).await?;

        // A statement that returns rows produces a JSONCompact document; DDL and
        // INSERT produce an empty body. Checking the body avoids guessing from
        // the statement text.
        if body.trim().is_empty() {
            return Ok(StatementResult::Affected {
                rows_affected: summary.written_rows,
                last_insert_id: None,
            });
        }

        let doc: types::JsonCompact = serde_json::from_str(&body)
            .map_err(|e| Error::query(format!("could not parse the ClickHouse response: {e}")))?;

        let columns: Vec<Column> = doc
            .meta
            .iter()
            .map(|m| Column {
                name: m.name.clone(),
                type_name: m.type_name.clone(),
                // ClickHouse spells nullability in the type: `Nullable(String)`.
                nullable: Some(m.type_name.starts_with("Nullable(")),
                // See the module docs — JSONCompact carries no source table.
                source: None,
            })
            .collect();

        let cap = opts.max_rows.unwrap_or(usize::MAX);
        let truncated = doc.data.len().saturating_sub(opts.offset) > cap;

        let rows = doc
            .data
            .iter()
            .skip(opts.offset)
            .take(cap)
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        let ty = doc.meta.get(i).map(|m| m.type_name.as_str()).unwrap_or("");
                        types::decode(cell, ty)
                    })
                    .collect()
            })
            .collect();

        let mut rs = ResultSet {
            columns,
            rows,
            truncated,
            editable: false,
            key_columns: Vec::new(),
        };
        rs.recompute_editable();
        Ok(StatementResult::Rows(rs))
    }

    /// POST a statement and return its body plus the server's summary.
    async fn query_raw(
        &self,
        sql: &str,
        timeout_secs: Option<u64>,
    ) -> Result<(String, types::Summary)> {
        let mut url = reqwest::Url::parse(&self.base)
            .map_err(|e| Error::Config(format!("invalid ClickHouse URL {}: {e}", self.base)))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("database", &self.database);
            // Set as a parameter rather than appended to the SQL, so DDL and
            // INSERT — which reject a FORMAT clause — still work.
            q.append_pair("default_format", "JSONCompact");
            // A JSON number is an IEEE double, so anything past 2^53 would lose
            // its low bits in transit. Asking the server to quote wide integers
            // is what keeps them exact.
            q.append_pair("output_format_json_quote_64bit_integers", "1");
            q.append_pair("output_format_json_quote_denormals", "1");
            if let Some(secs) = timeout_secs {
                q.append_pair("max_execution_time", &secs.to_string());
            }
        }

        let request = self
            .client
            .post(url)
            .header("X-ClickHouse-User", &self.user)
            .header("X-ClickHouse-Key", &self.password)
            .body(sql.to_string());

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                Error::Timeout(timeout_secs.unwrap_or(0))
            } else if e.is_connect() {
                Error::Connection(format!("could not reach {}: {e}", self.base))
            } else {
                Error::Network(e.to_string())
            }
        })?;

        let status = response.status();
        let summary = types::Summary::from_headers(response.headers());
        let body = response
            .text()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;

        if !status.is_success() {
            return Err(map_http_error(status, &body));
        }
        Ok((body, summary))
    }

    /// Run a query and return its first column as strings.
    async fn column_of(&self, sql: &str) -> Result<Vec<String>> {
        Ok(self
            .rows_of(sql)
            .await?
            .into_iter()
            .filter_map(|mut r| {
                if r.is_empty() {
                    None
                } else {
                    Some(r.swap_remove(0))
                }
            })
            .collect())
    }

    /// Run a query and return its rows as strings, for catalog lookups.
    async fn rows_of(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        let (body, _) = self.query_raw(sql, Some(30)).await?;
        if body.trim().is_empty() {
            return Ok(Vec::new());
        }
        let doc: types::JsonCompact = serde_json::from_str(&body)
            .map_err(|e| Error::query(format!("could not parse the ClickHouse response: {e}")))?;

        Ok(doc
            .data
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|v| match v {
                        serde_json::Value::String(s) => s,
                        serde_json::Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect())
    }

    async fn browse_databases(&self) -> Result<Vec<SchemaNode>> {
        let names = self
            .column_of(&format!(
                "SELECT name FROM system.databases \
                 WHERE name NOT IN ({HIDDEN_DATABASES}) ORDER BY name"
            ))
            .await?;

        Ok(names
            .into_iter()
            .map(|name| {
                let node = SchemaNode::new(&[&name], &name, NodeKind::Database)
                    .expandable()
                    .qualified(quote_ident(&name, QUOTE));
                if name == self.database {
                    node.detail("in use")
                } else {
                    node
                }
            })
            .collect())
    }

    async fn browse_folder(&self, db: &str, spec: &Folder) -> Result<Vec<SchemaNode>> {
        if spec.id == "functions" {
            return self.browse_functions(db, spec).await;
        }

        // Engine names ending in "View" cover both `View` and
        // `MaterializedView`; everything else is a table of some engine.
        let comparison = if spec.id == "views" {
            "engine LIKE '%View'"
        } else {
            "engine NOT LIKE '%View'"
        };
        let rows = self
            .rows_of(&format!(
                "SELECT name, engine, toString(total_rows) FROM system.tables \
                 WHERE database = {} AND {comparison} ORDER BY name",
                types::literal_str(db)
            ))
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let name = r.first().cloned().unwrap_or_default();
                let engine = r.get(1).cloned().unwrap_or_default();
                let total = r.get(2).cloned().unwrap_or_default();
                let node = SchemaNode::new(&[db, spec.id, &name], &name, spec.kind.clone())
                    .expandable()
                    .qualified(qualify(db, &name));
                // total_rows is exact for MergeTree tables and null for engines
                // that cannot report it.
                if !total.is_empty() && total != "\\N" {
                    node.detail(format!("{total} rows · {engine}"))
                } else {
                    node.detail(engine)
                }
            })
            .collect())
    }

    async fn browse_functions(&self, db: &str, spec: &Folder) -> Result<Vec<SchemaNode>> {
        // Only user-defined ones: ClickHouse ships well over a thousand builtins,
        // and a folder of those is a reference manual, not a browsable list.
        let names = self
            .column_of("SELECT name FROM system.functions WHERE origin != 'System' ORDER BY name")
            .await?;

        Ok(names
            .into_iter()
            .map(|name| {
                SchemaNode::new(&[db, spec.id, &name], &name, spec.kind.clone())
                    .qualified(quote_ident(&name, QUOTE))
            })
            .collect())
    }

    async fn columns(&self, db: &str, table: &str) -> Result<Vec<ColumnDef>> {
        let rows = self
            .rows_of(&format!(
                "SELECT name, type, default_expression, toString(position), comment \
                 FROM system.columns \
                 WHERE database = {} AND table = {} ORDER BY position",
                types::literal_str(db),
                types::literal_str(table)
            ))
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let type_name = r.get(1).cloned().unwrap_or_default();
                let default = r.get(2).cloned().unwrap_or_default();
                let comment = r.get(4).cloned().unwrap_or_default();
                ColumnDef {
                    name: r.first().cloned().unwrap_or_default(),
                    // Nullability is part of the type name, not a separate flag.
                    nullable: type_name.starts_with("Nullable("),
                    type_name,
                    default: (!default.is_empty()).then_some(default),
                    auto_increment: false,
                    ordinal: r.get(3).and_then(|s| s.parse().ok()).unwrap_or(0),
                    comment: (!comment.is_empty()).then_some(comment),
                }
            })
            .collect())
    }
}

/// Install the ring crypto provider, once per process.
///
/// reqwest is built with `rustls-no-provider` to keep aws-lc-rs (and its NASM
/// build requirement) out of the tree, which means the provider has to be
/// installed explicitly — reqwest panics when building a client otherwise.
///
/// `install_default` returns an error if something already installed one; that
/// is a fine outcome, not a failure, so it is ignored.
pub(crate) fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Turn an HTTP failure into the shared taxonomy.
///
/// ClickHouse reports its own error code in the body as `Code: NNN.`, which is
/// more specific than the HTTP status alone.
fn map_http_error(status: reqwest::StatusCode, body: &str) -> Error {
    let code = body
        .strip_prefix("Code: ")
        .and_then(|rest| rest.split('.').next())
        .and_then(|n| n.trim().parse::<u32>().ok());

    // 516 AUTHENTICATION_FAILED, 497 ACCESS_DENIED, 192 UNKNOWN_USER.
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || matches!(code, Some(516 | 497 | 192))
    {
        return Error::Auth(body.trim().to_string());
    }
    // 81 UNKNOWN_DATABASE.
    if matches!(code, Some(81)) {
        return Error::Connection(body.trim().to_string());
    }
    if status.is_server_error() && code.is_none() {
        return Error::Network(format!("{status}: {}", body.trim()));
    }

    Error::Query {
        message: body.trim().to_string(),
        // ClickHouse reports a byte position inside the message text rather than
        // as a field, so the editor cannot underline the token directly.
        position: None,
        code: code.map(|c| c.to_string()),
    }
}

#[cfg(test)]
mod tests;
