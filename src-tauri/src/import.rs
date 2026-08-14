//! Running a SQL file against a connection.
//!
//! The file is read in chunks and split into statements as it arrives, so a
//! multi-gigabyte dump never exists in memory — only the statement currently
//! being assembled does. Statements reach the server while the rest of the file
//! is still being read.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tablex_core::{
    driver::FetchOptions,
    error::{Error, Result},
    sql::Splitter,
};

use crate::{export::Progress, state::AppState};

/// Bytes read per turn. Large enough to keep the round trips down, small enough
/// that progress moves while a big file is being read.
const CHUNK: usize = 64 * 1024;

pub struct ImportRequest {
    pub id: String,
    pub connection_id: String,
    pub path: String,
}

/// Apply every statement in a SQL file, returning how many were run.
pub async fn run(
    state: &AppState,
    request: ImportRequest,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(Progress) + Sync,
) -> Result<u64> {
    let file = std::fs::File::open(&request.path)
        .map_err(|e| Error::Io(format!("could not open {}: {}", request.path, e)))?;
    let total_bytes = file.metadata().ok().map(|m| m.len());
    let mut reader = std::io::BufReader::new(file);

    let label = std::path::Path::new(&request.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.path.clone());

    let session = state.sessions.get(&request.connection_id).await?;
    let mut guard = session.connection.lock().await;

    // Progress is measured in bytes read rather than statements applied: the
    // statement count is unknown until the end, and a bar needs a denominator.
    let report = |bytes: u64, done: bool| {
        on_progress(Progress {
            id: request.id.clone(),
            label: label.clone(),
            unit: "KB".into(),
            rows: bytes / 1024,
            total: total_bytes.map(|b| b / 1024),
            done,
        });
    };
    report(0, false);

    // Statements in a dump are DDL and INSERTs; a stray SELECT is capped rather
    // than materialized, since nothing here displays its rows.
    let opts = FetchOptions {
        max_rows: Some(1),
        offset: 0,
        timeout_secs: None,
    };

    let mut splitter = Splitter::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut leftover: Vec<u8> = Vec::new();
    let mut read_bytes = 0u64;
    let mut applied = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }

        let n = reader
            .read(&mut buffer)
            .map_err(|e| Error::Io(format!("could not read {}: {}", request.path, e)))?;
        if n == 0 {
            break;
        }
        read_bytes += n as u64;

        // A chunk boundary can fall inside a multi-byte character, so the tail
        // of an invalid sequence is carried forward rather than replaced with
        // U+FFFD — which would corrupt exactly the text an import is meant to
        // reproduce.
        leftover.extend_from_slice(&buffer[..n]);
        let text = match std::str::from_utf8(&leftover) {
            Ok(text) => {
                let owned = text.to_string();
                leftover.clear();
                owned
            }
            Err(e) => {
                let good = e.valid_up_to();
                let owned = String::from_utf8_lossy(&leftover[..good]).into_owned();
                leftover.drain(..good);
                owned
            }
        };

        for statement in splitter.push(&text) {
            apply(&mut **guard, &statement, &opts).await?;
            applied += 1;
        }
        report(read_bytes, false);
    }

    for statement in splitter.finish() {
        apply(&mut **guard, &statement, &opts).await?;
        applied += 1;
    }

    report(read_bytes, true);
    Ok(applied)
}

