//! PostgreSQL driver.
//!
//! Unlike SQLite, PostgreSQL reports the source table and column for every result
//! column, so ad-hoc query results can be inline-edited safely. That capability
//! difference is exactly what [`Capabilities`] exists to express.

mod introspect;
mod numeric;
mod types;

use async_trait::async_trait;
use std::collections::HashMap;

use tablex_core::{
    config::{ConnectionConfig, TlsMode},
    driver::{
        Capabilities, CompletionScope, Connection, Driver, DriverInfo, FetchOptions,
        PlaceholderStyle, RowEdit,
    },
    error::{Error, Result},
    result::{Column, ColumnSource, QueryOutcome, ResultSet, StatementResult},
    schema::{SchemaNode, TableDetail},
    sql::{quote_ident, split_statements},
};
use tokio_postgres::{Client, NoTls};

const QUOTE: char = '"';

pub struct PostgresDriver;

impl PostgresDriver {
    pub fn new() -> Self {
        PostgresDriver
    }
}

impl Default for PostgresDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for PostgresDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "postgres".into(),
            name: "PostgreSQL".into(),
            default_port: Some(5432),
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
                // The wire protocol reports table OID and attribute number per
                // column, so results can be traced back to their source table.
                column_provenance: true,
                // Cancellation needs a cancel token held alongside the session;
                // that belongs with the connection registry, so it is not
                // advertised until it is actually wired up.
                cancel: false,
                streaming: false,
                placeholder_style: PlaceholderStyle::Dollar,
                identifier_quote: QUOTE,
            },
        }
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        secret: Option<&str>,
    ) -> Result<Box<dyn Connection>> {
        let mut pg = tokio_postgres::Config::new();
        pg.host(config.host.as_deref().unwrap_or("localhost"));
        pg.port(config.port.unwrap_or(5432));
        if let Some(db) = &config.database {
            pg.dbname(db);
        }
        if let Some(user) = &config.username {
            pg.user(user);
        }
        if let Some(password) = secret {
            pg.password(password);
        }
        pg.application_name("Table X");

        let client = establish(&pg, config.tls.mode).await?;

        // Asked rather than taken from the config: with no `dbname` the server
        // connects you to one named after the user, and the tree has to mark the
        // database you are actually in, not the one you did not name.
        let database: String = client
            .query_one("SELECT current_database()", &[])
            .await
            .map_err(map_err)?
            .get(0);

        Ok(Box::new(PostgresConnection {
            client,
            database,
            type_cache: HashMap::new(),
        }))
    }
}

/// Open a session honouring the configured TLS mode.
async fn establish(pg: &tokio_postgres::Config, mode: TlsMode) -> Result<Client> {
    match mode {
        TlsMode::Disable => connect_plain(pg).await,

        TlsMode::VerifyFull => connect_tls(pg).await,

        // "Prefer" attempts a verified TLS connection and falls back to plaintext
        // if the server does not offer usable TLS. Note this is stricter than
        // libpq's `prefer`, which encrypts without verifying: rather than ship a
        // code path that accepts any certificate, a server with an untrusted
        // certificate falls back to an *obviously* unencrypted connection instead
        // of an encrypted one that provides no real guarantee.
        TlsMode::Prefer => match connect_tls(pg).await {
            Ok(client) => Ok(client),
            Err(e) => {
                tracing::warn!("TLS unavailable ({e}); falling back to an unencrypted connection");
                connect_plain(pg).await
            }
        },
    }
}

async fn connect_plain(pg: &tokio_postgres::Config) -> Result<Client> {
    let (client, connection) = pg.connect(NoTls).await.map_err(map_err)?;
    spawn_connection(connection);
    Ok(client)
}

async fn connect_tls(pg: &tokio_postgres::Config) -> Result<Client> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        // Individual malformed certificates in the system store are skipped
        // rather than failing the whole connection.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(Error::Tls("no trusted root certificates found".into()));
    }

    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);

    let (client, connection) = pg.connect(tls).await.map_err(map_err)?;
    spawn_connection(connection);
    Ok(client)
}

/// tokio-postgres splits the client from the I/O driver; the driver must be
/// polled for the client to work at all.
fn spawn_connection<S, T>(connection: tokio_postgres::Connection<S, T>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::warn!("postgres connection closed: {e}");
        }
    });
}

