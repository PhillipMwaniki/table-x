//! Integration tests against a real in-memory SQLite database.
//!
//! These exercise the driver end to end rather than mocking rusqlite: the whole
//! point of the driver is faithful behaviour against a real engine.

use super::*;
use indexmap::IndexMap;
use tablex_core::{config::TlsConfig, Value};

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
        folder: None,
        color: None,
        read_only: false,
        confirm_destructive: None,
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
    assert_eq!(err.category(), tablex_core::ErrorCategory::Query);
    // Retrying identical broken SQL is pointless; the UI must not offer it.
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn browse_starts_at_folders_then_objects_then_columns() {
    let mut conn = seeded().await;

    let roots = conn.browse(None).await.expect("browse roots");
    let folders: Vec<_> = roots.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(folders, vec!["Tables", "Views", "Triggers", "Indexes"]);
    // Children are not fetched until asked for — the tree is lazy.
    assert!(roots.iter().all(|n| n.children.is_none()));

    let tables = conn
        .browse(Some(&roots[0].id))
        .await
        .expect("browse tables");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].name, "users");
    // The name SQL should use travels with the node, so the UI never has to
    // know this engine's quoting rules.
    assert_eq!(tables[0].qualified.as_deref(), Some("\"users\""));

    let cols = conn
        .browse(Some(&tables[0].id))
        .await
        .expect("browse columns");
    let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "email", "active", "balance", "created"]);
    assert!(cols.iter().all(|c| !c.expandable));
}

#[tokio::test]
async fn views_and_triggers_are_listed_under_their_own_folders() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE VIEW active_users AS SELECT id FROM users WHERE active = 1",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TRIGGER users_touch AFTER UPDATE ON users BEGIN SELECT 1; END",
    )
    .await;

    let roots = conn.browse(None).await.expect("browse roots");
    let folder = |name: &str| {
        roots
            .iter()
            .find(|n| n.name == name)
            .expect(name)
            .id
            .clone()
    };

    let views = conn.browse(Some(&folder("Views"))).await.expect("views");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name, "active_users");

    let triggers = conn
        .browse(Some(&folder("Triggers")))
        .await
        .expect("triggers");
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].name, "users_touch");
    // A trigger is only meaningful against the table it fires on, so the list
    // says which one rather than leaving it to be looked up.
    assert_eq!(triggers[0].detail.as_deref(), Some("on users"));
    assert!(!triggers[0].expandable, "a trigger has no columns to show");

    // A table's own folder must not also contain the view.
    let tables = conn.browse(Some(&folder("Tables"))).await.expect("tables");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].name, "users");
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
    let tables_folder = &roots[0].id;
    let tables = conn
        .browse(Some(tables_folder))
        .await
        .expect("browse tables");
    assert!(
        tables.iter().all(|n| !n.name.starts_with("sqlite_")),
        "got {:?}",
        tables.iter().map(|n| &n.name).collect::<Vec<_>>()
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
    assert_eq!(err.category(), tablex_core::ErrorCategory::Unsupported);
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
async fn a_plain_table_query_is_editable() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT id, email FROM users").await;

    assert!(rs.editable);
    assert_eq!(rs.key_columns, vec!["id".to_string()]);
    let source = rs.columns[1].source.as_ref().expect("email has an origin");
    assert_eq!(source.table, "users");
    assert_eq!(source.column, "email");
}

#[tokio::test]
async fn an_alias_does_not_hide_the_underlying_column() {
    let mut conn = seeded().await;
    // The label changes; the origin does not. Without this the UPDATE would set
    // a column called `address`, which does not exist.
    let rs = query(&mut conn, "SELECT id, email AS address FROM users").await;

    assert!(rs.editable);
    assert_eq!(rs.columns[1].name, "address");
    assert_eq!(
        rs.columns[1].source.as_ref().expect("origin").column,
        "email"
    );
}

#[tokio::test]
async fn computed_columns_carry_no_origin() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT id, email, length(email) AS n FROM users").await;

    // The stored columns stay editable; the expression is not something an
    // UPDATE could ever target, and SQLite says so by reporting no origin.
    assert!(rs.editable);
    assert!(rs.columns[2].source.is_none());
}

