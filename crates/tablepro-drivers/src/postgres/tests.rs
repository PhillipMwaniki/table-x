//! PostgreSQL driver tests.
//!
//! The decoding logic — which is where the subtle bugs live — is covered by pure
//! unit tests in `numeric` and `types` that need no server.
//!
//! The tests below exercise a real server and are skipped unless `TABLEPRO_TEST_PG`
//! is set to a connection URL, e.g.
//!
//! ```text
//! TABLEPRO_TEST_PG=postgres://postgres:postgres@localhost:5432/postgres cargo test -p tablepro-drivers
//! ```
//!
//! They are skipped rather than failed when unset: a missing database is a missing
//! environment, not a broken driver, and a test suite that fails on a fresh
//! checkout trains people to ignore it.

use super::*;
use indexmap::IndexMap;
use tablepro_core::{config::TlsConfig, Value};

/// Parse `TABLEPRO_TEST_PG` into a config, or `None` to skip.
fn test_config() -> Option<ConnectionConfig> {
    let url = std::env::var("TABLEPRO_TEST_PG").ok()?;
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))?;

    let (creds, hostpart) = rest.split_once('@')?;
    let (user, password) = creds.split_once(':').unwrap_or((creds, ""));
    let (hostport, database) = hostpart.split_once('/').unwrap_or((hostpart, "postgres"));
    let (host, port) = hostport.split_once(':').unwrap_or((hostport, "5432"));

    Some((
        ConnectionConfig {
            id: "pgtest".into(),
            name: "pgtest".into(),
            driver: "postgres".into(),
            host: Some(host.to_string()),
            port: port.parse().ok(),
            database: Some(database.to_string()),
            username: Some(user.to_string()),
            file_path: None,
            tls: TlsConfig {
                mode: TlsMode::Disable,
                ..Default::default()
            },
            ssh: None,
            color: None,
            read_only: false,
            options: IndexMap::new(),
        },
        password.to_string(),
    ))
    .map(|(mut cfg, pw)| {
        cfg.options.insert("__password".into(), pw);
        cfg
    })
}

/// Connect, or return `None` when no test server is configured.
async fn connect() -> Option<Box<dyn Connection>> {
    let cfg = test_config()?;
    let password = cfg.options.get("__password").cloned().unwrap_or_default();
    match PostgresDriver::new().connect(&cfg, Some(&password)).await {
        Ok(c) => Some(c),
        Err(e) => panic!("TABLEPRO_TEST_PG is set but connecting failed: {e}"),
    }
}

/// Skip the body when no server is configured.
macro_rules! requires_server {
    ($conn:ident) => {
        let Some(mut $conn) = connect().await else {
            eprintln!("skipping: TABLEPRO_TEST_PG not set");
            return;
        };
    };
}

async fn exec(conn: &mut Box<dyn Connection>, sql: &str) {
    conn.execute(sql, &FetchOptions::default())
        .await
        .unwrap_or_else(|e| panic!("exec failed: {sql}\n{e}"));
}

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

// ---------------------------------------------------------------------------
// Unit tests — always run.
// ---------------------------------------------------------------------------

#[test]
fn driver_advertises_provenance_which_sqlite_cannot() {
    let info = PostgresDriver::new().info();
    assert_eq!(info.id, "postgres");
    assert_eq!(info.default_port, Some(5432));
    assert!(!info.file_based);
    // The capability that makes ad-hoc results editable on PostgreSQL.
    assert!(info.capabilities.column_provenance);
    assert!(info.capabilities.schemas);
    // Not yet wired to a cancel token, so not advertised.
    assert!(!info.capabilities.cancel);
    assert_eq!(
        info.capabilities.placeholder_style,
        tablepro_core::driver::PlaceholderStyle::Dollar
    );
}

// ---------------------------------------------------------------------------
// Integration tests — require TABLEPRO_TEST_PG.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connects_and_runs_a_query() {
    requires_server!(conn);
    let rs = query(&mut conn, "SELECT 1 AS one, 'x'::text AS letter").await;
    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("x".into()));
}