pub struct PostgresConnection {
    client: Client,
    /// The database this session is attached to. Fixed for its lifetime — the
    /// protocol offers no way to change it — so switching means reconnecting.
    database: String,
    /// OID → (schema, table) for result-column provenance. Table OIDs are stable
    /// for the life of a table, so this is cached rather than re-queried per row.
    type_cache: HashMap<u32, (String, String)>,
}

#[async_trait]
impl Connection for PostgresConnection {
    async fn execute(&mut self, sql: &str, opts: &FetchOptions) -> Result<QueryOutcome> {
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Err(Error::query("no statement to execute"));
        }

        let started = std::time::Instant::now();
        let mut out = Vec::with_capacity(statements.len());
        for stmt_sql in &statements {
            out.push(self.run_one(stmt_sql, opts).await?);
        }

        Ok(QueryOutcome {
            statements: out,
            elapsed_ms: started.elapsed().as_millis() as u64,
            notices: Vec::new(),
        })
    }

    async fn browse(&mut self, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
        introspect::browse(&self.client, &self.database, parent).await
    }

    async fn current_database(&mut self) -> Result<Option<String>> {
        Ok(Some(self.database.clone()))
    }

    async fn definition(&mut self, node_id: &str) -> Result<String> {
        introspect::definition(&self.client, node_id).await
    }

    // `use_database` is deliberately left at its default error. A PostgreSQL
    // connection is bound to one database by the startup packet and there is no
    // in-session equivalent of `USE`; the app layer reconnects instead.

    async fn table_detail(&mut self, schema: Option<&str>, table: &str) -> Result<TableDetail> {
        introspect::table_detail(&self.client, schema.unwrap_or("public"), table).await
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

        let schema = edit.schema.as_deref().unwrap_or("public");
        let col_types = introspect::column_types(&self.client, schema, &edit.table).await?;

        // Parameters are always sent as text and cast to the column's own type by
        // the server: `$1::text::int4`. The inner cast pins the parameter to text
        // so a single binding path works for every column type, and the outer cast
        // lets PostgreSQL do the conversion it already knows how to do. Critically,
        // exact numerics travel as their original digits rather than through any
        // fixed-width intermediate.
        let mut params: Vec<Option<String>> = Vec::new();
        let mut n = 0usize;

        let assignments = edit
            .changes
            .iter()
            .map(|(col, val)| {
                n += 1;
                params.push(types::to_param(val));
                let ty = col_types.get(col).cloned().unwrap_or_else(|| "text".into());
                format!(
                    "{} = ${}::text::{}",
                    quote_ident(col, QUOTE),
                    n,
                    quote_ident(&ty, QUOTE)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");

        let predicate = edit
            .key
            .iter()
            .map(|(col, val)| {
                // NULL never equals NULL; a NULL key needs IS NULL.
                if val.is_null() {
                    return format!("{} IS NULL", quote_ident(col, QUOTE));
                }
                n += 1;
                params.push(types::to_param(val));
                let ty = col_types.get(col).cloned().unwrap_or_else(|| "text".into());
                format!(
                    "{} = ${}::text::{}",
                    quote_ident(col, QUOTE),
                    n,
                    quote_ident(&ty, QUOTE)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");

        let sql = format!(
            "UPDATE {}.{} SET {} WHERE {}",
            quote_ident(schema, QUOTE),
            quote_ident(&edit.table, QUOTE),
            assignments,
            predicate
        );

        let bindings: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();

        let tx = self.client.transaction().await.map_err(map_err)?;
        let affected = tx.execute(sql.as_str(), &bindings).await.map_err(map_err)?;

        // The guard that makes inline editing safe: anything other than exactly
        // one row means the key was not unique or the row changed underneath us.
        if affected != 1 {
            tx.rollback().await.map_err(map_err)?;
            return Err(Error::Query {
                message: format!(
                    "edit matched {affected} rows, expected exactly 1 — \
                     the row may have changed since it was loaded"
                ),
                position: None,
                code: None,
            });
        }
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn ping(&mut self) -> Result<()> {
        self.client
            .simple_query("SELECT 1")
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    async fn close(&mut self) -> Result<()> {
        // Dropping the client closes the socket and ends the spawned driver task.
        Ok(())
    }

    async fn completion_scope(&mut self) -> Result<CompletionScope> {
        introspect::completion_scope(&self.client).await
    }
}

impl PostgresConnection {
    async fn run_one(&mut self, sql: &str, opts: &FetchOptions) -> Result<StatementResult> {
        // Preparing first gives column metadata (including provenance) and tells
        // us whether the statement returns rows at all. Sniffing the leading
        // keyword would misjudge CTEs and `INSERT ... RETURNING`.
        let stmt = self.client.prepare(sql).await.map_err(map_err)?;

        if stmt.columns().is_empty() {
            let affected = self.client.execute(&stmt, &[]).await.map_err(map_err)?;
            return Ok(StatementResult::Affected {
                rows_affected: affected,
                last_insert_id: None,
            });
        }

        // Resolve table OIDs to names once per statement, not once per column.
        let oids: Vec<u32> = stmt
            .columns()
            .iter()
            .filter_map(|c| c.table_oid())
            .filter(|oid| !self.type_cache.contains_key(oid))
            .collect();
        if !oids.is_empty() {
            let resolved = introspect::resolve_table_oids(&self.client, &oids).await?;
            self.type_cache.extend(resolved);
        }

        let columns: Vec<Column> = stmt
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().to_string(),
                type_name: c.type_().name().to_string(),
                nullable: None,
                source: c.table_oid().and_then(|oid| {
                    self.type_cache
                        .get(&oid)
                        .map(|(schema, table)| ColumnSource {
                            schema: Some(schema.clone()),
                            table: table.clone(),
                            // `name()` is the output label, which respects aliases;
                            // for editing we want the underlying column, and for a
                            // plain projection they are the same.
                            column: c.name().to_string(),
                        })
                }),
            })
            .collect();

        let rows = self.client.query(&stmt, &[]).await.map_err(map_err)?;

        let cap = opts.max_rows.unwrap_or(usize::MAX);
        let truncated = rows.len().saturating_sub(opts.offset) > cap;

        let decoded: Vec<Vec<tablex_core::Value>> = rows
            .iter()
            .skip(opts.offset)
            .take(cap)
            .map(|row| {
                stmt.columns()
                    .iter()
                    .enumerate()
                    .map(|(i, col)| {
                        let raw: types::Raw = row.get(i);
                        types::decode(raw.0.as_deref(), col.type_())
                    })
                    .collect()
            })
            .collect();

        // Determine an edit key: only when every column came from one table and
        // that table's primary key is fully present in the projection.
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

    /// The primary key of the single source table, if the projection contains all
    /// of it. Returns empty when the result spans tables or omits a key column,
    /// which forces the grid read-only.
    async fn edit_key_for(&self, columns: &[Column]) -> Vec<String> {
        let mut sources = columns.iter().filter_map(|c| c.source.as_ref());
        let Some(first) = sources.next() else {
            return Vec::new();
        };
        if !sources.all(|s| s.table == first.table && s.schema == first.schema) {
            return Vec::new();
        }

        let schema = first.schema.as_deref().unwrap_or("public");
        let Ok(pk) = introspect::primary_key(&self.client, schema, &first.table).await else {
            return Vec::new();
        };
        // A key the user cannot see is a key we cannot send back in a WHERE clause.
        if !pk.is_empty() && pk.iter().all(|k| columns.iter().any(|c| &c.name == k)) {
            pk
        } else {
            Vec::new()
        }
    }
}

/// Translate driver errors into the shared taxonomy, preserving the SQLSTATE and
/// the character position so the editor can underline the offending token.
pub(crate) fn map_err(e: tokio_postgres::Error) -> Error {
    if let Some(db) = e.as_db_error() {
        use tokio_postgres::error::SqlState;
        let code = db.code();
        let message = db.message().to_string();

        // Class 28 is invalid authorization; 3D/3F are missing database/schema.
        if *code == SqlState::INVALID_PASSWORD
            || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
        {
            return Error::Auth(message);
        }
        if *code == SqlState::UNDEFINED_DATABASE {
            return Error::Connection(message);
        }

        return Error::Query {
            message,
            // `Internal` positions point into a generated statement body (a
            // function or DO block); both are still the best cursor target we have.
            position: db.position().map(|p| match p {
                tokio_postgres::error::ErrorPosition::Original(n)
                | tokio_postgres::error::ErrorPosition::Internal { position: n, .. } => *n,
            }),
            code: Some(code.code().to_string()),
        };
    }

    if e.is_closed() {
        return Error::Connection(e.to_string());
    }
    Error::Network(e.to_string())
}

/// Table OID → `(schema, table)`, used to attach provenance to result columns.
pub(crate) type OidMap = HashMap<u32, (String, String)>;

#[cfg(test)]
mod tests;
