//! Tauri IPC surface.
//!
//! Commands are thin: validate, delegate to core, map errors to a serializable
//! payload. No database logic lives here.
//!
//! Every fallible command returns [`ErrorPayload`] so the frontend has exactly one
//! error shape to handle, carrying a category, a retryable flag, and — for query
//! errors — the SQLSTATE and character offset the editor needs.

use serde::{Deserialize, Serialize};
use tablex_core::{
    driver::{DriverInfo, FetchOptions, RowEdit},
    result::{QueryOutcome, StatementResult},
    schema::{SchemaNode, TableDetail},
    ConnectionConfig, ErrorPayload,
};

use crate::{
    history::{self, HistoryEntry, HistoryQuery},
    secrets,
    state::AppState,
};

pub type IpcResult<T> = std::result::Result<T, ErrorPayload>;

#[derive(Serialize)]
pub struct BackendInfo {
    pub version: String,
    pub drivers: Vec<String>,
}

/// Handshake used by the frontend on boot.
#[tauri::command(rename_all = "snake_case")]
pub fn backend_info(state: tauri::State<'_, AppState>) -> BackendInfo {
    BackendInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        drivers: state.drivers.list().into_iter().map(|d| d.name).collect(),
    }
}

/// Full driver descriptors, used to render the connection form for a chosen driver.
#[tauri::command(rename_all = "snake_case")]
pub fn list_drivers(state: tauri::State<'_, AppState>) -> Vec<DriverInfo> {
    state.drivers.list()
}

// ---------------------------------------------------------------------------
// Saved connections
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn list_connections(
    state: tauri::State<'_, AppState>,
) -> IpcResult<Vec<ConnectionConfig>> {
    Ok(state.connections.lock().await.clone())
}

/// Ids of connections with a live session, so the UI can show which are open.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_connections(state: tauri::State<'_, AppState>) -> IpcResult<Vec<String>> {
    Ok(state.sessions.open_ids().await)
}

/// Create or update a saved connection.
///
/// Secrets are passed separately and never travel inside the config, so they
/// cannot end up in the JSON file by accident. Passing `None` leaves any existing
/// keychain entry untouched, which is what lets the UI save an edited connection
/// without re-prompting for a password it never displayed; passing `Some("")`
/// explicitly clears it.
///
/// The database credential and the SSH credential are stored under separate
/// keychain entries, so saving one never overwrites the other.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_connection(
    state: tauri::State<'_, AppState>,
    config: ConnectionConfig,
    secret: Option<String>,
    ssh_secret: Option<String>,
) -> IpcResult<()> {
    if config.id.trim().is_empty() {
        return Err(tablex_core::Error::Config("connection id is required".into()).into());
    }
    if !state.drivers.contains(&config.driver) {
        return Err(tablex_core::Error::UnknownDriver(config.driver.clone()).into());
    }

    store_secret(&config.keychain_key(), secret)?;
    store_secret(&config.ssh_keychain_key(), ssh_secret)?;

    let mut connections = state.connections.lock().await;
    match connections.iter_mut().find(|c| c.id == config.id) {
        Some(existing) => *existing = config,
        None => connections.push(config),
    }
    state.store.save(&connections)?;
    Ok(())
}

/// What a submitted secret field means for the stored credential.
///
/// The three-way distinction is what stops an edit dialog from destroying a
/// password it never displayed: the field starts empty either way, so "empty"
/// alone cannot tell us whether the user wants it cleared or left alone.
#[derive(Debug, PartialEq, Eq)]
enum SecretAction {
    /// The field was not touched — keep whatever is in the keychain.
    Leave,
    /// The field was explicitly emptied — remove the stored credential.
    Clear,
    /// The field holds a new value.
    Replace,
}

fn secret_action(value: Option<&str>) -> SecretAction {
    match value {
        None => SecretAction::Leave,
        Some("") => SecretAction::Clear,
        Some(_) => SecretAction::Replace,
    }
}