#[tokio::test]
async fn numerics_are_exact_end_to_end() {
    requires_server!(conn);
    // 40 significant digits — past f64 and past every fixed-width decimal type.
    let exact = "12345678901234567890.12345678901234567890";
    let rs = query(&mut conn, &format!("SELECT '{exact}'::numeric AS n")).await;
    assert_eq!(rs.rows[0][0], Value::Numeric(exact.into()));
}

#[tokio::test]
async fn unknown_types_are_shown_not_dropped() {
    requires_server!(conn);
    // `point` has no dedicated decoder; the row must still come back.
    let rs = query(&mut conn, "SELECT '(1,2)'::point AS p, 42 AS n").await;
    assert!(matches!(rs.rows[0][0], Value::Unsupported { .. }));
    // The rest of the row is unaffected — this is the whole point of degrading.
    assert_eq!(rs.rows[0][1], Value::Int(42));
}

#[tokio::test]
async fn arrays_decode_elementwise_including_nulls() {
    requires_server!(conn);
    let rs = query(&mut conn, "SELECT ARRAY[1, NULL, 3]::int4[] AS a").await;
    assert_eq!(
        rs.rows[0][0],
        Value::Array(vec![Value::Int(1), Value::Null, Value::Int(3)])
    );
}

#[tokio::test]
async fn timestamp_and_timestamptz_are_distinguished() {
    requires_server!(conn);
    let rs = query(
        &mut conn,
        "SELECT '2026-08-13 11:30:00'::timestamp AS naive, \
                '2026-08-13 11:30:00+00'::timestamptz AS instant",
    )
    .await;
    // A wall-clock reading must not be silently promoted to an instant.
    assert!(
        matches!(rs.rows[0][0], Value::DateTime(_)),
        "{:?}",
        rs.rows[0][0]
    );
    assert!(
        matches!(rs.rows[0][1], Value::TimestampTz(_)),
        "{:?}",
        rs.rows[0][1]
    );
}

#[tokio::test]
async fn syntax_errors_carry_position_and_sqlstate() {
    requires_server!(conn);
    let err = conn
        .execute("SLECT 1", &FetchOptions::default())
        .await
        .expect_err("must fail");
    match err {
        Error::Query { position, code, .. } => {
            // The editor uses these to underline the offending token.
            assert!(position.is_some(), "expected a character position");
            assert_eq!(code.as_deref(), Some("42601"));
        }
        other => panic!("expected a query error, got {other:?}"),
    }
}

#[tokio::test]
async fn single_table_results_are_editable_and_joins_are_not() {
    requires_server!(conn);
    exec(
        &mut conn,
        "DROP TABLE IF EXISTS tpx_users, tpx_orders CASCADE",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE tpx_users (id int PRIMARY KEY, email text NOT NULL)",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE tpx_orders (id int PRIMARY KEY, user_id int REFERENCES tpx_users(id))",
    )
    .await;
    exec(
        &mut conn,
        "INSERT INTO tpx_users VALUES (1, 'a@example.com'), (2, 'b@example.com')",
    )
    .await;

    // Provenance resolves to one table and the primary key is projected.
    let rs = query(&mut conn, "SELECT id, email FROM tpx_users ORDER BY id").await;
    assert!(
        rs.editable,
        "single-table result with its PK must be editable"
    );
    assert_eq!(rs.key_columns, vec!["id".to_string()]);

    // A projection that omits the key cannot build a safe WHERE clause.
    let rs = query(&mut conn, "SELECT email FROM tpx_users").await;
    assert!(!rs.editable, "result without its key must be read-only");

    // A join has no single target table for an UPDATE.
    let rs = query(
        &mut conn,
        "SELECT u.id, o.id AS order_id FROM tpx_users u JOIN tpx_orders o ON o.user_id = u.id",
    )
    .await;
    assert!(!rs.editable, "a join must never be editable");

    // An aggregate has no provenance at all.
    let rs = query(&mut conn, "SELECT count(*) FROM tpx_users").await;
    assert!(!rs.editable);

    exec(&mut conn, "DROP TABLE tpx_orders, tpx_users CASCADE").await;
}