#[tokio::test]
async fn a_join_is_not_editable() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id))",
    )
    .await;

    // Two source tables means no single target for an UPDATE, so any edit would
    // be a guess about which row the user meant.
    let rs = query(
        &mut conn,
        "SELECT users.id, users.email, orders.id FROM users JOIN orders ON orders.user_id = users.id",
    )
    .await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn an_aggregate_is_not_editable() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT count(*) AS n FROM users").await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn a_projection_without_the_key_is_not_editable() {
    let mut conn = seeded().await;
    // The WHERE clause is built from key values the user can see. `id` was not
    // selected, so there is nothing to address the row with.
    let rs = query(&mut conn, "SELECT email FROM users").await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn a_key_hidden_behind_an_alias_is_not_usable() {
    let mut conn = seeded().await;
    // `id` is in the projection but under another label, and the grid looks the
    // key up by the name it displays. Treating `ident` as the key would build a
    // WHERE clause naming a column the table does not have.
    let rs = query(&mut conn, "SELECT id AS ident, email FROM users").await;
    assert!(!rs.editable);
}

#[tokio::test]
async fn a_table_without_a_primary_key_is_not_editable() {
    let mut conn = seeded().await;
    exec(&mut conn, "CREATE TABLE notes (body TEXT)").await;
    exec(&mut conn, "INSERT INTO notes (body) VALUES ('hello')").await;

    // SQLite would still have a rowid here, but it is not in the projection.
    // Selecting it behind the user's back would key an UPDATE on a value they
    // never saw and cannot check.
    let rs = query(&mut conn, "SELECT body FROM notes").await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn a_query_result_carries_everything_an_edit_needs() {
    let mut conn = seeded().await;
    let rs = query(&mut conn, "SELECT id, email FROM users ORDER BY id").await;

    // Build the edit the way the grid does: the SET column from the origin, the
    // WHERE values from the row on screen.
    let source = rs.columns[1].source.clone().expect("origin");
    let key_index = rs
        .columns
        .iter()
        .position(|c| c.name == rs.key_columns[0])
        .expect("key column is displayed");

    conn.apply_edit(&RowEdit {
        schema: source.schema,
        table: source.table,
        changes: vec![("email".into(), Value::Text("edited@example.com".into()))],
        key: vec![(rs.key_columns[0].clone(), rs.rows[0][key_index].clone())],
    })
    .await
    .expect("edit applies");

    let after = query(&mut conn, "SELECT email FROM users WHERE id = 1").await;
    assert_eq!(after.rows[0][0], Value::Text("edited@example.com".into()));
}

#[tokio::test]
async fn a_view_reports_the_base_table_it_reads_from() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE VIEW active_users AS SELECT id, email FROM users WHERE active = 1",
    )
    .await;

    // The value lives in `users`, and that is the row an edit has to reach.
    // Naming the view instead would produce an UPDATE against something that
    // stores nothing.
    let rs = query(&mut conn, "SELECT id, email FROM active_users").await;
    assert!(rs.editable);
    assert_eq!(
        rs.columns[1].source.as_ref().expect("origin").table,
        "users"
    );

    conn.apply_edit(&RowEdit {
        schema: None,
        table: "users".into(),
        changes: vec![("email".into(), Value::Text("via-view@example.com".into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit through the view's base table applies");

    let after = query(&mut conn, "SELECT email FROM users WHERE id = 1").await;
    assert_eq!(after.rows[0][0], Value::Text("via-view@example.com".into()));
}

#[tokio::test]
async fn a_trigger_definition_comes_back_as_written() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TRIGGER users_touch AFTER UPDATE ON users BEGIN SELECT 1; END",
    )
    .await;

    let roots = conn.browse(None).await.expect("browse");
    let folder = roots.iter().find(|n| n.name == "Triggers").expect("folder");
    let triggers = conn.browse(Some(&folder.id)).await.expect("triggers");

    let sql = conn
        .definition(&triggers[0].id)
        .await
        .expect("trigger definition");

    // SQLite keeps the original text, so this is the statement as typed rather
    // than a rendering of it — including the body, which is the part worth
    // editing.
    assert!(sql.starts_with("CREATE TRIGGER users_touch"), "{sql}");
    assert!(sql.contains("SELECT 1"), "{sql}");
}

#[tokio::test]
async fn indexes_list_only_the_ones_someone_wrote() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE INDEX users_by_balance ON users (balance)",
    )
    .await;

    let roots = conn.browse(None).await.expect("browse");
    let folder = roots.iter().find(|n| n.name == "Indexes").expect("folder");
    let indexes = conn.browse(Some(&folder.id)).await.expect("indexes");

    // `users.email` is UNIQUE, so SQLite also made `sqlite_autoindex_users_1`.
    // It is filtered out with the rest of the `sqlite_` internals, which is why
    // every index in this list has a statement to show.
    let names: Vec<_> = indexes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, vec!["users_by_balance"]);

    let sql = conn
        .definition(&indexes[0].id)
        .await
        .expect("index definition");
    assert!(sql.starts_with("CREATE INDEX users_by_balance"), "{sql}");
}

