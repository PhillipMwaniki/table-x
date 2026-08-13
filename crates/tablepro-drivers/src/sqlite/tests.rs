//! Integration tests against a real in-memory SQLite database.
//!
//! These exercise the driver end to end rather than mocking rusqlite: the whole
//! point of the driver is faithful behaviour against a real engine.

use super::*;
use indexmap::IndexMap;
use tablepro_core::{config::TlsConfig, Value};

fn config() -> ConnectionConfig {
    ConnectionConfig {
        id: "test".into(),
        name: "test".into(),
        driver: "sqlite".into(),
        host: None,
        port: None,
        database: None,
        username: None,
        file_path: Some(":memory:".into()),
        tls: TlsConfig::default(),
        ssh: None,
        color: None,
        read_only: false,
        options: IndexMap::new(),
    }
}

async fn connect() -> Box<dyn Connection> {
    SqliteDriver::new()
        .connect(&config(), None)
        .await
        .expect("connect to in-memory sqlite")
}

/// Run SQL and return the first result set, failing if it was not a row-returning
/// statement.
async fn query(conn: &mut Box<dyn Connection>, sql: &str) -> ResultSet {
    let out = conn
        .execute(sql, &FetchOptions::default())
        .await
        .unwrap_or_else(|e| panic!("query failed: {sql}\n{e}"));
    match out.statements.into_iter().next().expect("one statement") {
        StatementResult::Rows(rs) => rs,
        other => panic!("expected rows, got {other:?}"),
    }
}

async fn exec(conn: &mut Box<dyn Connection>, sql: &str) {
    conn.execute(sql, &FetchOptions::default())
        .await
        .unwrap_or_else(|e| panic!("exec failed: {sql}\n{e}"));
}

async fn seeded() -> Box<dyn Connection> {
    let mut conn = connect().await;
    exec(
        &mut conn,
        "CREATE TABLE users (
            id      INTEGER PRIMARY KEY,
            email   TEXT NOT NULL UNIQUE,
            active  BOOLEAN NOT NULL DEFAULT 1,
            balance DECIMAL(20,10),
            created DATETIME
        )",
    )
    .await;
    exec(
        &mut conn,
        "INSERT INTO users (id, email, active, balance, created) VALUES
            (1, 'a@example.com', 1, '123456789012345678.1234567890', '2026-08-13 11:30:00'),
            (2, 'b@example.com', 0, '0.5', '2026-08-13 12:00:00')",
    )
    .await;
    conn
}

#[tokio::test]
async fn executes_a_query_and_decodes_by_declared_type() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT id, email, active FROM users ORDER BY id").await;

    assert_eq!(rs.columns.len(), 3);
    assert_eq!(rs.rows.len(), 2);
    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("a@example.com".into()));
    // Declared BOOLEAN, stored as INTEGER — must come back as a bool.
    assert_eq!(rs.rows[0][2], Value::Bool(true));
    assert_eq!(rs.rows[1][2], Value::Bool(false));
}

#[tokio::test]
async fn decimal_affinity_coercion_happens_inside_sqlite_and_is_reported_faithfully() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT balance FROM users WHERE id = 1").await;

    // A SQLite gotcha worth pinning down: `DECIMAL(20,10)` carries NUMERIC
    // affinity, so SQLite converts the inserted text to a float *at write time*
    // and loses digits before the driver ever sees it. That precision is gone in
    // the file, not in our decoding — and the driver must not pretend otherwise
    // by fabricating the digits back.
    //
    // What the driver does guarantee: the column still reports as exact-numeric
    // rather than as a plain integer, so the grid treats it consistently.
    assert_eq!(rs.rows[0][0], Value::Numeric("123456789012345680".into()));
    assert!(rs.rows[0][0].is_numeric());
}

#[tokio::test]
async fn exact_decimals_survive_when_the_column_has_text_affinity() {
    // The practical consequence of the test above: storing exact decimals in
    // SQLite requires a TEXT-affinity column. This is what ORMs do for money.
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE ledger (amount TEXT NOT NULL)").await;
    let exact = "123456789012345678.1234567890";
    exec(
        &mut conn,
        &format!("INSERT INTO ledger (amount) VALUES ('{exact}')"),
    )
    .await;

    let rs = query(&mut conn, "SELECT amount FROM ledger").await;
    // Every digit intact — no f64 anywhere in the path.
    assert_eq!(rs.rows[0][0], Value::Text(exact.into()));
    assert_eq!(rs.rows[0][0].to_string(), exact);
}