/// Run one statement, naming it if it fails.
///
/// A restore that stops at statement 40,000 with only the server's message is
/// nearly impossible to act on; the statement itself is what tells you whether
/// the dump is wrong, the target already has the table, or the file is for a
/// different engine entirely.
async fn apply(
    conn: &mut dyn tablex_core::driver::Connection,
    statement: &str,
    opts: &FetchOptions,
) -> Result<()> {
    conn.execute(statement, opts).await.map_err(|e| {
        let preview: String = statement.chars().take(200).collect();
        let ellipsis = if statement.chars().count() > 200 {
            "…"
        } else {
            ""
        };
        Error::Query {
            message: format!("{e}\n\nwhile running:\n{preview}{ellipsis}"),
            position: None,
            code: None,
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use indexmap::IndexMap;
    use std::sync::Mutex;
    use tablex_core::{
        config::TlsConfig,
        driver::{Connection, RowEdit},
        result::QueryOutcome,
        schema::{SchemaNode, TableDetail},
        ConnectionConfig,
    };

    /// Records every statement it is asked to run, and can be told to fail on
    /// one of them.
    struct Recorder {
        applied: Arc<Mutex<Vec<String>>>,
        fail_on: Option<&'static str>,
    }

    #[async_trait]
    impl Connection for Recorder {
        async fn execute(&mut self, sql: &str, _opts: &FetchOptions) -> Result<QueryOutcome> {
            if self.fail_on.is_some_and(|needle| sql.contains(needle)) {
                return Err(Error::query("syntax error at or near \"oops\""));
            }
            self.applied.lock().unwrap().push(sql.to_string());
            Ok(QueryOutcome {
                statements: vec![],
                elapsed_ms: 0,
                notices: vec![],
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

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-import-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    async fn state_with(
        dir: &std::path::Path,
        fail_on: Option<&'static str>,
    ) -> (AppState, Arc<Mutex<Vec<String>>>) {
        let state = AppState::new(dir);
        let applied = Arc::new(Mutex::new(Vec::new()));

        state.connections.lock().await.push(ConnectionConfig {
            id: "c1".into(),
            name: "test".into(),
            driver: "sqlite".into(),
            host: None,
            port: None,
            database: None,
            username: None,
            file_path: Some(":memory:".into()),
            tls: TlsConfig::default(),
            ssh: None,
            folder: None,
            color: None,
            read_only: false,
            options: IndexMap::new(),
        });

        state
            .sessions
            .insert(
                "c1",
                Box::new(Recorder {
                    applied: applied.clone(),
                    fail_on,
                }),
                None,
            )
            .await;

        (state, applied)
    }

    fn write(dir: &std::path::Path, name: &str, sql: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, sql).expect("write dump");
        path.to_string_lossy().into_owned()
    }

    fn request(path: String) -> ImportRequest {
        ImportRequest {
            id: "i1".into(),
            connection_id: "c1".into(),
            path,
        }
    }

    #[tokio::test]
    async fn applies_every_statement_in_order() {
        let dir = temp_dir("basic");
        let path = write(
            &dir,
            "dump.sql",
            "CREATE TABLE t (a TEXT);\n\
             INSERT INTO t VALUES ('a;b');\n\
             -- a comment; with a semicolon\n\
             INSERT INTO t VALUES ('c')",
        );
        let (state, applied) = state_with(&dir, None).await;

        let count = run(
            &state,
            request(path),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect("import");

        let applied = applied.lock().unwrap().clone();
        assert_eq!(count, 3, "{applied:?}");
        // The semicolon inside the string did not split the statement, and the
        // last statement arrived despite having no terminator.
        assert!(applied[1].contains("'a;b'"), "{applied:?}");
        assert!(applied[2].contains("'c'"), "{applied:?}");
    }

    #[tokio::test]
    async fn a_failing_statement_is_named_in_the_error() {
        let dir = temp_dir("failure");
        let path = write(
            &dir,
            "dump.sql",
            "INSERT INTO t VALUES (1);\nINSERT INTO oops VALUES (2);\nINSERT INTO t VALUES (3);",
        );
        let (state, applied) = state_with(&dir, Some("oops")).await;

        let err = run(
            &state,
            request(path),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .expect_err("the second statement fails");

        // A restore that stops at statement 40,000 with only the server's
        // message is nearly impossible to act on.
        let text = err.to_string();
        assert!(text.contains("oops"), "{text}");
        assert!(text.contains("while running"), "{text}");
        // And it stopped there rather than carrying on into the next statement.
        assert_eq!(applied.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancelling_stops_partway_through() {
        let dir = temp_dir("cancel");
        let mut sql = String::new();
        for i in 0..5_000 {
            sql.push_str(&format!("INSERT INTO t VALUES ({i});\n"));
        }
        let path = write(&dir, "dump.sql", &sql);
        let (state, applied) = state_with(&dir, None).await;

        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let err = run(&state, request(path), cancel, move |p| {
            if p.rows > 0 {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .await
        .expect_err("cancelled");

        assert!(matches!(err, Error::Cancelled), "{err}");
        // Statements already applied stay applied: a half-run script cannot be
        // taken back, and pretending otherwise would be worse than saying so.
        let count = applied.lock().unwrap().len();
        assert!(count < 5_000, "applied {count} of 5000");
    }

    #[tokio::test]
    async fn progress_is_measured_in_bytes_read() {
        let dir = temp_dir("progress");
        let path = write(&dir, "dump.sql", "SELECT 1;\nSELECT 2;");
        let (state, _) = state_with(&dir, None).await;
        let seen = Arc::new(Mutex::new(Vec::new()));

        let collector = seen.clone();
        run(
            &state,
            request(path),
            Arc::new(AtomicBool::new(false)),
            move |p| collector.lock().unwrap().push((p.unit.clone(), p.done)),
        )
        .await
        .expect("import");

        let reports = seen.lock().unwrap().clone();
        // The statement count is unknown until the end, so the bar counts what
        // it can: how much of the file has been read.
        assert!(reports.iter().all(|(unit, _)| unit == "KB"), "{reports:?}");
        assert_eq!(reports.first().map(|r| r.1), Some(false));
        assert_eq!(reports.last().map(|r| r.1), Some(true));
    }
}