#[tokio::test]
async fn a_folder_has_no_definition() {
    let mut conn = seeded().await;
    let roots = conn.browse(None).await.expect("browse");
    let err = conn
        .definition(&roots[0].id)
        .await
        .expect_err("a folder is not an object");
    assert!(err.to_string().contains("object"), "{err}");
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
        Err(e) => assert_eq!(e.category(), tablex_core::ErrorCategory::Config),
        Ok(_) => panic!("connecting without a file path must fail"),
    }
}

#[test]
fn driver_advertises_only_what_it_implements() {
    let info = SqliteDriver::new().info();
    assert_eq!(info.id, "sqlite");
    assert!(info.file_based);
    assert!(info.capabilities.transactions);
    // Requires SQLITE_ENABLE_COLUMN_METADATA, which this build compiles in. If
    // the rusqlite `column_metadata` feature is ever dropped, every result
    // silently becomes read-only, so the claim is pinned here.
    assert!(info.capabilities.column_provenance);
    // Deliberately unsupported — see the comment on `info()`.
    assert!(!info.capabilities.schemas);
    assert!(!info.capabilities.cancel);
}

/// Collects what a stream hands over, so tests can assert on the pieces.
#[derive(Default)]
struct Collector {
    columns: Vec<String>,
    batches: Vec<usize>,
    rows: Vec<Vec<Value>>,
    /// Stop after this many rows, standing in for a cancelled export.
    stop_after: Option<usize>,
}

impl tablex_core::driver::RowSink for Collector {
    fn columns(&mut self, columns: &[tablex_core::result::Column]) -> Result<()> {
        self.columns = columns.iter().map(|c| c.name.clone()).collect();
        Ok(())
    }

    fn rows(&mut self, rows: &[Vec<Value>]) -> Result<()> {
        if let Some(limit) = self.stop_after {
            if self.rows.len() >= limit {
                return Err(Error::Cancelled);
            }
        }
        self.batches.push(rows.len());
        self.rows.extend(rows.iter().cloned());
        Ok(())
    }
}

#[tokio::test]
async fn streaming_delivers_rows_in_batches_rather_than_all_at_once() {
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE nums (n INTEGER)").await;
    // More than one batch, and not a multiple of it, so the last batch is short.
    let count = tablex_core::driver::STREAM_BATCH * 2 + 3;
    exec(
        &mut conn,
        &format!(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < {count}) \
             INSERT INTO nums SELECT n FROM seq"
        ),
    )
    .await;

    let mut sink = Collector::default();
    let opts = FetchOptions {
        max_rows: None,
        offset: 0,
        timeout_secs: None,
    };
    let total = conn
        .stream("SELECT n FROM nums ORDER BY n", &opts, &mut sink)
        .await
        .expect("stream");

    assert_eq!(total, count as u64);
    assert_eq!(sink.rows.len(), count);
    assert_eq!(sink.columns, vec!["n".to_string()]);

    // The point of the exercise: the rows arrived in pieces. One batch would
    // mean the whole table was in memory at once, which is what streaming is
    // for avoiding.
    assert_eq!(sink.batches.len(), 3, "{:?}", sink.batches);
    assert_eq!(sink.batches[0], tablex_core::driver::STREAM_BATCH);
    assert_eq!(sink.batches[2], 3);

    // In order, and every row present exactly once.
    assert_eq!(sink.rows[0][0], Value::Int(1));
    assert_eq!(sink.rows[count - 1][0], Value::Int(count as i64));
}

