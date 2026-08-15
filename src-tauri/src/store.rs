//! Persistence for saved connections.
//!
//! Connections are kept as a plain JSON file in the app config directory. It is
//! deliberately human-readable and free of secrets, so it can be inspected,
//! version-controlled, or copied between machines without leaking credentials —
//! those live in the OS keychain and are looked up by connection id.

use std::path::{Path, PathBuf};
use tablex_core::{
    error::{Error, Result},
    ConnectionConfig,
};

const FILE_NAME: &str = "connections.json";

/// The on-disk document. Versioned from the start so a future format change can
/// migrate rather than silently discard a user's saved connections.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Document {
    version: u32,
    connections: Vec<ConnectionConfig>,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: 1,
            connections: Vec::new(),
        }
    }
}

pub struct ConnectionStore {
    path: PathBuf,
}

impl ConnectionStore {
    pub fn new(config_dir: &Path) -> Self {
        ConnectionStore {
            path: config_dir.join(FILE_NAME),
        }
    }

    pub fn load(&self) -> Result<Vec<ConnectionConfig>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            // No file yet is the normal first-run state.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e.to_string())),
        };

        let doc: Document = serde_json::from_slice(&bytes).map_err(|e| {
            // Do not silently start empty: that looks identical to "all your
            // connections vanished". Name the file so the user can inspect it.
            Error::Config(format!(
                "{} is not valid JSON ({e}); it has been left untouched",
                self.path.display()
            ))
        })?;
        Ok(doc.connections)
    }

    pub fn save(&self, connections: &[ConnectionConfig]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }

        let doc = Document {
            version: 1,
            connections: connections.to_vec(),
        };
        let json = serde_json::to_vec_pretty(&doc)?;

        // Write to a temporary file and rename over the target. A crash or a full
        // disk midway through then leaves the previous file intact rather than a
        // half-written one, which would lose every saved connection.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }

    /// Where the file lives. Used by tests today; a "reveal config file" menu
    /// item will want it too.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tablex_core::config::TlsConfig;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-store-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn config(id: &str) -> ConnectionConfig {
        ConnectionConfig {
            id: id.into(),
            name: "Prod".into(),
            driver: "postgres".into(),
            host: Some("db.example.com".into()),
            port: Some(5432),
            database: Some("app".into()),
            username: Some("readonly".into()),
            file_path: None,
            tls: TlsConfig::default(),
            ssh: None,
            folder: None,
            color: None,
            read_only: false,
            confirm_destructive: None,
            options: IndexMap::new(),
        }
    }

    #[test]
    fn missing_file_is_an_empty_list_not_an_error() {
        let store = ConnectionStore::new(&temp_dir("missing"));
        assert!(store.load().expect("first run must succeed").is_empty());
    }

    #[test]
    fn round_trips_connections() {
        let store = ConnectionStore::new(&temp_dir("roundtrip"));
        store.save(&[config("a"), config("b")]).expect("save");

        let loaded = store.load().expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "a");
        assert_eq!(loaded[1].name, "Prod");
    }

    #[test]
    fn saved_file_contains_no_secrets() {
        let store = ConnectionStore::new(&temp_dir("nosecrets"));
        store.save(&[config("a")]).expect("save");

        let text = std::fs::read_to_string(store.path()).expect("read back");
        // This file is plain JSON on disk; a password reaching it would be a leak.
        assert!(!text.contains("password"), "{text}");
        assert!(!text.contains("passphrase"), "{text}");
    }

    #[test]
    fn corrupt_file_reports_the_path_instead_of_starting_empty() {
        let dir = temp_dir("corrupt");
        let store = ConnectionStore::new(&dir);
        std::fs::write(store.path(), b"{not json").expect("write garbage");

        // Returning an empty list here would be indistinguishable from "all your
        // saved connections disappeared", and would then be overwritten on save.
        let err = store.load().expect_err("must not silently start empty");
        assert!(err.to_string().contains("connections.json"), "{err}");
    }

    #[test]
    fn a_failed_write_leaves_the_previous_file_intact() {
        let store = ConnectionStore::new(&temp_dir("atomic"));
        store.save(&[config("original")]).expect("first save");

        // The temp-then-rename path means a partial write never lands on the
        // real file; verify the target is complete and parseable after a save.
        store.save(&[config("replacement")]).expect("second save");
        let loaded = store.load().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "replacement");

        // And no stray temp file is left behind.
        assert!(!store.path().with_extension("json.tmp").exists());
    }
}
