//! Notebooks: prose and queries kept together.
//!
//! A snippet is a statement worth keeping. A notebook is the reasoning around
//! several of them — why this query, what its result showed, what to run next.
//! That is a different thing to store and a different thing to lose, so it gets
//! its own file rather than a `kind` column on snippets.
//!
//! Results are deliberately not saved. A notebook records what to run and why,
//! and a stored result would be a claim about a database that may have been
//! true a month ago. Re-running is cheap; being quietly wrong is not.
//!
//! Same temp-then-rename write as the other stores: this is content the user
//! wrote and cannot reconstruct.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tablex_core::error::{Error, Result};

const FILE_NAME: &str = "notebooks.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    Markdown,
    Sql,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub id: String,
    pub kind: CellKind,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    pub id: String,
    pub name: String,
    pub cells: Vec<Cell>,
    /// The connection it was written against, so opening it reconnects to the
    /// right database rather than running against whatever is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    /// RFC 3339, UTC. Set once and preserved across edits.
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Document {
    version: u32,
    notebooks: Vec<Notebook>,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: 1,
            notebooks: Vec::new(),
        }
    }
}

pub struct NotebookStore {
    path: PathBuf,
    notebooks: Vec<Notebook>,
}

impl NotebookStore {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE_NAME);
        let notebooks = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Document>(&bytes) {
                Ok(doc) => doc.notebooks,
                Err(e) => {
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

        NotebookStore { path, notebooks }
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Notebook> {
        let mut out = self.notebooks.clone();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn save(&mut self, mut notebook: Notebook) -> Result<Notebook> {
        if notebook.name.trim().is_empty() {
            return Err(Error::Config("a notebook needs a name".into()));
        }
        notebook.name = notebook.name.trim().to_string();

        let now = chrono::Utc::now().to_rfc3339();
        notebook.updated_at = now.clone();

        match self.notebooks.iter_mut().find(|n| n.id == notebook.id) {
            Some(existing) => {
                // When the user first wrote it, not when they last touched it.
                notebook.created_at = existing.created_at.clone();
                *existing = notebook.clone();
            }
            None => {
                if notebook.created_at.is_empty() {
                    notebook.created_at = now;
                }
                self.notebooks.push(notebook.clone());
            }
        }

        self.write()?;
        Ok(notebook)
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.notebooks.retain(|n| n.id != id);
        self.write()
    }

    fn write(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }

        let doc = Document {
            version: 1,
            notebooks: self.notebooks.clone(),
        };
        let json = serde_json::to_vec_pretty(&doc)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-notebooks-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn notebook(id: &str, name: &str) -> Notebook {
        Notebook {
            id: id.into(),
            name: name.into(),
            cells: vec![Cell {
                id: "c1".into(),
                kind: CellKind::Sql,
                source: "SELECT 1".into(),
            }],
            connection_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_notebook_survives_a_reload() {
        let dir = scratch("reload");
        let mut store = NotebookStore::load(&dir);
        store.save(notebook("n1", "Investigation")).expect("save");

        let reloaded = NotebookStore::load(&dir);
        let list = reloaded.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Investigation");
        assert_eq!(list[0].cells[0].source, "SELECT 1");
    }

    #[test]
    fn editing_keeps_the_date_it_was_written() {
        // Rewriting it on every save would make the field mean "last edited"
        // twice over and leave nothing recording when it began.
        let dir = scratch("created");
        let mut store = NotebookStore::load(&dir);
        let first = store.save(notebook("n1", "One")).expect("save");

        let mut edited = notebook("n1", "One renamed");
        edited.created_at = String::new();
        let second = store.save(edited).expect("save again");

        assert_eq!(second.created_at, first.created_at);
        assert_ne!(second.updated_at, String::new());
        assert_eq!(store.list().len(), 1, "an edit must not add a second one");
    }

    #[test]
    fn a_notebook_needs_a_name() {
        let dir = scratch("unnamed");
        let mut store = NotebookStore::load(&dir);
        assert!(store.save(notebook("n1", "   ")).is_err());
    }

    #[test]
    fn deleting_is_idempotent() {
        // So the UI does not have to guard against a double click.
        let dir = scratch("delete");
        let mut store = NotebookStore::load(&dir);
        store.save(notebook("n1", "One")).expect("save");
        store.delete("n1").expect("delete");
        store.delete("n1").expect("delete again");
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_failed_write_leaves_the_previous_file_intact() {
        // The same guarantee the connection store makes, for the same reason:
        // this is content nobody can reconstruct.
        let dir = scratch("atomic");
        let mut store = NotebookStore::load(&dir);
        store.save(notebook("n1", "Kept")).expect("save");
        let bytes = std::fs::read(dir.join(FILE_NAME)).expect("read");

        // A directory where the temp file wants to be makes the write fail.
        std::fs::create_dir_all(dir.join("notebooks.json.tmp")).expect("block");
        assert!(store.save(notebook("n2", "Doomed")).is_err());

        assert_eq!(std::fs::read(dir.join(FILE_NAME)).expect("read"), bytes);
    }
}