#[tokio::test]
async fn a_sink_that_stops_stops_the_stream() {
    let mut conn = connect().await;
    exec(&mut conn, "CREATE TABLE nums (n INTEGER)").await;
    let count = tablex_core::driver::STREAM_BATCH * 4;
    exec(
        &mut conn,
        &format!(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < {count}) \
             INSERT INTO nums SELECT n FROM seq"
        ),
    )
    .await;

    let mut sink = Collector {
        stop_after: Some(tablex_core::driver::STREAM_BATCH),
        ..Collector::default()
    };
    let opts = FetchOptions {
        max_rows: None,
        offset: 0,
        timeout_secs: None,
    };

    let err = conn
        .stream("SELECT n FROM nums ORDER BY n", &opts, &mut sink)
        .await
        .expect_err("the sink asked to stop");

    // The sink's own error comes back, not a channel-closed error from the
    // reader: a cancelled export must report cancellation, not plumbing.
    assert!(matches!(err, Error::Cancelled), "{err}");
    // And it stopped early rather than reading the rest of the table.
    assert!(
        sink.rows.len() < count,
        "read {} of {count}",
        sink.rows.len()
    );
}

#[tokio::test]
async fn streaming_honours_a_row_cap() {
    let mut conn = seeded().await;
    let mut sink = Collector::default();
    let opts = FetchOptions {
        max_rows: Some(1),
        offset: 0,
        timeout_secs: None,
    };

    let total = conn
        .stream("SELECT id FROM users ORDER BY id", &opts, &mut sink)
        .await
        .expect("stream");

    assert_eq!(total, 1);
    assert_eq!(sink.rows.len(), 1);
}

#[tokio::test]
async fn streaming_a_write_statement_is_refused() {
    let mut conn = seeded().await;
    let mut sink = Collector::default();
    let err = conn
        .stream(
            "UPDATE users SET active = 1",
            &FetchOptions::default(),
            &mut sink,
        )
        .await
        .expect_err("a write has no rows to stream");
    assert!(err.to_string().contains("no rows"), "{err}");
}

#[tokio::test]
async fn explains_a_query_as_a_tree() {
    let mut conn = seeded().await;
    let plan = conn
        .explain("SELECT * FROM users WHERE email = 'a@example.com'", false)
        .await
        .expect("explain");

    // SQLite plans a unique-index lookup for this, which is the whole reason the
    // plan is worth showing: the same query without the index is a full scan.
    assert!(
        plan.root.label.contains("SEARCH") && plan.root.label.contains("users"),
        "unexpected plan: {:?}",
        plan.root
    );
    // Estimates are not invented where the engine reports none.
    assert_eq!(plan.root.rows, None);
    assert_eq!(plan.root.cost, None);
    assert!(!plan.analyzed);
    assert!(!plan.raw.is_empty());
}

#[tokio::test]
async fn a_join_plan_has_a_step_per_table() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, total DECIMAL(10,2))",
    )
    .await;

    let plan = conn
        .explain(
            "SELECT u.email, o.total FROM users u JOIN orders o ON o.user_id = u.id",
            false,
        )
        .await
        .expect("explain");

    // Both sides have to appear somewhere in the tree — a plan that lost one
    // would be a plan of a different query. SQLite names them by their aliases
    // rather than their tables, which is what it prints and what the user
    // wrote, so that is what is checked.
    let mut labels = Vec::new();
    fn collect(node: &tablex_core::plan::PlanNode, out: &mut Vec<String>) {
        out.push(node.label.clone());
        for child in &node.children {
            collect(child, out);
        }
    }
    collect(&plan.root, &mut labels);
    let all = labels.join(" | ");
    assert!(all.contains("SCAN o"), "{all}");
    assert!(all.contains("SEARCH u"), "{all}");

    // Two tables being joined are siblings, not one under the other, so the
    // wrapper root is kept rather than one of them being promoted over it.
    assert_eq!(plan.root.label, "Query");
    assert_eq!(plan.root.children.len(), 2);
}

#[tokio::test]
async fn explaining_a_broken_statement_reports_the_syntax_error() {
    // Not "could not read the plan": the statement is what is wrong, and the
    // message has to say so or the user goes looking in the wrong place.
    let mut conn = seeded().await;
    let err = conn
        .explain("SELECT * FROM", false)
        .await
        .expect_err("a broken statement has no plan");

    // SQLite words this one "incomplete input" rather than "syntax error", so
    // what is pinned is that the engine's own complaint survives — not that
    // this became a "could not read the plan", which would send the user
    // looking at the wrong thing.
    assert!(matches!(err, Error::Query { .. }), "{err:?}");
    assert!(err.to_string().contains("incomplete input"), "{err}");
}

