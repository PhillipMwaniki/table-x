//! Persisted, searchable query history.
//!
//! Every statement submitted from the editor is appended here so it can be found
//! again later, together with how long it took and whether it worked.
//!
//! **Why JSON Lines rather than one JSON document.** Recording a query is the
//! most frequent write this app makes. Appending a line is a single small write
//! regardless of how much history exists; rewriting a growing JSON array would
//! make every query cost more than the one before it. The file stays
//! human-readable and greppable, which matches how `connections.json` is meant
//! to be treated.
//!
//! **Why not a second SQLite database with an FTS index.** History is capped at
//! [`MAX_ENTRIES`], and matching a few thousand short strings takes well under a
//! millisecond — far less than the schema, migration, and locking surface an
//! embedded index would add to a feature whose whole job is to be unobtrusive.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use tablex_core::error::{Error, Result};

const FILE_NAME: &str = "history.jsonl";

/// How many entries are retained. Old ones fall off the end.
///
/// A cap exists so the file cannot grow without bound on a machine that is never
/// tidied up; 5,000 entries is roughly a year of heavy use and still loads in
/// a few milliseconds.
pub const MAX_ENTRIES: usize = 5_000;

/// Rewrite the file once it holds this many lines. Appending is cheap, but a
/// file of nothing but superseded lines is not, so it is compacted in bulk
/// rather than on every write.
const COMPACT_AT: usize = MAX_ENTRIES * 2;

/// Rows and results returned per search when the caller does not say.
const DEFAULT_LIMIT: usize = 200;

/// One executed submission.
///
/// The connection's name and driver are copied in rather than referenced by id:
/// history outlives the connection it ran against, and an entry that renders as
/// "unknown connection" after a cleanup is worth much less than one that still
/// says where it ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub connection_id: String,
    pub connection_name: String,
    pub driver: String,
    pub sql: String,
    /// RFC 3339, UTC. A string because it crosses to the frontend, which formats
    /// it in the user's locale.
    pub ran_at: String,
    pub elapsed_ms: u64,
    /// Rows returned or affected, summed across statements. `None` when the
    /// submission failed and no count is meaningful.
    #[serde(default)]
    pub rows: Option<u64>,
    pub succeeded: bool,
    /// The error message, so a failed query can be found by what went wrong.
    #[serde(default)]
    pub error: Option<String>,
}

/// A search over the history.
#[derive(Debug, Default, Deserialize)]
pub struct HistoryQuery {
    /// Restrict to one connection. `None` searches every connection.
    #[serde(default)]
    pub connection_id: Option<String>,
    /// Whitespace-separated terms; an entry must contain all of them.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub struct QueryHistory {
    path: PathBuf,
    /// Oldest first, so pushing is appending and trimming is from the front.
    entries: Vec<HistoryEntry>,
    /// Lines currently in the file, including ones already trimmed from memory.
    /// Drives compaction.
    lines_on_disk: usize,
}

impl QueryHistory {
    /// Load whatever is on disk. A missing file is the normal first run.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).unwrap_or_default();

