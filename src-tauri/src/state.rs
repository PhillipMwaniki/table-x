//! Process-wide application state.

use crate::{sessions::SessionRegistry, store::ConnectionStore};
use std::path::Path;
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