#[tokio::test]
async fn the_schema_graph_finds_every_table_and_key() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TABLE orders (
            id      INTEGER PRIMARY KEY,
            user_id INTEGER REFERENCES users(id),
            total   DECIMAL(10,2)
        )",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE order_items (
            id       INTEGER PRIMARY KEY,
            order_id INTEGER REFERENCES orders(id),
            sku      TEXT
        )",
    )
    .await;
    // Unrelated to anything, and still part of the schema.
    exec(&mut conn, "CREATE TABLE settings (key TEXT PRIMARY KEY)").await;

    let graph = conn.schema_graph(None).await.expect("schema graph");
    let names: Vec<&str> = graph.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["order_items", "orders", "settings", "users"]);

    let orders = graph.tables.iter().find(|t| t.name == "orders").unwrap();
    assert_eq!(orders.foreign_keys.len(), 1);
    assert_eq!(orders.foreign_keys[0].referenced_table, "users");
    assert_eq!(orders.foreign_keys[0].columns, vec!["user_id"]);

    // A table with no keys is still a table, and still belongs on the diagram.
    let settings = graph.tables.iter().find(|t| t.name == "settings").unwrap();
    assert!(settings.foreign_keys.is_empty());
}

#[tokio::test]
async fn the_diagram_stacks_a_chain_of_references() {
    let mut conn = seeded().await;
    exec(
        &mut conn,
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id))",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE order_items (id INTEGER PRIMARY KEY, order_id INTEGER REFERENCES orders(id))",
    )
    .await;

    let diagram = tablex_core::diagram::layout(&conn.schema_graph(None).await.unwrap());
    let y = |name: &str| {
        diagram
            .boxes
            .iter()
            .find(|b| b.table == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .y
    };
    // users is what orders stands on, and orders is what order_items stands on.
    assert!(y("users") > y("orders"));
    assert!(y("orders") > y("order_items"));
    assert!(diagram.dangling.is_empty(), "{:?}", diagram.dangling);
}

#[tokio::test]
async fn a_self_referencing_table_survives_the_round_trip() {
    // The shape that hangs a naive layout, read off a real catalog rather than
    // a hand-built graph.
    let mut conn = connect().await;
    exec(
        &mut conn,
        "CREATE TABLE employees (
            id         INTEGER PRIMARY KEY,
            manager_id INTEGER REFERENCES employees(id)
        )",
    )
    .await;

    let diagram = tablex_core::diagram::layout(&conn.schema_graph(None).await.unwrap());
    assert_eq!(diagram.boxes.len(), 1);
    assert_eq!(diagram.edges.len(), 1);
    assert!(diagram.edges[0].reflexive);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_statement_can_be_cancelled() {
    // The property the whole feature exists for: a statement that would run for
    // a very long time stops when asked, rather than the process having to be
    // killed. Multi-threaded on purpose — the interrupt has to arrive while the
    // blocking pool is inside the query.
    let mut conn = connect().await;

    let handle = conn.cancel_handle().expect("SQLite advertises cancellation");

    // A recursive CTE with no bound: it never completes on its own, so a test
    // that passes cannot be passing because the query happened to finish.
    let query = tokio::spawn(async move {
        let opts = FetchOptions {
            max_rows: None,
            offset: 0,
            timeout_secs: None,
        };
        let result = conn
            .execute(
                "WITH RECURSIVE forever(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM forever) \
                 SELECT count(*) FROM forever",
                &opts,
            )
            .await;
        result
    });

    // Long enough for the statement to be running, short enough that a broken
    // cancel fails the test rather than hanging it.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    handle.cancel().await.expect("cancel");

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), query)
        .await
        .expect("the statement should have stopped, not run to the timeout")
        .expect("the task should not panic");

    let err = outcome.expect_err("an interrupted statement does not return rows");
    // Reported as cancelled rather than as a query error: the user asked for
    // this, and a red error message for something that worked is wrong.
    assert_eq!(err.category(), tablex_core::ErrorCategory::Cancelled, "{err:?}");
}

#[tokio::test]
async fn cancelling_an_idle_connection_is_harmless() {
    // A click often lands after the statement has already finished. Treating
    // that as a failure would report the good outcome as the bad one.
    let mut conn = seeded().await;
    let handle = conn.cancel_handle().expect("handle");
    handle.cancel().await.expect("cancelling nothing succeeds");

    // And the connection is still usable afterwards.
    let rs = query(&mut conn, "SELECT count(*) FROM users").await;
    assert_eq!(rs.rows.len(), 1);
}