        let mut entries = Vec::new();
        let mut lines_on_disk = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            lines_on_disk += 1;
            match serde_json::from_str::<HistoryEntry>(line) {
                Ok(entry) => entries.push(entry),
                // Unlike connections.json, a damaged line here costs one
                // remembered query, not a credential the user cannot recreate.
                // Skipping it and keeping the rest is the useful behaviour; a
                // hard failure would throw away a working history over one bad
                // line, most likely written by a crash mid-append.
                Err(e) => tracing::warn!("skipping unreadable history line: {e}"),
            }
        }

        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }

        QueryHistory {
            path,
            entries,
            lines_on_disk,
        }
    }

    /// Append one entry.
    ///
    /// Errors are returned rather than swallowed so the caller can log them, but
    /// callers should not fail a query because its history line could not be
    /// written — the result the user asked for still arrived.
    pub fn record(&mut self, entry: HistoryEntry) -> Result<()> {
        let line = serde_json::to_string(&entry)?;
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }

        if self.lines_on_disk >= COMPACT_AT {
            // The new entry is already in memory, so a rewrite persists it too.
            return self.rewrite();
        }

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| Error::Io(e.to_string()))?;
        writeln!(file, "{line}").map_err(|e| Error::Io(e.to_string()))?;
        self.lines_on_disk += 1;
        Ok(())
    }

    /// Matching entries, newest first.
    pub fn search(&self, query: &HistoryQuery) -> Vec<HistoryEntry> {
        let terms: Vec<String> = query
            .text
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .collect();
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT);

        self.entries
            .iter()
            .rev()
            .filter(|e| match &query.connection_id {
                Some(id) => &e.connection_id == id,
                None => true,
            })
            .filter(|e| matches(e, &terms))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Forget history for one connection, or all of it.
    pub fn clear(&mut self, connection_id: Option<&str>) -> Result<()> {
        match connection_id {
            Some(id) => self.entries.retain(|e| e.connection_id != id),
            None => self.entries.clear(),
        }
        self.rewrite()
    }

    /// Replace the file with exactly what is in memory.
    ///
    /// Temp-then-rename, like the connection store: a crash midway leaves the
    /// previous history intact rather than a half-written file that would lose
    /// every entry.
    fn rewrite(&mut self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }

        let mut body = String::new();
        for entry in &self.entries {
            body.push_str(&serde_json::to_string(entry)?);
            body.push('\n');
        }

        let tmp = self.path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, body.as_bytes()).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::Io(e.to_string()))?;
        self.lines_on_disk = self.entries.len();
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Whether an entry contains every search term.
///
/// Terms are matched against the statement, the connection name, and the error
/// message, so "prod timeout" finds a failed query without the user remembering
/// its text. Matching is substring rather than word-boundary because SQL is full
/// of punctuation: `users.id` should be found by `users`.
fn matches(entry: &HistoryEntry, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {}",
        entry.sql.to_lowercase(),
        entry.connection_name.to_lowercase(),
        entry.error.as_deref().unwrap_or_default().to_lowercase(),
    );
    terms.iter().all(|t| haystack.contains(t.as_str()))
}

