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
    result::QueryOutcome,
    schema::{SchemaNode, TableDetail},
    ConnectionConfig, ErrorPayload,
};

use crate::{secrets, state::AppState};

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
pub async fn list_connections(state: tauri::State<'_, AppState>) -> IpcResult<Vec<ConnectionConfig>> {
    Ok(state.connections.lock().await.clone())
}

/// Ids of connections with a live session, so the UI can show which are open.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_connections(state: tauri::State<'_, AppState>) -> IpcResult<Vec<String>> {
    Ok(state.sessions.open_ids().await)
}

/// Create or update a saved connection.
///
/// The secret is passed separately and never travels inside the config, so it
/// cannot end up in the JSON file by accident. Passing `None` leaves any existing
/// keychain entry untouched, which is what lets the UI save an edited connection
/// without re-prompting for the password.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_connection(
    state: tauri::State<'_, AppState>,
    config: ConnectionConfig,
    secret: Option<String>,
) -> IpcResult<()> {
    if config.id.trim().is_empty() {
        return Err(tablex_core::Error::Config("connection id is required".into()).into());
    }
    if !state.drivers.contains(&config.driver) {
        return Err(tablex_core::Error::UnknownDriver(config.driver.clone()).into());
    }

    if let Some(secret) = secret {
        if secret.is_empty() {
            secrets::delete(&config.keychain_key())?;
        } else {
            secrets::set(&config.keychain_key(), &secret)?;
        }
    }

    let mut connections = state.connections.lock().await;
    match connections.iter_mut().find(|c| c.id == config.id) {
        Some(existing) => *existing = config,
        None => connections.push(config),
    }
    state.store.save(&connections)?;
    Ok(())
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
    let Some(ssh) = &config.ssh else {
        return Ok((config.clone(), None));
    };

    let target_host = config.host.clone().unwrap_or_else(|| "localhost".into());
    let target_port = config.port.ok_or_else(|| {
        tablex_core::Error::Config("a tunnelled connection needs a target port".into())
    })?;

    // The SSH credential is stored separately from the database password, so a
    // key passphrase and a database password never overwrite each other.
    let ssh_secret = secrets::get(&config.ssh_keychain_key())?;
    let tunnel =
        tablex_tunnel::open(ssh, &target_host, target_port, ssh_secret.as_deref()).await?;

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
    // takes rather than reporting success on a route that will not be used.
    let (config, tunnel) = establish_tunnel(&config).await?;

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
        return Err(tablex_core::Error::Unsupported(
            "this connection is marked read-only".into(),
        )
        .into());
    }

    let defaults = FetchOptions::default();
    let opts = FetchOptions {
        max_rows: request.max_rows.or(defaults.max_rows),
        offset: request.offset,
        timeout_secs: request.timeout_secs.or(defaults.timeout_secs),
    };

    let session = state.sessions.get(&request.connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.execute(&request.sql, &opts).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browse(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    parent: Option<String>,
) -> IpcResult<Vec<SchemaNode>> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.browse(parent.as_deref()).await?)
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
        return Err(tablex_core::Error::Unsupported(
            "this connection is marked read-only".into(),
        )
        .into());
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
    Ok(guard.completion_scope().await?)
}
