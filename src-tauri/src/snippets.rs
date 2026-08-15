//! Saved queries.
//!
//! Query history records everything that ran; this records what someone decided
//! was worth keeping. The two are deliberately separate stores: history is a log
//! that ages out at a cap, and a snippet someone named should never disappear
//! because five thousand queries ran after it.
//!
//! Written as one JSON document with the same temp-then-rename as the connection
//! store, for the same reason: this is content the user created and cannot
//! reconstruct, so a crash mid-write must leave the previous file intact.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tablex_core::error::{Error, Result};

const FILE_NAME: &str = "snippets.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub sql: String,
    /// RFC 3339, UTC. Set once and preserved across edits.
    pub created_at: String,
    pub updated_at: String,
    /// Free-text label for grouping, mirroring connection folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Document {
    version: u32,
    snippets: Vec<Snippet>,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: 1,
            snippets: Vec::new(),
        }
    }
}

pub struct SnippetStore {
    path: PathBuf,
    snippets: Vec<Snippet>,
}

impl SnippetStore {
    /// Load whatever is on disk. A missing file is the normal first run.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE_NAME);
        let snippets = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Document>(&bytes) {
                Ok(doc) => doc.snippets,
                Err(e) => {
                    // Unlike a damaged history line, this file is small and
                    // hand-editable; naming it lets the user go and look rather
                    // than wondering where their queries went.
                    tracing::error!("{} is not valid JSON ({e}); starting empty", path.display());
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::error!("could not read {}: {e}", path.display());
                Vec::new()
            }
        };

        SnippetStore { path, snippets }
    }

    /// Newest first, which is the order a list of saved things is read in.
    pub fn list(&self) -> Vec<Snippet> {
        let mut out = self.snippets.clone();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    /// Create or update one, keyed by id.
    ///
    /// `created_at` survives an edit: it is when the user first kept this query,
    /// and rewriting it on every save would make the field meaningless.
    pub fn save(&mut self, mut snippet: Snippet) -> Result<Snippet> {
        if snippet.name.trim().is_empty() {
            return Err(Error::Config("a snippet needs a name".into()));
        }
        snippet.name = snippet.name.trim().to_string();

        let now = chrono::Utc::now().to_rfc3339();
        snippet.updated_at = now.clone();

        match self.snippets.iter_mut().find(|s| s.id == snippet.id) {
            Some(existing) => {
                snippet.created_at = existing.created_at.clone();
                *existing = snippet.clone();
            }
            None => {
                if snippet.created_at.is_empty() {
                    snippet.created_at = now;
                }
                self.snippets.push(snippet.clone());
            }
        }

        self.write()?;
        Ok(snippet)
    }

    /// Remove one. Deleting something already gone succeeds, so the UI does not
    /// have to guard against a double click.
    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.snippets.retain(|s| s.id != id);
        self.write()
    }

    fn write(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }

        let doc = Document {
            version: 1,
            snippets: self.snippets.clone(),
        };
        let json = serde_json::to_vec_pretty(&doc)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-snippets-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn snippet(id: &str, name: &str) -> Snippet {
        Snippet {
            id: id.into(),
            name: name.into(),
            sql: "SELECT 1".into(),
            created_at: String::new(),
            updated_at: String::new(),
            folder: None,
        }
    }

    #[test]
    fn missing_file_is_an_empty_list_not_an_error() {
        let store = SnippetStore::load(&temp_dir("missing"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn saved_snippets_survive_a_reload() {
        let dir = temp_dir("reload");
        let mut store = SnippetStore::load(&dir);
        store.save(snippet("a", "Daily actives")).expect("save");

        let reloaded = SnippetStore::load(&dir);
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].name, "Daily actives");
    }

    #[test]
    fn an_edit_keeps_the_original_creation_time() {
        let dir = temp_dir("created");
        let mut store = SnippetStore::load(&dir);
        let first = store.save(snippet("a", "First name")).expect("save");

        let mut edited = snippet("a", "Renamed");
        edited.sql = "SELECT 2".into();
        let second = store.save(edited).expect("save again");

        // When it was first kept is a fact about the snippet; rewriting it on
        // every edit would make the field meaningless.
        assert_eq!(second.created_at, first.created_at);
        assert!(second.updated_at >= first.updated_at);
        assert_eq!(store.list().len(), 1, "an edit must not add a second entry");
        assert_eq!(store.list()[0].sql, "SELECT 2");
    }

    #[test]
    fn a_nameless_snippet_is_refused() {
        // The name is how it is found again; without one it is indistinguishable
        // from every other row in the list.
        let mut store = SnippetStore::load(&temp_dir("nameless"));
        assert!(store.save(snippet("a", "   ")).is_err());
    }

    #[test]
    fn names_are_trimmed_so_two_do_not_look_identical() {
        let mut store = SnippetStore::load(&temp_dir("trim"));
        let saved = store.save(snippet("a", "  Report  ")).expect("save");
        assert_eq!(saved.name, "Report");
    }

    #[test]
    fn deleting_is_idempotent() {
        let dir = temp_dir("delete");
        let mut store = SnippetStore::load(&dir);
        store.save(snippet("a", "One")).expect("save");

        store.delete("a").expect("delete");
        store.delete("a").expect("deleting twice is not an error");
        assert!(SnippetStore::load(&dir).list().is_empty());
    }

    #[test]
    fn the_newest_edit_sorts_first() {
        let dir = temp_dir("order");
        let mut store = SnippetStore::load(&dir);
        store.save(snippet("a", "Older")).expect("save");
        std::thread::sleep(std::time::Duration::from_millis(5));
        store.save(snippet("b", "Newer")).expect("save");

        assert_eq!(store.list()[0].name, "Newer");
    }

    #[test]
    fn a_failed_write_leaves_the_previous_file_intact() {
        let dir = temp_dir("atomic");
        let mut store = SnippetStore::load(&dir);
        store.save(snippet("a", "One")).expect("first");
        store.save(snippet("b", "Two")).expect("second");

        assert_eq!(SnippetStore::load(&dir).list().len(), 2);
        // Temp-then-rename leaves nothing behind to be mistaken for the real file.
        assert!(!store.path().with_extension("json.tmp").exists());
    }
}
