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
    designs::Design,
    history::{self, HistoryEntry, HistoryQuery},
    notebooks::Notebook,
    secrets,
    snippets::Snippet,
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
    // One per SSH hop, in chain order. A `None` entry leaves that hop's stored
    // credential alone; `Some("")` clears it.
    ssh_secrets: Option<Vec<Option<String>>>,
) -> IpcResult<()> {
    if config.id.trim().is_empty() {
        return Err(tablex_core::Error::Config("connection id is required".into()).into());
    }
    if !state.drivers.contains(&config.driver) {
        return Err(tablex_core::Error::UnknownDriver(config.driver.clone()).into());
    }

    store_secret(&config.keychain_key(), secret)?;

    // One entry per hop, with the backend deriving the names from the saved
    // chain so a hop added or removed in the middle cannot end up reading the
    // credential of whichever hop used to be in that position.
    //
    // Nothing supplied means nothing was edited, so every stored credential
    // stays as it was — which is what an unchanged form should do.
    if let Some(values) = &ssh_secrets {
        for (index, key) in config.ssh_hop_keys().iter().enumerate() {
            store_secret(key, values.get(index).cloned().flatten())?;
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
            confirm_destructive: None,
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
    state.sessions.remove(&id).await;

    let mut connections = state.connections.lock().await;
    let Some(index) = connections.iter().position(|c| c.id == id) else {
        return Err(tablex_core::Error::UnknownConnection(id).into());
    };
    let removed = connections.remove(index);
    state.store.save(&connections)?;

    // Best effort: a stale keychain entry is untidy but not dangerous, and
    // failing here would leave the config and the keychain inconsistent.
    //
    // Every hop, not just the first: a chained connection stores one credential
    // per hop, and leaving the jump hosts' behind would orphan secrets nothing
    // can reach or clean up afterwards.
    let _ = secrets::delete(&removed.keychain_key());
    for key in removed.ssh_hop_keys() {
        let _ = secrets::delete(&key);
    }
    // Removed unconditionally: a connection that once had a tunnel and no
    // longer does still has the entry, and `ssh_hop_keys` is empty for it.
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
    open_session(&state, &id, None).await
}

/// Rebuild a link that has broken, in place.
///
/// A dropped TCP connection cannot be revived: the socket is gone, and so, in
/// most cases, is the SSH tunnel that carried it — a tunnel is itself a TCP
/// connection to the bastion, and whatever killed one usually killed both. So
/// this opens a genuinely new session, tunnel included, under the same
/// connection id. Everything the user is looking at — tabs, results, history,
/// the transaction indicator — is keyed by that id and stays where it is.
///
/// Two things are deliberately *not* carried over. The session comes back on
/// the database it was pointed at rather than the config's default, because a
/// silent move to another database is how the wrong statement gets run against
/// the wrong data. And any transaction is gone: the server rolled it back when
/// the socket died, and the fresh session says so rather than showing a badge
/// over a transaction that no longer exists.
#[tauri::command(rename_all = "snake_case")]
pub async fn reconnect(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    // A connection whose session is already gone — closed by an earlier
    // disconnect, or never opened — reconnects to the config's own database.
    // That is exactly what connecting does, so it is not an error here.
    let database = match state.sessions.get(&id).await {
        Ok(session) => session.database().await,
        Err(_) => None,
    };
    open_session(&state, &id, database).await
}

/// Open a fresh session and register it, replacing whatever was there.
///
/// `database` overrides the saved config's, for a reconnect that has to come
/// back where it was. The old session is closed by the registry as it is
/// displaced, so a reconnect leaks neither socket nor tunnel.
async fn open_session(state: &AppState, id: &str, database: Option<String>) -> IpcResult<()> {
    let mut config = state.config_for(id).await?;
    let driver = state.drivers.get(&config.driver)?;
    let secret = secrets::get(&config.keychain_key())?;
    if database.is_some() {
        config.database = database;
    }

    let (target, tunnel) = establish_tunnel(&config).await?;
    // Opened before the old session is displaced: if the server is still
    // unreachable, the user keeps whatever they had rather than being left with
    // nothing at all and a second error to read.
    let connection = driver.connect(&target, secret.as_deref()).await?;
    state
        .sessions
        .insert(id, connection, tunnel, config.database)
        .await;
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
    ssh_secrets: Option<Vec<Option<String>>>,
) -> IpcResult<(ConnectionConfig, Option<tablex_tunnel::Tunnel>)> {
    let Some(ssh) = &config.ssh else {
        return Ok((config.clone(), None));
    };

    let target_host = config.host.clone().unwrap_or_else(|| "localhost".into());
    let target_port = config.port.ok_or_else(|| {
        tablex_core::Error::Config("a tunnelled connection needs a target port".into())
    })?;

    // One credential per hop, each under its own keychain entry, so a bastion's
    // key passphrase and a jump host's password never overwrite each other.
    // A secret typed into the form wins for its hop; a blank one falls back to
    // the keychain, which is what "leave it alone to keep the saved credential"
    // has to mean for a field that never displays what it is holding.
    let mut hop_secrets: Vec<Option<String>> = Vec::new();
    for (index, key) in config.ssh_hop_keys().iter().enumerate() {
        let typed = ssh_secrets
            .as_ref()
            .and_then(|values| values.get(index).cloned().flatten())
            .filter(|s| !s.is_empty());
        match typed {
            Some(value) => hop_secrets.push(Some(value)),
            None => hop_secrets.push(secrets::get(key)?),
        }
    }

    let tunnel = tablex_tunnel::open(ssh, &target_host, target_port, &hop_secrets).await?;

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
pub async fn ssh_host_fingerprint(
    ssh: tablex_core::config::SshConfig,
    #[allow(unused_variables)] secrets: Option<Vec<Option<String>>>,
) -> IpcResult<String> {
    // Reaching a jump host means authenticating everything in front of it, so
    // probing one needs those hops' secrets. A directly reachable host needs
    // none, which is why this is optional.
    let secrets = secrets.unwrap_or_default();
    Ok(tablex_tunnel::probe_host_key(&ssh, &secrets).await?)
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
    ssh_secrets: Option<Vec<Option<String>>>,
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
    let (config, tunnel) = establish_tunnel_with(&config, ssh_secrets).await?;

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
    state.sessions.remove(&id).await;
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
    /// Names this run in progress events. Absent for runs nobody is watching
    /// — a notebook cell, a paged fetch — and no events are sent.
    #[serde(default)]
    pub progress_id: Option<String>,
}

/// Emitted after each statement of a multi-statement run.
pub const QUERY_PROGRESS_EVENT: &str = "query-progress";

#[derive(Clone, Serialize)]
pub struct QueryProgress {
    pub id: String,
    /// Statements finished so far.
    pub done: usize,
    pub total: usize,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn execute(
    app: tauri::AppHandle,
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
        execute_reporting(&mut **guard, &request.sql, &opts, |done, total| {
            if let Some(id) = &request.progress_id {
                // A failed emit means the window has gone. The run finishes
                // regardless; what it did to the database is done either way.
                let _ = tauri::Emitter::emit(
                    &app,
                    QUERY_PROGRESS_EVENT,
                    QueryProgress {
                        id: id.clone(),
                        done,
                        total,
                    },
                );
            }
        })
        .await
    };

    // A BEGIN somebody typed leaves the session just as inside a transaction as
    // the button does, and only a submission that succeeded changed anything.
    if outcome.is_ok() {
        session.note_effect(tablex_core::sql::transaction_effect(&request.sql));
    }

    // A paged fetch continues a query that is already in history; recording it
    // again would fill the panel with duplicates of whatever the user scrolled.
    if request.offset == 0 {
        record_history(&state, &config, &request.sql, started, &outcome).await;
    }

    Ok(outcome?)
}

/// Run a submission one statement at a time, reporting after each.
///
/// Every driver already splits a submission and runs the pieces in order, so
/// doing the split here changes nothing about what reaches the server — it only
/// puts a point between statements where progress can be reported. A script of
/// four hundred statements otherwise shows a spinner for its whole duration,
/// and a spinner that has been going for a minute looks the same as a hang.
///
/// A single statement is passed through untouched: there is nothing to report
/// between, and no reason to touch the fast path.
async fn execute_reporting(
    conn: &mut dyn tablex_core::driver::Connection,
    sql: &str,
    opts: &FetchOptions,
    mut report: impl FnMut(usize, usize),
) -> tablex_core::error::Result<QueryOutcome> {
    let statements = tablex_core::sql::split_statements(sql);
    if statements.len() < 2 {
        return conn.execute(sql, opts).await;
    }

    let total = statements.len();
    report(0, total);
    let mut merged = QueryOutcome {
        statements: Vec::with_capacity(total),
        elapsed_ms: 0,
        notices: Vec::new(),
    };
    for (done, statement) in statements.iter().enumerate() {
        let outcome = conn.execute(statement, opts).await?;
        merged.statements.extend(outcome.statements);
        merged.elapsed_ms += outcome.elapsed_ms;
        merged.notices.extend(outcome.notices);
        report(done + 1, total);
    }
    Ok(merged)
}

#[cfg(test)]
mod execute_tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use tablex_core::{
        driver::{Connection, RowEdit},
        error::Result,
        schema::{SchemaNode, TableDetail},
        Error,
    };

    /// Records what it is asked to run, and can be told to fail on a statement.
    struct Recorder {
        ran: Arc<Mutex<Vec<String>>>,
        fail_on: Option<&'static str>,
    }

    #[async_trait]
    impl Connection for Recorder {
        async fn execute(&mut self, sql: &str, _opts: &FetchOptions) -> Result<QueryOutcome> {
            if self.fail_on.is_some_and(|needle| sql.contains(needle)) {
                return Err(Error::query("boom"));
            }
            self.ran.lock().unwrap().push(sql.to_string());
            Ok(QueryOutcome {
                statements: vec![StatementResult::Affected {
                    rows_affected: 1,
                    last_insert_id: None,
                }],
                elapsed_ms: 5,
                notices: vec![format!("ran {sql}")],
            })
        }

        async fn browse(&mut self, _parent: Option<&str>) -> Result<Vec<SchemaNode>> {
            Ok(vec![])
        }

        async fn table_detail(
            &mut self,
            _schema: Option<&str>,
            _table: &str,
        ) -> Result<TableDetail> {
            Err(Error::Unsupported("not needed".into()))
        }

        async fn apply_edit(&mut self, _edit: &RowEdit) -> Result<()> {
            Ok(())
        }

        async fn ping(&mut self) -> Result<()> {
            Ok(())
        }

        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    fn recorder(fail_on: Option<&'static str>) -> (Recorder, Arc<Mutex<Vec<String>>>) {
        let ran = Arc::new(Mutex::new(Vec::new()));
        (
            Recorder {
                ran: ran.clone(),
                fail_on,
            },
            ran,
        )
    }

    #[tokio::test]
    async fn each_statement_is_reported_as_it_finishes() {
        let (mut conn, ran) = recorder(None);
        let mut reports = Vec::new();
        let outcome = execute_reporting(
            &mut conn,
            "INSERT INTO t VALUES (1); INSERT INTO t VALUES (2); SELECT 'a;b'",
            &FetchOptions::default(),
            |done, total| reports.push((done, total)),
        )
        .await
        .unwrap();

        // A zero first, so the bar appears before the slow first statement
        // rather than after it.
        assert_eq!(reports, vec![(0, 3), (1, 3), (2, 3), (3, 3)]);
        assert_eq!(ran.lock().unwrap().len(), 3);
        // The pieces come back as one outcome, the way a driver returns them.
        assert_eq!(outcome.statements.len(), 3);
        assert_eq!(outcome.elapsed_ms, 15);
        assert_eq!(outcome.notices.len(), 3);
    }

    #[tokio::test]
    async fn a_single_statement_goes_straight_through() {
        let (mut conn, ran) = recorder(None);
        let mut reports = Vec::new();
        execute_reporting(
            &mut conn,
            "SELECT 1;",
            &FetchOptions::default(),
            |done, total| reports.push((done, total)),
        )
        .await
        .unwrap();

        assert!(
            reports.is_empty(),
            "nothing to report between one statement"
        );
        // Untouched: the driver sees the text as submitted, trailing `;` and all.
        assert_eq!(ran.lock().unwrap().as_slice(), ["SELECT 1;"]);
    }

    #[tokio::test]
    async fn a_failure_stops_the_run_where_it_happened() {
        let (mut conn, ran) = recorder(Some("oops"));
        let mut reports = Vec::new();
        let err = execute_reporting(
            &mut conn,
            "SELECT 1; SELECT oops; SELECT 3",
            &FetchOptions::default(),
            |done, total| reports.push((done, total)),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, Error::Query { .. }));
        // The third never ran: a script stops at its first error, as it does
        // in every driver.
        assert_eq!(ran.lock().unwrap().as_slice(), ["SELECT 1"]);
        assert_eq!(reports, vec![(0, 3), (1, 3)]);
    }
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
            Ok(()) => {
                drop(guard);
                // So a reconnect after this comes back here rather than on the
                // database the connection was originally saved with.
                session.note_database(Some(database.clone())).await;
                return Ok(database);
            }
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
    session.replace(connection, Some(database.clone())).await;
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

/// What the frontend asks for when dumping a database.
#[derive(Deserialize)]
pub struct DatabaseExportArgs {
    pub id: String,
    pub connection_id: String,
    pub database: String,
    pub path: String,
}

/// Dump a whole database to one SQL file.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_database(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: DatabaseExportArgs,
) -> IpcResult<u64> {
    let started = std::time::Instant::now();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .exports
        .lock()
        .await
        .insert(request.id.clone(), cancel.clone());
    let id = request.id.clone();
    let path = request.path.clone();

    let result = crate::export::run_database(
        &state,
        crate::export::DatabaseExportRequest {
            id: request.id,
            connection_id: request.connection_id,
            database: request.database,
            path: request.path,
        },
        cancel,
        |progress| {
            let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
        },
    )
    .await;

    state.exports.lock().await.remove(&id);
    let rows = result?;
    tracing::debug!(
        rows,
        path,
        elapsed_ms = started.elapsed().as_millis(),
        "database export"
    );
    Ok(rows)
}

/// What the frontend asks for when loading a delimited file.
#[derive(Deserialize)]
pub struct CsvImportArgs {
    pub id: String,
    pub connection_id: String,
    pub path: String,
    pub qualified: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub table: String,
    pub delimiter: String,
    pub has_header: bool,
    /// Target column per field position; null skips that field.
    pub mapping: Vec<Option<String>>,
    pub null_as_empty: bool,
}

/// What a delimited file looks like, for the mapping dialog.
#[derive(Serialize)]
pub struct CsvPreview {
    /// The delimiter used, whether given or sniffed.
    pub delimiter: String,
    pub rows: Vec<Vec<String>>,
}

/// Read the first rows of a delimited file without importing anything.
#[tauri::command(rename_all = "snake_case")]
pub fn preview_csv(path: String, delimiter: Option<String>) -> IpcResult<CsvPreview> {
    let wanted = delimiter.and_then(|d| d.chars().next());
    let (delimiter, rows) = crate::import::preview(&path, wanted, 20)?;
    Ok(CsvPreview {
        delimiter: delimiter.to_string(),
        rows,
    })
}

/// Load a delimited file into a table.
#[tauri::command(rename_all = "snake_case")]
pub async fn import_csv(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: CsvImportArgs,
) -> IpcResult<u64> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .exports
        .lock()
        .await
        .insert(request.id.clone(), cancel.clone());
    let id = request.id.clone();

    let result = crate::import::run_csv(
        &state,
        crate::import::CsvImportRequest {
            id: request.id,
            connection_id: request.connection_id,
            path: request.path,
            qualified: request.qualified,
            schema: request.schema,
            table: request.table,
            delimiter: request.delimiter.chars().next().unwrap_or(','),
            has_header: request.has_header,
            mapping: request.mapping,
            null_as_empty: request.null_as_empty,
        },
        cancel,
        |progress| {
            let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
        },
    )
    .await;

    state.exports.lock().await.remove(&id);
    Ok(result?)
}

/// What the frontend asks for when running a SQL file.
#[derive(Deserialize)]
pub struct ImportArgs {
    pub id: String,
    pub connection_id: String,
    pub path: String,
}

/// Run every statement in a SQL file against a connection.
///
/// Shares the cancellation registry with exports, so one command stops either.
#[tauri::command(rename_all = "snake_case")]
pub async fn import_sql(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: ImportArgs,
) -> IpcResult<u64> {
    let started = std::time::Instant::now();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .exports
        .lock()
        .await
        .insert(request.id.clone(), cancel.clone());
    let id = request.id.clone();
    let path = request.path.clone();

    let result = crate::import::run(
        &state,
        crate::import::ImportRequest {
            id: request.id,
            connection_id: request.connection_id,
            path: request.path,
        },
        cancel,
        |progress| {
            let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
        },
    )
    .await;

    state.exports.lock().await.remove(&id);
    let statements = result?;
    tracing::debug!(
        statements,
        path,
        elapsed_ms = started.elapsed().as_millis(),
        "sql import"
    );
    Ok(statements)
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

/// Pretty-print SQL.
///
/// Lives in the backend because the formatter lives in `tablex-core`, where a
/// future CLI can reach it too — not because formatting needs a database.
#[tauri::command(rename_all = "snake_case")]
pub fn format_sql(sql: String) -> String {
    tablex_core::format::format_sql(&sql)
}

// ---------------------------------------------------------------------------
// Saved queries
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn list_snippets(state: tauri::State<'_, AppState>) -> IpcResult<Vec<Snippet>> {
    Ok(state.snippets.lock().await.list())
}

/// Create or update a saved query, returning it as stored.
///
/// Returned rather than acknowledged, because the store owns the timestamps and
/// the trimmed name — the UI should show what was kept, not what was sent.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_snippet(
    state: tauri::State<'_, AppState>,
    snippet: Snippet,
) -> IpcResult<Snippet> {
    Ok(state.snippets.lock().await.save(snippet)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_snippet(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    state.snippets.lock().await.delete(&id)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_notebooks(state: tauri::State<'_, AppState>) -> IpcResult<Vec<Notebook>> {
    Ok(state.notebooks.lock().await.list())
}

/// Create or update a notebook, returning it as stored.
///
/// Results are not part of what is saved — see the store's own note. A notebook
/// records what to run and why; a stored result would be a claim about a
/// database that may have been true a month ago.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_notebook(
    state: tauri::State<'_, AppState>,
    notebook: Notebook,
) -> IpcResult<Notebook> {
    Ok(state.notebooks.lock().await.save(notebook)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_notebook(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    state.notebooks.lock().await.delete(&id)?;
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

/// Write the query history to a file, as an audit trail that can leave.
///
/// The history is already every statement run, with its timing, its outcome and
/// the connection it ran against. What it could not do was leave the machine —
/// and a record that cannot be handed to somebody is not much of an audit.
///
/// CSV and JSON, the same two formats a result exports to, because the thing
/// someone does next with it is open it in a spreadsheet or feed it to a script.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_history(
    state: tauri::State<'_, AppState>,
    path: String,
    format: String,
    query: HistoryQuery,
) -> IpcResult<u64> {
    let entries = state.history.lock().await.search(&query);

    let text = if format == "json" {
        serde_json::to_string_pretty(&entries)
            .map_err(|e| tablex_core::Error::Other(e.to_string()))?
    } else {
        let mut out = String::from(
            "ran_at,connection,driver,succeeded,elapsed_ms,rows,sql,error
",
        );
        for entry in &entries {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{}
",
                csv_field(&entry.ran_at),
                csv_field(&entry.connection_name),
                csv_field(&entry.driver),
                entry.succeeded,
                entry.elapsed_ms,
                entry.rows.map(|r| r.to_string()).unwrap_or_default(),
                csv_field(&entry.sql),
                csv_field(entry.error.as_deref().unwrap_or("")),
            ));
        }
        out
    };

    std::fs::write(&path, text)
        .map_err(|e| tablex_core::Error::Io(format!("could not write {path}: {e}")))?;
    Ok(entries.len() as u64)
}

/// Quote a CSV field, doubling any quotes inside it.
///
/// SQL is full of commas, quotes and newlines, so every field is quoted rather
/// than only the ones that look like they need it.
fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Stop whatever a connection is running.
///
/// Deliberately does not take the connection lock: the statement being
/// cancelled is holding it.
#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_query(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<()> {
    let session = state.sessions.get(&connection_id).await?;
    Ok(session.cancel().await?)
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

/// A session's transaction state, for the indicator.
#[derive(Serialize)]
pub struct TransactionState {
    /// Whether this engine has transactions at all. `false` hides the controls
    /// rather than offering buttons that can only produce an error.
    pub supported: bool,
    pub open: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn transaction_state(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<TransactionState> {
    let session = state.sessions.get(&connection_id).await?;
    let supported = session
        .connection
        .lock()
        .await
        .transaction_statements()
        .is_some();
    Ok(TransactionState {
        supported,
        open: session.in_transaction(),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn begin_transaction(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<()> {
    let session = state.sessions.get(&connection_id).await?;
    Ok(session.begin().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn commit_transaction(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<()> {
    let session = state.sessions.get(&connection_id).await?;
    Ok(session.commit().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rollback_transaction(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<()> {
    let session = state.sessions.get(&connection_id).await?;
    Ok(session.rollback().await?)
}

/// What a submission would destroy, and whether this connection asks first.
///
/// Analysed in Rust rather than the frontend because the same scanner backs the
/// read-only guard and the MCP server's refusal: three answers to "does this
/// destroy something" would eventually be three different answers.
#[derive(Serialize)]
pub struct HazardReport {
    /// Whether this connection is configured to ask before destroying data.
    pub confirms: bool,
    pub hazards: Vec<HazardItem>,
}

#[derive(Serialize)]
pub struct HazardItem {
    pub summary: String,
    /// Affects everything rather than a chosen subset.
    pub unbounded: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inspect_statement(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    sql: String,
) -> IpcResult<HazardReport> {
    let config = state.config_for(&connection_id).await?;
    Ok(HazardReport {
        confirms: config.confirms_destructive(),
        hazards: tablex_core::sql::hazards(&sql)
            .into_iter()
            .map(|h| HazardItem {
                summary: h.summary,
                unbounded: h.unbounded,
            })
            .collect(),
    })
}

/// A writing statement rewritten as a read of the rows it would touch.
///
/// The connection is named so the rewrite can use the engine's identifier
/// quote and know whether it is talking to Oracle; nothing is sent to the
/// database here. Running the result is the caller's decision.
#[tauri::command(rename_all = "snake_case")]
pub async fn preview_statement(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    sql: String,
) -> IpcResult<tablex_core::preview::Preview> {
    let config = state.config_for(&connection_id).await?;
    let capabilities = state.drivers.get(&config.driver)?.info().capabilities;
    Ok(tablex_core::preview::preview(
        &sql,
        tablex_core::preview::Dialect {
            quote: capabilities.identifier_quote,
            oracle: config.driver == "oracle",
        },
    )?)
}

/// Write the rows the user selected in the grid.
///
/// The rows come from the frontend rather than being re-queried: the selection
/// was made on rows already on screen, and no `WHERE` clause generally
/// reproduces an arbitrary set of them. Writing what was shown is both faster
/// and the only version that is certainly what the user picked.
#[derive(Deserialize)]
pub struct RowExportArgs {
    pub connection_id: String,
    pub path: String,
    pub format: tablex_core::export::Format,
    pub table: String,
    pub columns: Vec<tablex_core::result::Column>,
    pub rows: Vec<Vec<tablex_core::Value>>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn export_rows(
    state: tauri::State<'_, AppState>,
    request: RowExportArgs,
) -> IpcResult<u64> {
    let config = state.config_for(&request.connection_id).await?;
    let quote = state
        .drivers
        .get(&config.driver)?
        .info()
        .capabilities
        .identifier_quote;

    Ok(crate::export::run_rows(crate::export::RowExportRequest {
        path: request.path,
        format: request.format,
        table: request.table,
        columns: request.columns,
        rows: request.rows,
        quote,
    })?)
}

/// As [`export_rows`], but returning the text instead of writing it.
#[derive(Deserialize)]
pub struct RowTextArgs {
    pub connection_id: String,
    pub format: tablex_core::export::Format,
    pub table: String,
    pub columns: Vec<tablex_core::result::Column>,
    pub rows: Vec<Vec<tablex_core::Value>>,
    /// Whether CSV and TSV name their columns on the first line. Absent means
    /// they do, which is what a file export does.
    #[serde(default = "yes")]
    pub header: bool,
}

fn yes() -> bool {
    true
}

/// Rows as text, for the clipboard.
///
/// Formatted here rather than in the webview so that a copied `INSERT` is
/// quoted and escaped by the code that already knows how — including the
/// identifier quote this particular engine uses, which the frontend has no
/// business knowing.
#[tauri::command(rename_all = "snake_case")]
pub async fn format_rows(
    state: tauri::State<'_, AppState>,
    request: RowTextArgs,
) -> IpcResult<String> {
    let config = state.config_for(&request.connection_id).await?;
    let quote = state
        .drivers
        .get(&config.driver)?
        .info()
        .capabilities
        .identifier_quote;

    Ok(crate::export::rows_to_text(
        crate::export::RowTextRequest {
            format: request.format,
            table: request.table,
            columns: request.columns,
            rows: request.rows,
            quote,
            header: request.header,
        },
    )?)
}

/// Who exists on this server, and what each of them can reach.
#[tauri::command(rename_all = "snake_case")]
pub async fn privileges(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<tablex_core::privileges::Privileges> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.privileges().await?)
}

/// One side of a comparison.
#[derive(Deserialize)]
pub struct CompareSide {
    pub connection_id: String,
    #[serde(default)]
    pub schema: Option<String>,
    /// How this side is named in the report.
    pub label: String,
}

#[derive(Deserialize)]
pub struct CompareArgs {
    pub id: String,
    pub from: CompareSide,
    pub to: CompareSide,
    /// Which engine's syntax the script is written in — the one it will be run
    /// against, which is always the `from` side.
    pub driver: String,
}

/// What a comparison found, and the statements that would reconcile it.
#[derive(Serialize)]
pub struct DiffReport {
    pub from: String,
    pub to: String,
    pub changes: Vec<tablex_core::diff::Change>,
    pub statements: Vec<tablex_core::diff::Statement>,
}

/// Compare two schemas and write the migration between them.
///
/// The direction is fixed: the script turns `from` into `to`, and `from` is the
/// side it would be run against. Naming both sides in the report is what keeps
/// that legible once the script is in a tab on its own.
#[tauri::command(rename_all = "snake_case")]
pub async fn compare_schemas(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: CompareArgs,
) -> IpcResult<DiffReport> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .exports
        .lock()
        .await
        .insert(request.id.clone(), cancel.clone());

    let progress = |progress| {
        let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
    };

    let result = async {
        let from = crate::snapshot::capture(
            &state,
            &request.id,
            &request.from.connection_id,
            request.from.schema.as_deref(),
            request.from.label.clone(),
            &cancel,
            &progress,
        )
        .await?;
        let to = crate::snapshot::capture(
            &state,
            &request.id,
            &request.to.connection_id,
            request.to.schema.as_deref(),
            request.to.label.clone(),
            &cancel,
            &progress,
        )
        .await?;

        let changes = tablex_core::diff::diff(&from, &to);
        let statements = tablex_core::diff::migration(
            &changes,
            tablex_core::diff::Dialect::for_driver(&request.driver),
        );
        Ok::<_, tablex_core::Error>(DiffReport {
            from: from.label,
            to: to.label,
            changes,
            statements,
        })
    }
    .await;

    state.exports.lock().await.remove(&request.id);
    Ok(result?)
}

// ---------------------------------------------------------------------------
// Schema designs
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn list_designs(state: tauri::State<'_, AppState>) -> IpcResult<Vec<Design>> {
    Ok(state.designs.lock().await.list())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_design(state: tauri::State<'_, AppState>, design: Design) -> IpcResult<Design> {
    Ok(state.designs.lock().await.save(design)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_design(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    state.designs.lock().await.delete(&id)?;
    Ok(())
}

/// The design laid out for the canvas.
///
/// Laid out in Rust for the same reason a live schema is — it has to come out
/// the same every time — and by the same code, so a design and the database it
/// came from are not drawn by two implementations that disagree.
///
/// Takes the design rather than its id, so the canvas draws what is in front of
/// the user rather than what was last written to disk. An edit that only became
/// visible after a save would make adding a table feel like it had failed.
#[tauri::command(rename_all = "snake_case")]
pub fn design_diagram(design: Design) -> tablex_core::diagram::Diagram {
    design.diagram()
}

/// Read a live schema into a new design.
///
/// Reverse engineering, and the usual way a design starts: most schemas being
/// changed already exist. The design keeps the driver it was read from, because
/// the script it will eventually produce has to be written for one engine and
/// this is the only moment where the right answer is known for certain.
#[tauri::command(rename_all = "snake_case")]
pub async fn design_from_schema(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    connection_id: String,
    schema: Option<String>,
    name: String,
) -> IpcResult<Design> {
    let config = state.config_for(&connection_id).await?;
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let id = uuid::Uuid::new_v4().to_string();

    let progress = |progress| {
        let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
    };

    let snapshot = crate::snapshot::capture(
        &state,
        &id,
        &connection_id,
        schema.as_deref(),
        name.clone(),
        &cancel,
        &progress,
    )
    .await?;

    let design = Design {
        id,
        name,
        driver: config.driver,
        schema,
        tables: snapshot.tables,
        // Empty: nothing has been moved yet, so every table is wherever the
        // layout puts it, which is what a freshly read schema should look like.
        layout: Vec::new(),
        // Not on disk anywhere yet. It becomes a file when somebody says so.
        path: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    Ok(state.designs.lock().await.save(design)?)
}

/// The script that would build this design from nothing.
///
/// Forward engineering, expressed as the difference between an empty schema and
/// the design — which is what it is, and which means it goes through the same
/// migration writer as everything else rather than a second one that would
/// eventually disagree about how to quote a default.
#[tauri::command(rename_all = "snake_case")]
pub async fn design_script(state: tauri::State<'_, AppState>, id: String) -> IpcResult<DiffReport> {
    let design = state
        .designs
        .lock()
        .await
        .get(&id)
        .ok_or_else(|| tablex_core::Error::Config(format!("no such design: {id}")))?;

    let to = design.snapshot();
    let changes = tablex_core::diff::diff(&tablex_core::diff::SchemaSnapshot::default(), &to);
    let statements = tablex_core::diff::migration(
        &changes,
        tablex_core::diff::Dialect::for_driver(&design.driver),
    );
    Ok(DiffReport {
        from: "nothing".into(),
        to: to.label,
        changes,
        statements,
    })
}

/// What it would take to make a database match this design.
///
/// The direction is the one that makes a design useful: the database is what
/// the script runs against, the design is what it is being brought to. Read
/// the other way round it would be a script that undoes the design, which is
/// never what somebody asks for from this screen.
#[tauri::command(rename_all = "snake_case")]
pub async fn design_sync(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    connection_id: String,
    schema: Option<String>,
) -> IpcResult<DiffReport> {
    let design = state
        .designs
        .lock()
        .await
        .get(&id)
        .ok_or_else(|| tablex_core::Error::Config(format!("no such design: {id}")))?;

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let job = uuid::Uuid::new_v4().to_string();
    state
        .exports
        .lock()
        .await
        .insert(job.clone(), cancel.clone());

    let progress = |progress| {
        let _ = tauri::Emitter::emit(&app, crate::export::PROGRESS_EVENT, progress);
    };

    let config = state.config_for(&connection_id).await?;
    let live = crate::snapshot::capture(
        &state,
        &job,
        &connection_id,
        schema.as_deref(),
        config.name.clone(),
        &cancel,
        &progress,
    )
    .await;
    state.exports.lock().await.remove(&job);
    let live = live?;

    let to = design.snapshot();
    let changes = tablex_core::diff::diff(&live, &to);
    // The connection's engine, not the design's: the statements are going to be
    // run against this database, and a design written for one engine can still
    // be compared against another. Where that produces something the engine
    // cannot do, the migration writer says so rather than emitting it.
    let statements = tablex_core::diff::migration(
        &changes,
        tablex_core::diff::Dialect::for_driver(&config.driver),
    );
    Ok(DiffReport {
        from: live.label,
        to: to.label,
        changes,
        statements,
    })
}

/// Write a design to a `.erd` file.
///
/// The file is a copy that can be moved, mailed, or committed; the store keeps
/// its own. What the two share is the design's id, so opening the file again --
/// here or on another machine -- continues the same design rather than starting
/// a second one that slowly diverges from it.
#[tauri::command(rename_all = "snake_case")]
pub async fn write_design_file(
    state: tauri::State<'_, AppState>,
    id: String,
    path: String,
) -> IpcResult<Design> {
    let mut design = state
        .designs
        .lock()
        .await
        .get(&id)
        .ok_or_else(|| tablex_core::Error::Config(format!("no such design: {id}")))?;

    std::fs::write(&path, design.to_file()?)
        .map_err(|e| tablex_core::Error::Io(format!("could not write {path}: {e}")))?;

    // Remembered so that saving again knows where again is.
    design.path = Some(path);
    Ok(state.designs.lock().await.save(design)?)
}

/// Open a `.erd` file, and keep it.
///
/// Reading one adds it to the designs this machine knows about, so a file that
/// arrived from somewhere else behaves from then on like any other design —
/// including being listed, compared against a database, and saved back to the
/// file it came from.
#[tauri::command(rename_all = "snake_case")]
pub async fn read_design_file(
    state: tauri::State<'_, AppState>,
    path: String,
) -> IpcResult<Design> {
    let bytes = std::fs::read(&path)
        .map_err(|e| tablex_core::Error::Io(format!("could not read {path}: {e}")))?;
    let design = Design::from_file(&bytes, &path)?;
    Ok(state.designs.lock().await.save(design)?)
}

/// Design files this launch was asked to open.
///
/// Double-clicking a `.erd` file starts the app with the path as an argument,
/// so the frontend asks for them once it is ready rather than the backend
/// pushing them at a window that may not exist yet. A second double-click while
/// the app is running arrives as an event instead — see the single-instance
/// handler.
#[tauri::command(rename_all = "snake_case")]
pub fn startup_designs() -> Vec<String> {
    crate::design_arguments(std::env::args())
}

/// The schema as a diagram, already laid out.
///
/// The layout happens in Rust rather than in the view because it has to be the
/// same every time — a diagram that reshuffles itself when reopened cannot be
/// learned — and determinism is easier to pin down with a test than to promise.
#[tauri::command(rename_all = "snake_case")]
pub async fn schema_diagram(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    schema: Option<String>,
) -> IpcResult<tablex_core::diagram::Diagram> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    let graph = guard.schema_graph(schema.as_deref()).await?;
    Ok(tablex_core::diagram::layout(&graph))
}

/// How the engine intends to run a statement.
///
/// `analyze` measures instead of estimating, which means running the statement.
/// Only offered where the driver can also undo it — see the capability.
#[tauri::command(rename_all = "snake_case")]
pub async fn explain(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    sql: String,
    analyze: bool,
) -> IpcResult<tablex_core::plan::Plan> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.explain(&sql, analyze).await?)
}

/// What the server is doing right now.
///
/// Read fresh on every call and never cached: an activity list that is a minute
/// old describes a server that no longer exists.
#[tauri::command(rename_all = "snake_case")]
pub async fn server_activity(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> IpcResult<tablex_core::activity::ServerActivity> {
    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.activity().await?)
}

/// End someone's session.
///
/// Gated on read-only for the same reason an edit is: the flag means this
/// connection does not change the server, and disconnecting another user is a
/// change to the server whatever else it is.
#[tauri::command(rename_all = "snake_case")]
pub async fn kill_session(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    session_id: String,
) -> IpcResult<()> {
    let config = state.config_for(&connection_id).await?;
    if config.read_only {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let session = state.sessions.get(&connection_id).await?;
    let mut guard = session.connection.lock().await;
    Ok(guard.kill_session(&session_id).await?)
}

/// Refuse a grid edit while the user is holding a transaction open.
///
/// Every driver's edit path opens a transaction of its own — that is the whole
/// mechanism behind "exactly one row or nothing", and it is what makes inline
/// editing safe. Nested inside a transaction the user opened, the same mechanism
/// turns dangerous rather than merely redundant: MySQL's `START TRANSACTION`
/// implicitly commits the pending one, and PostgreSQL's inner `COMMIT` ends the
/// outer one. Either way an inline cell edit would silently commit work somebody
/// opened a transaction specifically to review first.
///
/// So it is refused, and named. The statement editor still works — a typed
/// `UPDATE` runs inside the transaction exactly as expected, because it is not
/// wrapped in one.
///
/// The proper fix is a savepoint around the row check rather than a transaction,
/// which every one of these engines supports; until each driver does that, this
/// is the honest answer.
fn refuse_edit_inside_transaction(session: &crate::sessions::Session) -> IpcResult<()> {
    if session.in_transaction() {
        return Err(tablex_core::Error::Unsupported(
            "grid edits are not available while a transaction is open — commit or roll back              first, or write the statement in the editor, which does run inside it"
                .into(),
        )
        .into());
    }
    Ok(())
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
    refuse_edit_inside_transaction(&session)?;
    let mut guard = session.connection.lock().await;
    Ok(guard.apply_edit(&edit).await?)
}

/// Add a row.
///
/// Gated on read-only exactly as an edit is: the flag means this connection
/// does not change the database, and a new row is a change.
#[tauri::command(rename_all = "snake_case")]
pub async fn insert_row(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    insert: tablex_core::driver::RowInsert,
) -> IpcResult<()> {
    let config = state.config_for(&connection_id).await?;
    if config.read_only {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let session = state.sessions.get(&connection_id).await?;
    refuse_edit_inside_transaction(&session)?;
    let mut guard = session.connection.lock().await;
    Ok(guard.insert_row(&insert).await?)
}

/// Remove a row.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_row(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    delete: tablex_core::driver::RowDelete,
) -> IpcResult<()> {
    let config = state.config_for(&connection_id).await?;
    if config.read_only {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let session = state.sessions.get(&connection_id).await?;
    refuse_edit_inside_transaction(&session)?;
    let mut guard = session.connection.lock().await;
    Ok(guard.delete_row(&delete).await?)
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

// ---------------------------------------------------------------------------
// Editing a table's structure
// ---------------------------------------------------------------------------

/// A set of structure edits, as the editor collected them.
#[derive(Debug, serde::Deserialize)]
pub struct TableChanges {
    pub connection_id: String,
    pub changes: Vec<tablex_core::diff::Change>,
}

/// What running the edits would do, before any of it happens.
#[derive(Debug, serde::Serialize)]
pub struct DdlPlan {
    pub statements: Vec<tablex_core::diff::Statement>,
    /// Changes this engine will not be asked to make, each with its reason.
    /// Non-empty means [`apply_table_changes`] will refuse.
    pub refusals: Vec<String>,
    /// Whether a failure partway through leaves the earlier statements applied.
    pub transactional: bool,
}

/// Create a database.
///
/// One statement, built here rather than in the driver because the only part
/// that differs between engines is the identifier quote, which the capability
/// already carries. Refused where the engine has no such statement, so a UI
/// that somehow offered the option still cannot produce one it would reject.
#[tauri::command(rename_all = "snake_case")]
pub async fn create_database(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    name: String,
) -> IpcResult<()> {
    run_creation(&state, &connection_id, &name, Container::Database).await
}

/// Create a schema inside the database this session is connected to.
#[tauri::command(rename_all = "snake_case")]
pub async fn create_schema(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    name: String,
) -> IpcResult<()> {
    run_creation(&state, &connection_id, &name, Container::Schema).await
}

/// Which of the two containers is being made.
#[derive(Clone, Copy)]
enum Container {
    Database,
    Schema,
}

async fn run_creation(
    state: &AppState,
    connection_id: &str,
    name: &str,
    what: Container,
) -> IpcResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(tablex_core::Error::Config("a name is required".into()).into());
    }

    let config = state.config_for(connection_id).await?;
    if config.read_only {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let capabilities = state.drivers.get(&config.driver)?.info().capabilities;
    let (allowed, keyword) = match what {
        Container::Database => (capabilities.ddl.create_database, "DATABASE"),
        Container::Schema => (capabilities.ddl.create_schema, "SCHEMA"),
    };
    if !allowed {
        return Err(tablex_core::Error::Unsupported(format!(
            "{} cannot create a {}",
            config.driver,
            keyword.to_lowercase()
        ))
        .into());
    }

    // Quoted, so a name with a space or a reserved word in it is a name rather
    // than a syntax error -- and so that a name carrying the quote character
    // cannot end the identifier early.
    let statement = format!(
        "CREATE {keyword} {}",
        tablex_core::sql::quote_ident(name, capabilities.identifier_quote)
    );

    let session = state.sessions.get(connection_id).await?;
    let mut guard = session.connection.lock().await;
    guard.run_control(&statement).await?;
    Ok(())
}

/// Build the statements for a set of edits without running any of them.
///
/// Separate from applying them on purpose. The editor shows this, the user reads
/// it, and only then is there anything to run — which is the same bargain the
/// schema diff makes, and the reason the structure view was read-only until now.
#[tauri::command(rename_all = "snake_case")]
pub async fn preview_table_changes(
    state: tauri::State<'_, AppState>,
    request: TableChanges,
) -> IpcResult<DdlPlan> {
    Ok(plan_changes(&state, &request).await?)
}

async fn plan_changes(
    state: &AppState,
    request: &TableChanges,
) -> Result<DdlPlan, tablex_core::Error> {
    let config = state.config_for(&request.connection_id).await?;
    let capabilities = state.drivers.get(&config.driver)?.info().capabilities;

    let refusals = request
        .changes
        .iter()
        .filter_map(|c| tablex_core::diff::refusal(c, capabilities.ddl))
        .collect();

    let statements = tablex_core::diff::migration(
        &request.changes,
        tablex_core::diff::Dialect::for_driver(&config.driver),
    );

    Ok(DdlPlan {
        statements,
        refusals,
        transactional: capabilities.ddl.transactional_ddl && capabilities.transactions,
    })
}

/// How far an apply got.
#[derive(Debug, serde::Serialize)]
pub struct DdlOutcome {
    pub applied: usize,
    pub elapsed_ms: u64,
}

/// Run the statements a preview produced.
///
/// Refuses rather than half-tries: a read-only connection, a change this engine
/// cannot make, or a statement the dialect could only render as a comment all
/// stop the whole set before the first one runs. Getting halfway through a
/// migration is worse than not starting, and the checks that can be made without
/// touching the database are made first.
#[tauri::command(rename_all = "snake_case")]
pub async fn apply_table_changes(
    state: tauri::State<'_, AppState>,
    request: TableChanges,
) -> IpcResult<DdlOutcome> {
    let config = state.config_for(&request.connection_id).await?;

    // The same guard `execute` applies, for the same reason: this is protection
    // against running the wrong thing against production, not a security
    // boundary the database is unaware of.
    if config.read_only {
        return Err(
            tablex_core::Error::Unsupported("this connection is marked read-only".into()).into(),
        );
    }

    let plan = plan_changes(&state, &request).await?;

    if let Some(reason) = plan.refusals.first() {
        return Err(tablex_core::Error::Unsupported(reason.clone()).into());
    }
    if let Some(blocked) = plan.statements.iter().find(|s| s.unsupported) {
        return Err(tablex_core::Error::Unsupported(
            blocked
                .note
                .clone()
                .unwrap_or_else(|| "this engine has no statement for that change".into()),
        )
        .into());
    }
    if plan.statements.is_empty() {
        return Err(tablex_core::Error::query("there is nothing to apply").into());
    }

    let started = std::time::Instant::now();
    let session = state.sessions.get(&request.connection_id).await?;
    let opts = FetchOptions::default();

    let mut guard = session.connection.lock().await;
    let tx = plan
        .transactional
        .then(|| guard.transaction_statements())
        .flatten();

    if let Some(tx) = tx {
        guard.execute(tx.begin, &opts).await?;
    }

    let mut applied = 0usize;
    for statement in &plan.statements {
        match guard.execute(&statement.sql, &opts).await {
            Ok(_) => applied += 1,
            Err(e) => {
                if let Some(tx) = tx {
                    // Best effort: the rollback failing must not replace the
                    // error that explains why we are rolling back.
                    let _ = guard.execute(tx.rollback, &opts).await;
                    return Err(e.into());
                }
                // Nothing to undo with, so the honest thing is to say how far it
                // got. "Statement 3 failed" is useless without knowing that 1 and
                // 2 are still in place.
                return Err(tablex_core::Error::query(format!(
                    "statement {} of {} failed and this engine does not roll DDL back, \
                     so the {} before it are already applied: {e}",
                    applied + 1,
                    plan.statements.len(),
                    applied,
                ))
                .into());
            }
        }
    }

    if let Some(tx) = tx {
        guard.execute(tx.commit, &opts).await?;
    }

    Ok(DdlOutcome {
        applied,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}