#[tokio::test]
async fn null_is_preserved_as_null() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "INSERT INTO users (id, email, balance) VALUES (3, 'c@example.com', NULL)",
    )
    .await;
    let rs = query(&mut conn, "SELECT balance FROM users WHERE id = 3").await;
    assert_eq!(rs.rows[0][0], Value::Null);
    assert!(rs.rows[0][0].is_null());
}

#[tokio::test]
async fn row_cap_truncates_and_says_so() {
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE nums (n INTEGER)").await;
    exec(
        &mut conn,
        "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 500) \
         INSERT INTO nums SELECT n FROM seq",
    )
    .await;

    let opts = FetchOptions {
        max_rows: Some(10),
        offset: 0,
        timeout_secs: None,
    };
    let out = conn
        .execute("SELECT n FROM nums ORDER BY n", &opts)
        .await
        .expect("query");
    match &out.statements[0] {
        StatementResult::Rows(rs) => {
            assert_eq!(rs.rows.len(), 10);
            // Without this flag the UI would present a partial result as complete.
            assert!(rs.truncated, "capped result must be marked truncated");
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
async fn a_complete_result_is_not_marked_truncated() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT id FROM users").await;
    assert_eq!(rs.rows.len(), 2);
    assert!(!rs.truncated);
}

#[tokio::test]
async fn offset_skips_rows() {
    let mut conn = seeded().await;
    let opts = FetchOptions {
        max_rows: Some(10),
        offset: 1,
        timeout_secs: None,
    };
    let out = conn
        .execute("SELECT id FROM users ORDER BY id", &opts)
        .await
        .expect("query");
    match &out.statements[0] {
        StatementResult::Rows(rs) => {
            assert_eq!(rs.rows.len(), 1);
            assert_eq!(rs.rows[0][0], Value::Int(2));
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
async fn writes_report_affected_rows_not_an_empty_result_set() {
    let mut conn = seeded().await;
    let out = conn
        .execute(
            "UPDATE users SET active = 1 WHERE id = 2",
            &FetchOptions::default(),
        )
        .await
        .expect("update");
    match &out.statements[0] {
        StatementResult::Affected { rows_affected, .. } => assert_eq!(*rows_affected, 1),
        other => panic!("expected affected count, got {other:?}"),
    }
}

#[tokio::test]
async fn multiple_statements_run_in_order() {
    let mut conn = connect().await;
    let out = conn
        .execute(
            "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (7); SELECT a FROM t",
            &FetchOptions::default(),
        )
        .await
        .expect("batch");
    assert_eq!(out.statements.len(), 3);
    match &out.statements[2] {
        StatementResult::Rows(rs) => assert_eq!(rs.rows[0][0], Value::Int(7)),
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
async fn a_semicolon_inside_a_string_does_not_split_the_statement() {
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE t (s TEXT)").await;
    // If the splitter were naive, this would execute as two broken statements.
    exec(&mut conn, "INSERT INTO t VALUES ('a;b')").await;
    let rs = query(&mut conn, "SELECT s FROM t").await;
    assert_eq!(rs.rows[0][0], Value::Text("a;b".into()));
}

#[tokio::test]
async fn syntax_errors_are_categorized_as_query_errors() {
    let mut conn = connect().await;
    let err = conn
        .execute("SLECT 1", &FetchOptions::default())
        .await
        .expect_err("must fail");
    assert_eq!(err.category(), tablepro_core::ErrorCategory::Query);
    // Retrying identical broken SQL is pointless; the UI must not offer it.
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn browse_lists_tables_then_columns() {
    let mut conn = seeded().await;

    let roots = conn.browse(None).await.expect("browse roots");
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].name, "users");
    assert!(roots[0].expandable);
    // Children are not fetched until asked for — the tree is lazy.
    assert!(roots[0].children.is_none());

    let cols = conn.browse(Some("users")).await.expect("browse columns");
    let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "email", "active", "balance", "created"]);
    assert!(cols.iter().all(|c| !c.expandable));
}

#[tokio::test]
async fn internal_sqlite_tables_are_hidden() {
    let mut conn = connect().await;
    // AUTOINCREMENT creates sqlite_sequence, which users should never see.
    exec(
        &mut conn,
        "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT)",
    )
    .await;
    exec(&mut conn, "INSERT INTO t DEFAULT VALUES").await;

    let roots = conn.browse(None).await.expect("browse");
    assert!(
        roots.iter().all(|n| !n.name.starts_with("sqlite_")),
        "got {:?}",
        roots.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn table_detail_reports_key_index_and_foreign_key() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TABLE orders (
            id      INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE
        )",
    )
    .await;

    let detail = conn.table_detail(None, "orders").await.expect("detail");
    assert_eq!(detail.name, "orders");
    assert_eq!(detail.primary_key, vec!["id".to_string()]);
    assert_eq!(detail.columns.len(), 2);

    let fk = detail.foreign_keys.first().expect("one foreign key");
    assert_eq!(fk.referenced_table, "users");
    assert_eq!(fk.columns, vec!["user_id".to_string()]);
    assert_eq!(fk.referenced_columns, vec!["id".to_string()]);
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));

    // edit_key comes from the primary key.
    assert_eq!(detail.edit_key(), vec!["id".to_string()]);
}

#[tokio::test]
async fn table_without_a_primary_key_falls_back_to_a_unique_index() {
    let mut conn = seeded().await;
    let detail = conn.table_detail(None, "users").await.expect("detail");
    // users has an explicit PK, so that wins.
    assert_eq!(detail.edit_key(), vec!["id".to_string()]);

    exec(
        &mut conn,
        "CREATE TABLE tags (slug TEXT NOT NULL UNIQUE, label TEXT)",
    )
    .await;
    let detail = conn.table_detail(None, "tags").await.expect("detail");
    assert!(detail.primary_key.is_empty());
    assert_eq!(detail.edit_key(), vec!["slug".to_string()]);
}

#[tokio::test]
async fn apply_edit_updates_exactly_one_row() {
    let mut conn = seeded().await;

    conn.apply_edit(&RowEdit {
        schema: None,
        table: "users".into(),
        changes: vec![("email".into(), Value::Text("new@example.com".into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit applies");

    let rs = query(&mut conn, "SELECT email FROM users WHERE id = 1").await;
    assert_eq!(rs.rows[0][0], Value::Text("new@example.com".into()));

    // The other row is untouched.
    let rs = query(&mut conn, "SELECT email FROM users WHERE id = 2").await;
    assert_eq!(rs.rows[0][0], Value::Text("b@example.com".into()));
}

#[tokio::test]
async fn apply_edit_rolls_back_when_the_key_matches_no_row() {
    let mut conn = seeded().await;

    let err = conn
        .apply_edit(&RowEdit {
            schema: None,
            table: "users".into(),
            changes: vec![("email".into(), Value::Text("ghost@example.com".into()))],
            // This row does not exist — it may have been deleted since load.
            key: vec![("id".into(), Value::Int(999))],
        })
        .await
        .expect_err("must not silently succeed");
    assert!(err.to_string().contains("expected exactly 1"), "{err}");

    // Nothing was written.
    let rs = query(
        &mut conn,
        "SELECT count(*) FROM users WHERE email LIKE 'ghost%'",
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Int(0));
}

#[tokio::test]
async fn apply_edit_rolls_back_when_the_key_matches_many_rows() {
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE t (grp INTEGER, note TEXT)").await;
    exec(
        &mut conn,
        "INSERT INTO t VALUES (1, 'one'), (1, 'two'), (2, 'three')",
    )
    .await;

    // `grp` is not unique: this WHERE clause matches two rows. Committing would
    // silently overwrite a row the user never edited.
    let err = conn
        .apply_edit(&RowEdit {
            schema: None,
            table: "t".into(),
            changes: vec![("note".into(), Value::Text("clobbered".into()))],
            key: vec![("grp".into(), Value::Int(1))],
        })
        .await
        .expect_err("must refuse a non-unique key");
    assert!(err.to_string().contains("expected exactly 1"), "{err}");

    let rs = query(&mut conn, "SELECT count(*) FROM t WHERE note = 'clobbered'").await;
    assert_eq!(rs.rows[0][0], Value::Int(0), "rollback must leave no trace");
}

#[tokio::test]
async fn apply_edit_refuses_a_row_with_no_key() {
    let mut conn = seeded().await;
    let err = conn
        .apply_edit(&RowEdit {
            schema: None,
            table: "users".into(),
            changes: vec![("email".into(), Value::Text("x@example.com".into()))],
            key: vec![],
        })
        .await
        .expect_err("must refuse");
    // An empty WHERE clause would update every row in the table.
    assert_eq!(err.category(), tablepro_core::ErrorCategory::Unsupported);
}

#[tokio::test]
async fn apply_edit_matches_null_keys_with_is_null() {
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE t (k TEXT, v TEXT)").await;
    exec(&mut conn, "INSERT INTO t VALUES (NULL, 'before')").await;

    // `k = NULL` is never true in SQL; only `k IS NULL` matches.
    conn.apply_edit(&RowEdit {
        schema: None,
        table: "t".into(),
        changes: vec![("v".into(), Value::Text("after".into()))],
        key: vec![("k".into(), Value::Null)],
    })
    .await
    .expect("NULL key must match via IS NULL");

    let rs = query(&mut conn, "SELECT v FROM t").await;
    assert_eq!(rs.rows[0][0], Value::Text("after".into()));
}

#[tokio::test]
async fn arbitrary_query_results_are_not_editable() {
    let mut conn = seeded().await;
    // SQLite gives no column provenance, so the grid must stay read-only for
    // ad-hoc SQL rather than guessing a target table.
    let rs = query(&mut conn, "SELECT id, email FROM users").await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id))",
    )
    .await;

    // SQLite defaults foreign_keys OFF; the driver turns them on at connect.
    let err = conn
        .execute(
            "INSERT INTO orders (id, user_id) VALUES (1, 999)",
            &FetchOptions::default(),
        )
        .await
        .expect_err("foreign key must be enforced");
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "{err}"
    );
}

#[tokio::test]
async fn completion_scope_lists_tables_with_their_columns() {
    let mut conn = seeded().await;
    let scope = conn.completion_scope().await.expect("scope");

    let (table, columns) = scope
        .tables
        .iter()
        .find(|(t, _)| t == "users")
        .expect("users in completion scope");
    assert_eq!(table, "users");
    assert!(columns.contains(&"email".to_string()));
    assert!(!scope.functions.is_empty());
}

#[tokio::test]
async fn ping_succeeds_on_a_live_connection() {
    let mut conn = connect().await;
    conn.ping().await.expect("ping");
}

#[tokio::test]
async fn read_only_connections_reject_writes() {
    // Opening :memory: read-only yields an empty database that cannot be written.
    let mut cfg = config();
    cfg.read_only = true;
    let mut conn = SqliteDriver::new()
        .connect(&cfg, None)
        .await
        .expect("connect read-only");

    let err = conn
        .execute("CREATE TABLE t (a INTEGER)", &FetchOptions::default())
        .await
        .expect_err("write must be rejected");
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn missing_file_path_is_a_config_error() {
    let mut cfg = config();
    cfg.file_path = None;
    // `Box<dyn Connection>` is not Debug, so `expect_err` is unavailable here.
    match SqliteDriver::new().connect(&cfg, None).await {
        Err(e) => assert_eq!(e.category(), tablepro_core::ErrorCategory::Config),
        Ok(_) => panic!("connecting without a file path must fail"),
    }
}

#[test]
fn driver_advertises_only_what_it_implements() {
    let info = SqliteDriver::new().info();
    assert_eq!(info.id, "sqlite");
    assert!(info.file_based);
    assert!(info.capabilities.transactions);
    // Deliberately unsupported — see the comment on `info()`.
    assert!(!info.capabilities.column_provenance);
    assert!(!info.capabilities.schemas);
    assert!(!info.capabilities.cancel);
}