#[tokio::test]
async fn apply_edit_updates_exactly_one_row() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_edit CASCADE").await;
    exec(
        &mut conn,
        "CREATE TABLE tpx_edit (id int PRIMARY KEY, email text, amount numeric(30,10))",
    )
    .await;
    exec(
        &mut conn,
        "INSERT INTO tpx_edit VALUES (1, 'a@example.com', 1.5), (2, 'b@example.com', 2.5)",
    )
    .await;

    conn.apply_edit(&RowEdit {
        schema: Some("public".into()),
        table: "tpx_edit".into(),
        changes: vec![("email".into(), Value::Text("new@example.com".into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit applies");

    let rs = query(&mut conn, "SELECT email FROM tpx_edit WHERE id = 1").await;
    assert_eq!(rs.rows[0][0], Value::Text("new@example.com".into()));

    // The sibling row is untouched.
    let rs = query(&mut conn, "SELECT email FROM tpx_edit WHERE id = 2").await;
    assert_eq!(rs.rows[0][0], Value::Text("b@example.com".into()));

    exec(&mut conn, "DROP TABLE tpx_edit CASCADE").await;
}

#[tokio::test]
async fn editing_a_numeric_column_does_not_round_it() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_money CASCADE").await;
    exec(
        &mut conn,
        "CREATE TABLE tpx_money (id int PRIMARY KEY, amount numeric(40,20))",
    )
    .await;
    exec(&mut conn, "INSERT INTO tpx_money VALUES (1, 0)").await;

    // The text-cast parameter path must carry every digit through to the server.
    let exact = "12345678901234567890.12345678901234567890";
    conn.apply_edit(&RowEdit {
        schema: Some("public".into()),
        table: "tpx_money".into(),
        changes: vec![("amount".into(), Value::Numeric(exact.into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit applies");

    let rs = query(&mut conn, "SELECT amount FROM tpx_money WHERE id = 1").await;
    assert_eq!(rs.rows[0][0], Value::Numeric(exact.into()));

    exec(&mut conn, "DROP TABLE tpx_money CASCADE").await;
}

#[tokio::test]
async fn apply_edit_rolls_back_when_the_key_is_not_unique() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_dup CASCADE").await;
    exec(&mut conn, "CREATE TABLE tpx_dup (grp int, note text)").await;
    exec(
        &mut conn,
        "INSERT INTO tpx_dup VALUES (1, 'one'), (1, 'two'), (2, 'three')",
    )
    .await;

    let err = conn
        .apply_edit(&RowEdit {
            schema: Some("public".into()),
            table: "tpx_dup".into(),
            changes: vec![("note".into(), Value::Text("clobbered".into()))],
            key: vec![("grp".into(), Value::Int(1))],
        })
        .await
        .expect_err("a non-unique key must be refused");
    assert!(err.to_string().contains("expected exactly 1"), "{err}");

    let rs = query(
        &mut conn,
        "SELECT count(*) FROM tpx_dup WHERE note = 'clobbered'",
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Int(0), "rollback must leave no trace");

    exec(&mut conn, "DROP TABLE tpx_dup CASCADE").await;
}

#[tokio::test]
async fn apply_edit_matches_null_keys_with_is_null() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_null CASCADE").await;
    exec(&mut conn, "CREATE TABLE tpx_null (k text, v text)").await;
    exec(&mut conn, "INSERT INTO tpx_null VALUES (NULL, 'before')").await;

    conn.apply_edit(&RowEdit {
        schema: Some("public".into()),
        table: "tpx_null".into(),
        changes: vec![("v".into(), Value::Text("after".into()))],
        key: vec![("k".into(), Value::Null)],
    })
    .await
    .expect("NULL key must match via IS NULL");

    let rs = query(&mut conn, "SELECT v FROM tpx_null").await;
    assert_eq!(rs.rows[0][0], Value::Text("after".into()));

    exec(&mut conn, "DROP TABLE tpx_null CASCADE").await;
}

#[tokio::test]
async fn browse_walks_schemas_then_tables_then_columns() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_browse CASCADE").await;
    exec(
        &mut conn,
        "CREATE TABLE tpx_browse (id int PRIMARY KEY, label text NOT NULL)",
    )
    .await;

    let schemas = conn.browse(None).await.expect("browse schemas");
    assert!(schemas.iter().any(|s| s.name == "public"));
    // System schemas are noise and must be hidden.
    assert!(!schemas.iter().any(|s| s.name == "pg_catalog"));
    assert!(!schemas.iter().any(|s| s.name == "information_schema"));

    let tables = conn.browse(Some("public")).await.expect("browse tables");
    assert!(tables.iter().any(|t| t.name == "tpx_browse"));

    let columns = conn
        .browse(Some("public.tpx_browse"))
        .await
        .expect("browse columns");
    let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "label"]);
    assert!(columns.iter().all(|c| !c.expandable));

    exec(&mut conn, "DROP TABLE tpx_browse CASCADE").await;
}

#[tokio::test]
async fn table_detail_reports_keys_indexes_and_foreign_keys() {
    requires_server!(conn);
    exec(
        &mut conn,
        "DROP TABLE IF EXISTS tpx_child, tpx_parent CASCADE",
    )
    .await;
    exec(&mut conn, "CREATE TABLE tpx_parent (id int PRIMARY KEY)").await;
    exec(
        &mut conn,
        "CREATE TABLE tpx_child ( \
            id int PRIMARY KEY, \
            parent_id int NOT NULL REFERENCES tpx_parent(id) ON DELETE CASCADE, \
            code text NOT NULL UNIQUE )",
    )
    .await;

    let detail = conn
        .table_detail(Some("public"), "tpx_child")
        .await
        .expect("detail");
    assert_eq!(detail.primary_key, vec!["id".to_string()]);
    assert_eq!(detail.edit_key(), vec!["id".to_string()]);
    assert_eq!(detail.columns.len(), 3);

    let fk = detail.foreign_keys.first().expect("a foreign key");
    assert_eq!(fk.referenced_table, "tpx_parent");
    assert_eq!(fk.columns, vec!["parent_id".to_string()]);
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));

    assert!(detail.indexes.iter().any(|i| i.primary));
    assert!(detail.indexes.iter().any(|i| i.unique && !i.primary));

    exec(&mut conn, "DROP TABLE tpx_child, tpx_parent CASCADE").await;
}

#[tokio::test]
async fn row_cap_truncates_and_says_so() {
    requires_server!(conn);
    let opts = FetchOptions {
        max_rows: Some(10),
        offset: 0,
        timeout_secs: None,
    };
    let out = conn
        .execute("SELECT generate_series(1, 500) AS n", &opts)
        .await
        .expect("query");
    match &out.statements[0] {
        StatementResult::Rows(rs) => {
            assert_eq!(rs.rows.len(), 10);
            assert!(rs.truncated, "capped result must be marked truncated");
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
async fn writes_report_affected_rows() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_count CASCADE").await;
    exec(&mut conn, "CREATE TABLE tpx_count (n int)").await;

    let out = conn
        .execute(
            "INSERT INTO tpx_count SELECT generate_series(1, 5)",
            &FetchOptions::default(),
        )
        .await
        .expect("insert");
    match &out.statements[0] {
        StatementResult::Affected { rows_affected, .. } => assert_eq!(*rows_affected, 5),
        other => panic!("expected affected count, got {other:?}"),
    }

    exec(&mut conn, "DROP TABLE tpx_count CASCADE").await;
}

#[tokio::test]
async fn completion_scope_lists_schemas_tables_and_functions() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tpx_scope CASCADE").await;
    exec(&mut conn, "CREATE TABLE tpx_scope (alpha int, beta text)").await;

    let scope = conn.completion_scope().await.expect("scope");
    assert!(scope.schemas.iter().any(|s| s == "public"));

    let (_, columns) = scope
        .tables
        .iter()
        .find(|(t, _)| t == "public.tpx_scope")
        .expect("table in completion scope");
    assert!(columns.contains(&"alpha".to_string()));
    assert!(!scope.functions.is_empty());

    exec(&mut conn, "DROP TABLE tpx_scope CASCADE").await;
}

#[tokio::test]
async fn ping_succeeds_on_a_live_connection() {
    requires_server!(conn);
    conn.ping().await.expect("ping");
}
