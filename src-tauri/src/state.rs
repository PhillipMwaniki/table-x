//! Process-wide application state.

use crate::{
    history::QueryHistory, sessions::SessionRegistry, snippets::SnippetStore,
    store::ConnectionStore,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tablex_core::{
    error::{Error, Result},
    registry::DriverRegistry,
    ConnectionConfig,
};
use tokio::sync::Mutex;

pub struct AppState {
    /// Drivers compiled into this build.
    pub drivers: DriverRegistry,
    /// Live sessions, keyed by connection id.
    pub sessions: SessionRegistry,
    /// Saved connections, mirrored to disk on every change.
    pub connections: Mutex<Vec<ConnectionConfig>>,
    pub store: ConnectionStore,
    /// Executed statements, appended to disk as they run.
    pub history: Mutex<QueryHistory>,
    /// Queries the user chose to keep, which is a different thing from a log.
    pub snippets: Mutex<SnippetStore>,
    /// Cancellation flags for exports currently running, by export id.
    ///
    /// A flag rather than an abort: the export is inside a database round trip
    /// most of the time, and dropping the task mid-statement would leave the
    /// session's protocol stream in a state the next query would trip over.
    pub exports: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(config_dir: &Path) -> Self {
        let store = ConnectionStore::new(config_dir);
        // A corrupt or unreadable file must not stop the app from starting; the
        // error is logged and the user sees an empty list they can still add to,
        // while the file itself is left untouched for inspection.
        let connections = store.load().unwrap_or_else(|e| {
            tracing::error!("could not load saved connections: {e}");
            Vec::new()
        });

        AppState {
            drivers: tablex_drivers::registry(),
            sessions: SessionRegistry::new(),
            connections: Mutex::new(connections),
            store,
            history: Mutex::new(QueryHistory::load(config_dir)),
            snippets: Mutex::new(SnippetStore::load(config_dir)),
            exports: Mutex::new(HashMap::new()),
        }
    }

    /// The saved config for a connection id.
    pub async fn config_for(&self, id: &str) -> Result<ConnectionConfig> {
        self.connections
            .lock()
            .await
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| Error::UnknownConnection(id.to_string()))
    }
}