/// Apply a secret update to one keychain entry.
fn store_secret(key: &str, value: Option<String>) -> IpcResult<()> {
    match secret_action(value.as_deref()) {
        SecretAction::Leave => Ok(()),
        SecretAction::Clear => Ok(secrets::delete(key)?),
        SecretAction::Replace => Ok(secrets::set(key, value.as_deref().unwrap_or_default())?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_field_leaves_the_stored_secret_alone() {
        // The edit dialog never receives the stored password, so its field is
        // always empty on open. Treating that as "clear it" would silently
        // destroy the credential every time a user renamed a connection.
        assert_eq!(secret_action(None), SecretAction::Leave);
    }

    #[test]
    fn an_explicitly_emptied_field_clears_the_stored_secret() {
        assert_eq!(secret_action(Some("")), SecretAction::Clear);
    }

    #[test]
    fn a_filled_field_replaces_the_stored_secret() {
        assert_eq!(secret_action(Some("hunter2")), SecretAction::Replace);
        // Whitespace is a legitimate secret, not an empty one.
        assert_eq!(secret_action(Some(" ")), SecretAction::Replace);
    }

    #[test]
    fn database_and_ssh_secrets_target_different_entries() {
        use indexmap::IndexMap;
        use tablex_core::config::TlsConfig;

        let config = ConnectionConfig {
            id: "abc".into(),
            name: "n".into(),
            driver: "postgres".into(),
            host: None,
            port: None,
            database: None,
            username: None,
            file_path: None,
            tls: TlsConfig::default(),
            ssh: None,
            folder: None,
            color: None,
            read_only: false,
            options: IndexMap::new(),
        };
        // Saving a database password must never overwrite a key passphrase.
        assert_ne!(config.keychain_key(), config.ssh_keychain_key());
    }
}

/// Delete a saved connection, its credentials, and any live session.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_connection(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    // Drop the live session first: leaving an open socket for a connection the
    // user just deleted would keep querying a database they can no longer see.
    state.sessions.remove(&id).await?;

    let mut connections = state.connections.lock().await;
    let Some(index) = connections.iter().position(|c| c.id == id) else {
        return Err(tablex_core::Error::UnknownConnection(id).into());
    };
    let removed = connections.remove(index);
    state.store.save(&connections)?;

    // Best effort: a stale keychain entry is untidy but not dangerous, and
    // failing here would leave the config and the keychain inconsistent.
    let _ = secrets::delete(&removed.keychain_key());
    let _ = secrets::delete(&removed.ssh_keychain_key());
    Ok(())
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Open a session for a saved connection, establishing an SSH tunnel first if
/// the connection is configured to use one.
#[tauri::command(rename_all = "snake_case")]
pub async fn connect(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    let config = state.config_for(&id).await?;
    let driver = state.drivers.get(&config.driver)?;
    let secret = secrets::get(&config.keychain_key())?;

    let (config, tunnel) = establish_tunnel(&config).await?;
    let connection = driver.connect(&config, secret.as_deref()).await?;
    state.sessions.insert(&id, connection, tunnel).await;
    Ok(())
}

/// Open the SSH tunnel, if configured, and rewrite the config to point at its
/// local end.
///
/// Returning the rewritten config rather than mutating in place keeps the saved
/// connection untouched: the loopback port is ephemeral and must never be
/// written back to disk.
async fn establish_tunnel(
    config: &ConnectionConfig,
) -> IpcResult<(ConnectionConfig, Option<tablex_tunnel::Tunnel>)> {
    establish_tunnel_with(config, None).await
}

/// As [`establish_tunnel`], but an explicit SSH secret takes priority over the
/// stored one — used by "Test connection", where the credential may have been
/// typed into the form and not saved yet.
async fn establish_tunnel_with(
    config: &ConnectionConfig,
    ssh_secret: Option<String>,
) -> IpcResult<(ConnectionConfig, Option<tablex_tunnel::Tunnel>)> {
    let Some(ssh) = &config.ssh else {
        return Ok((config.clone(), None));
    };

    let target_host = config.host.clone().unwrap_or_else(|| "localhost".into());
    let target_port = config.port.ok_or_else(|| {
        tablex_core::Error::Config("a tunnelled connection needs a target port".into())
    })?;

    // The SSH credential lives under its own keychain entry, so a key passphrase
    // and a database password never overwrite each other.
    let stored;
    let secret = match ssh_secret {
        Some(ref s) if !s.is_empty() => Some(s.as_str()),
        _ => {
            stored = secrets::get(&config.ssh_keychain_key())?;
            stored.as_deref()
        }
    };

    let tunnel = tablex_tunnel::open(ssh, &target_host, target_port, secret).await?;

    let mut tunnelled = config.clone();
    tunnelled.host = Some("127.0.0.1".into());
    tunnelled.port = Some(tunnel.local_port());
    Ok((tunnelled, Some(tunnel)))
}

/// Read the SSH server's host key fingerprint so the user can confirm it.
///
/// Connecting requires a stored fingerprint, so this is the first step when
/// setting up a tunnelled connection. Nothing is authenticated or forwarded.
#[tauri::command(rename_all = "snake_case")]
pub async fn ssh_host_fingerprint(ssh: tablex_core::config::SshConfig) -> IpcResult<String> {
    Ok(tablex_tunnel::probe_host_key(&ssh).await?)
}

/// Try a connection without saving a session — the "Test connection" button.
///
/// Takes the config directly rather than an id so it can validate a form the user
/// has not saved yet.
#[tauri::command(rename_all = "snake_case")]
pub async fn test_connection(
    state: tauri::State<'_, AppState>,
    config: ConnectionConfig,
    secret: Option<String>,
    ssh_secret: Option<String>,
) -> IpcResult<()> {
    let driver = state.drivers.get(&config.driver)?;

    // An explicit secret from the form wins; otherwise fall back to whatever is
    // already in the keychain, so testing an existing connection works without
    // retyping the password.
    let stored;
    let secret = match secret {
        Some(ref s) => Some(s.as_str()),
        None => {
            stored = secrets::get(&config.keychain_key())?;
            stored.as_deref()
        }
    };

    // Tunnel too, so "Test connection" exercises the same path a real connect
    // takes rather than reporting success on a route that will not be used —
    // including the SSH credential typed into the form but not yet saved.
    let (config, tunnel) = establish_tunnel_with(&config, ssh_secret).await?;

    let mut connection = driver.connect(&config, secret).await?;
    let result = connection.ping().await;
    // Close regardless of the ping result: a test must never leave a socket open.
    let _ = connection.close().await;
    drop(tunnel);
    result?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn disconnect(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    state.sessions.remove(&id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Queries and schema
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ExecuteRequest {
    pub connection_id: String,
    pub sql: String,
    #[serde(default)]
    pub max_rows: Option<usize>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn execute(
    state: tauri::State<'_, AppState>,
    request: ExecuteRequest,
) -> IpcResult<QueryOutcome> {
    let config = state.config_for(&request.connection_id).await?;

    // A read-only connection refuses writes here, independently of whatever
    // permissions the database itself grants. This is a guard against running the
    // wrong statement against production, not a security boundary.
    if config.read_only && tablex_core::sql::looks_like_write(&request.sql) {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let defaults = FetchOptions::default();
    let opts = FetchOptions {
        max_rows: request.max_rows.or(defaults.max_rows),
        offset: request.offset,
        timeout_secs: request.timeout_secs.or(defaults.timeout_secs),
    };

    let session = state.sessions.get(&request.connection_id).await?;
    let started = std::time::Instant::now();
    // Scoped so the connection lock is released before history is written: a
    // slow disk must never hold up the next query on the same session.
    let outcome = {
        let mut guard = session.connection.lock().await;
        guard.execute(&request.sql, &opts).await
    };

    // A paged fetch continues a query that is already in history; recording it
    // again would fill the panel with duplicates of whatever the user scrolled.
    if request.offset == 0 {
        record_history(&state, &config, &request.sql, started, &outcome).await;
    }

    Ok(outcome?)
}

/// Append one execution to the history file.
///
/// Failures are logged and swallowed: the user asked for a query, not for a log
/// line, and failing the command because history could not be written would turn
/// a full disk into "your database is broken".
async fn record_history(
    state: &AppState,
    config: &ConnectionConfig,
    sql: &str,
    started: std::time::Instant,
    outcome: &tablex_core::error::Result<QueryOutcome>,
) {
    if history::assigns_a_credential(sql) {
        return;
    }

    let entry = HistoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        connection_id: config.id.clone(),
        connection_name: config.name.clone(),
        driver: config.driver.clone(),
        sql: sql.to_string(),
        ran_at: chrono::Utc::now().to_rfc3339(),
        // On success prefer the driver's own measurement, which excludes the
        // time spent waiting for the session lock.
        elapsed_ms: match outcome {
            Ok(o) => o.elapsed_ms,
            Err(_) => started.elapsed().as_millis() as u64,
        },
        rows: outcome.as_ref().ok().map(row_count),
        succeeded: outcome.is_ok(),
        error: outcome.as_ref().err().map(|e| e.to_string()),
    };

    if let Err(e) = state.history.lock().await.record(entry) {
        tracing::warn!("could not record query history: {e}");
    }
}

/// Rows returned or affected across every statement in one submission.
fn row_count(outcome: &QueryOutcome) -> u64 {
    outcome
        .statements
        .iter()
        .map(|s| match s {
            StatementResult::Rows(set) => set.rows.len() as u64,
            StatementResult::Affected { rows_affected, .. } => *rows_affected,
        })
        .sum()
}

/// Search the history, newest first.
#[tauri::command(rename_all = "snake_case")]
pub async fn query_history(
    state: tauri::State<'_, AppState>,
    query: HistoryQuery,
) -> IpcResult<Vec<HistoryEntry>> {
    Ok(state.history.lock().await.search(&query))
}

/// Forget history for one connection, or all of it when `connection_id` is null.
#[tauri::command(rename_all = "snake_case")]
pub async fn clear_query_history(
    state: tauri::State<'_, AppState>,
    connection_id: Option<String>,
) -> IpcResult<()> {
    state.history.lock().await.clear(connection_id.as_deref())?;
    Ok(())
}

/// What the UI needs to know about a live session beyond "it is open".
#[derive(Serialize)]
pub struct SessionInfo {
    /// The database this session is pointed at, when the engine has databases.
    pub database: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_info(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<SessionInfo> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(SessionInfo {
        database: guard.current_database().await?,
    })
}

/// Point a session at another database on the same server.
///
/// Two paths, and which one runs is the driver's decision rather than a check
/// on the driver's name. MySQL, SQL Server, and ClickHouse switch in place. A
/// PostgreSQL connection is bound to one database for its lifetime, so its
/// driver reports the operation unsupported and this reconnects instead —
/// through the same tunnel, so a tunnelled connection does not authenticate a
/// second SSH session just to change database.
#[tauri::command(rename_all = "snake_case")]
pub async fn use_database(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    database: String,
) -> IpcResult<String> {
    let session = state.sessions.get(&connection_id).await?;

    {
        let mut guard = session.connection.lock().await;
        match guard.use_database(&database).await {
            Ok(()) => return Ok(database),
            // Fall through to the reconnect below. Any other failure is real —
            // a database that does not exist, or one this login cannot open —
            // and reconnecting would only produce the same error less clearly.
            Err(tablex_core::Error::Unsupported(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }

    let config = state.config_for(&connection_id).await?;
    let driver = state.drivers.get(&config.driver)?;
    let secret = secrets::get(&config.keychain_key())?;

    let mut target = config.clone();
    target.database = Some(database.clone());
    // Point at the existing tunnel's local end rather than the real host, which
    // is what the original connection did too.
    if let Some(port) = session.tunnel_port() {
        target.host = Some("127.0.0.1".into());
        target.port = Some(port);
    }

    // Opened before the old one is dropped: if the new database cannot be
    // reached, the user keeps the session they had rather than being left with
    // none at all.
    let connection = driver.connect(&target, secret.as_deref()).await?;
    session.replace(connection).await;
    Ok(database)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browse(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    parent: Option<String>,
) -> IpcResult<Vec<SchemaNode>> {
    let session = state.sessions.get(&connection_id).await?;
    let queued = std::time::Instant::now();
    let mut guard = session.connection.lock().await;

    // Two timings, not one. A session is serialized behind this lock, so a slow
    // browse is either a slow catalogue query or a fast one that waited for
    // something else on the same connection — and those have opposite fixes.
    let waited = queued.elapsed();
    let started = std::time::Instant::now();
    let nodes = guard.browse(parent.as_deref()).await?;
    tracing::debug!(
        parent = parent.as_deref().unwrap_or("<root>"),
        nodes = nodes.len(),
        waited_ms = waited.as_millis(),
        query_ms = started.elapsed().as_millis(),
        "browse"
    );
    Ok(nodes)
}

/// What the frontend asks for when exporting.
///
/// One struct rather than eight arguments: the command surface is already wide,
/// and a positional list this long is a mis-ordering waiting to happen.
#[derive(Deserialize)]
pub struct ExportArgs {
    /// Identifies this export in progress events and to `cancel_export`.
    pub id: String,
    pub connection_id: String,
    /// The table's name as SQL should refer to it, quoted by the driver.
    pub qualified: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub table: String,
    pub format: tablex_core::export::Format,
    pub path: String,
}

/// Write a table to a file as CSV, JSON, or SQL.
///
/// The path comes from the frontend's save dialog; the writing happens here,
/// because the webview has no filesystem access of its own and should not.
///
/// Progress arrives as events rather than as a return value, because the useful
/// part of a slow export is what it is doing before it finishes.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_table(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: ExportArgs,
) -> IpcResult<u64> {
    let ExportArgs {
        id,
        connection_id,
        qualified,
        schema,
        table,
        format,
        path,
    } = request;
    let started = std::time::Instant::now();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .exports
        .lock()
        .await
        .insert(id.clone(), cancel.clone());

    let result = crate::export::run(
        &state,
        crate::export::ExportRequest {
            id: id.clone(),
            connection_id,
            qualified,
            schema,
            table,
            format,
            path: path.clone(),
        },
        cancel,
        |progress| {
            // A failed emit means the window has gone; the export finishing is
            // still worth doing, and its file is still worth having.
            let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
        },
    )
    .await;

    // Removed however it ended, so a cancelled or failed export does not leave
    // a flag behind for an id that will never be used again.
    state.exports.lock().await.remove(&id);

    let rows = result?;
    tracing::debug!(
        rows,
        path,
        elapsed_ms = started.elapsed().as_millis(),
        "export"
    );
    Ok(rows)
}

/// Ask a running export to stop.
///
/// Sets a flag rather than aborting the task: an export spends most of its time
/// inside a database round trip, and dropping it there would leave the session's
/// protocol stream mid-message for the next query to trip over. It stops at the
/// next batch boundary and takes its half-written file with it.
#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_export(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    if let Some(flag) = state.exports.lock().await.get(&id) {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

/// The statement that would recreate an object, for viewing and editing.
#[tauri::command(rename_all = "snake_case")]
pub async fn object_definition(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    node_id: String,
) -> IpcResult<String> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.definition(&node_id).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn table_detail(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    schema: Option<String>,
    table: String,
) -> IpcResult<TableDetail> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.table_detail(schema.as_deref(), &table).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn apply_edit(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    edit: RowEdit,
) -> IpcResult<()> {
    let config = state.config_for(&connection_id).await?;
    if config.read_only {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.apply_edit(&edit).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn completion_scope(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<tablex_core::driver::CompletionScope> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    let started = std::time::Instant::now();
    let scope = guard.completion_scope().await?;
    // Worth timing because this one holds the session lock while the user is
    // trying to use the tree, and its cost scales with the catalogue rather
    // than with anything they asked for.
    tracing::debug!(
        tables = scope.tables.len(),
        query_ms = started.elapsed().as_millis(),
        "completion scope"
    );
    Ok(scope)
}