/// Whether a statement assigns a credential, and so must not be written to a
/// plaintext file.
///
/// History is stored unencrypted by design — it is meant to be greppable — so a
/// statement like `ALTER USER app PASSWORD 'hunter2'` would leak a live password
/// onto disk in a file nothing else protects. Such statements are dropped
/// entirely rather than redacted: a partial redaction that misses one vendor's
/// syntax leaks the secret anyway, and there is no way to be sure the list of
/// syntaxes is complete.
///
/// The test is deliberately narrow — a literal must actually follow the keyword.
/// `SELECT password_hash FROM users` is a normal query and stays in history.
pub fn assigns_a_credential(sql: &str) -> bool {
    let lower = sql.to_lowercase();

    if lower.contains("identified by") || lower.contains("identified with") {
        return true;
    }

    // `password 'x'`, `password = 'x'`, `password='x'` — but not `password` used
    // as an ordinary column or table name.
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("password") {
        let after = rest[at + "password".len()..].trim_start();
        let after = after.strip_prefix('=').unwrap_or(after).trim_start();
        if after.starts_with('\'') || after.starts_with('"') || after.starts_with('$') {
            return true;
        }
        rest = &rest[at + "password".len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-history-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn entry(sql: &str) -> HistoryEntry {
        HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            connection_id: "conn-1".into(),
            connection_name: "Prod".into(),
            driver: "postgres".into(),
            sql: sql.into(),
            ran_at: "2026-01-01T00:00:00Z".into(),
            elapsed_ms: 3,
            rows: Some(1),
            succeeded: true,
            error: None,
        }
    }

    #[test]
    fn missing_file_is_an_empty_history_not_an_error() {
        let history = QueryHistory::load(&temp_dir("missing"));
        assert!(history.search(&HistoryQuery::default()).is_empty());
    }

    #[test]
    fn entries_survive_a_reload() {
        let dir = temp_dir("reload");
        let mut history = QueryHistory::load(&dir);
        history.record(entry("select 1")).expect("record");
        history.record(entry("select 2")).expect("record");

        let reloaded = QueryHistory::load(&dir);
        let found = reloaded.search(&HistoryQuery::default());
        assert_eq!(found.len(), 2);
        // Newest first: the panel shows the most recent query at the top.
        assert_eq!(found[0].sql, "select 2");
    }

    #[test]
    fn search_requires_every_term() {
        let mut history = QueryHistory::load(&temp_dir("terms"));
        history.record(entry("SELECT * FROM orders")).expect("a");
        history.record(entry("SELECT * FROM users")).expect("b");

        let query = HistoryQuery {
            text: Some("select users".into()),
            ..Default::default()
        };
        let found = history.search(&query);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].sql.contains("users"));
    }

    #[test]
    fn search_is_case_insensitive_and_matches_inside_identifiers() {
        let mut history = QueryHistory::load(&temp_dir("case"));
        history
            .record(entry("SELECT users.id FROM users"))
            .expect("a");

        // A user searching "users" must find `users.id`; word-boundary matching
        // would miss it, and SQL is mostly punctuation.
        let query = HistoryQuery {
            text: Some("USERS.ID".into()),
            ..Default::default()
        };
        assert_eq!(history.search(&query).len(), 1);
    }

    #[test]
    fn a_failed_query_is_findable_by_its_error() {
        let mut history = QueryHistory::load(&temp_dir("errors"));
        let mut failed = entry("SELECT * FROM nope");
        failed.succeeded = false;
        failed.rows = None;
        failed.error = Some("relation \"nope\" does not exist".into());
        history.record(failed).expect("record");

        let query = HistoryQuery {
            text: Some("does not exist".into()),
            ..Default::default()
        };
        assert_eq!(history.search(&query).len(), 1);
    }

    #[test]
    fn search_can_be_limited_to_one_connection() {
        let mut history = QueryHistory::load(&temp_dir("filter"));
        history.record(entry("select 1")).expect("a");
        let mut other = entry("select 2");
        other.connection_id = "conn-2".into();
        history.record(other).expect("b");

        let query = HistoryQuery {
            connection_id: Some("conn-2".into()),
            ..Default::default()
        };
        let found = history.search(&query);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sql, "select 2");
    }

    #[test]
    fn clearing_one_connection_leaves_the_others() {
        let dir = temp_dir("clear-one");
        let mut history = QueryHistory::load(&dir);
        history.record(entry("select 1")).expect("a");
        let mut other = entry("select 2");
        other.connection_id = "conn-2".into();
        history.record(other).expect("b");

        history.clear(Some("conn-1")).expect("clear");
        assert_eq!(history.search(&HistoryQuery::default()).len(), 1);

        // And the removal is on disk, not just in memory.
        let reloaded = QueryHistory::load(&dir);
        assert_eq!(reloaded.search(&HistoryQuery::default()).len(), 1);
    }

    #[test]
    fn a_corrupt_line_costs_only_that_entry() {
        let dir = temp_dir("corrupt");
        let mut history = QueryHistory::load(&dir);
        history.record(entry("select 1")).expect("record");
        // Simulate a crash mid-append leaving a truncated line.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(history.path())
            .expect("open");
        writeln!(file, "{{\"id\":\"broken\"").expect("write");
        drop(file);

        let reloaded = QueryHistory::load(&dir);
        assert_eq!(reloaded.search(&HistoryQuery::default()).len(), 1);
    }

    #[test]
    fn history_is_capped_and_drops_the_oldest_first() {
        let dir = temp_dir("cap");
        let mut history = QueryHistory::load(&dir);
        for i in 0..MAX_ENTRIES + 5 {
            history
                .record(entry(&format!("select {i}")))
                .expect("record");
        }

        let found = history.search(&HistoryQuery {
            limit: Some(usize::MAX),
            ..Default::default()
        });
        assert_eq!(found.len(), MAX_ENTRIES);
        assert_eq!(found[0].sql, format!("select {}", MAX_ENTRIES + 4));
        // The oldest entries are gone rather than lingering.
        assert!(!found.iter().any(|e| e.sql == "select 0"));
    }

    #[test]
    fn credential_statements_are_recognised() {
        assert!(assigns_a_credential(
            "ALTER USER app WITH PASSWORD 'hunter2'"
        ));
        assert!(assigns_a_credential("CREATE ROLE r PASSWORD='x'"));
        assert!(assigns_a_credential(
            "CREATE USER 'a'@'%' IDENTIFIED BY 'hunter2'"
        ));
        assert!(assigns_a_credential(
            "ALTER USER a IDENTIFIED WITH mysql_native_password BY 'x'"
        ));
    }

    #[test]
    fn ordinary_queries_mentioning_passwords_are_still_recorded() {
        // Dropping these would make history quietly incomplete for anyone whose
        // schema happens to have a password column — a far more common case than
        // rotating a credential from the editor.
        assert!(!assigns_a_credential("SELECT password_hash FROM users"));
        assert!(!assigns_a_credential(
            "ALTER TABLE users DROP COLUMN password"
        ));
    }
}
