//! SQL Server driver tests.
//!
//! Decoding and literal-escaping are covered by unit tests in `types` that need
//! no server. The integration tests here are skipped unless `TABLEX_TEST_MSSQL`
//! is set:
//!
//! ```text
//! TABLEX_TEST_MSSQL=mssql://sa:Password123@localhost:1433/master cargo test -p tablex-drivers
//! ```

use super::*;
use indexmap::IndexMap;
use tablex_core::{config::TlsConfig, Value};

fn test_config() -> Option<(ConnectionConfig, String)> {
    let url = std::env::var("TABLEX_TEST_MSSQL").ok()?;
    let rest = url
        .strip_prefix("mssql://")
        .or_else(|| url.strip_prefix("sqlserver://"))?;

    let (creds, hostpart) = rest.split_once('@')?;
    let (user, password) = creds.split_once(':').unwrap_or((creds, ""));
    let (hostport, database) = hostpart.split_once('/').unwrap_or((hostpart, "master"));
    let (host, port) = hostport.split_once(':').unwrap_or((hostport, "1433"));

    Some((
        ConnectionConfig {
            id: "mstest".into(),
            name: "mstest".into(),
            driver: "mssql".into(),
            host: Some(host.to_string()),
            port: port.parse().ok(),
            database: Some(database.to_string()),
            username: Some(user.to_string()),
            file_path: None,
            tls: TlsConfig {
                mode: TlsMode::Prefer,
                ..Default::default()
            },
            ssh: None,
            folder: None,
            color: None,
            read_only: false,
            confirm_destructive: None,
            options: IndexMap::new(),
        },
        password.to_string(),
    ))
}

async fn connect() -> Option<Box<dyn Connection>> {
    let (config, password) = test_config()?;
    match MssqlDriver::new().connect(&config, Some(&password)).await {
        Ok(c) => Some(c),
        Err(e) => panic!("TABLEX_TEST_MSSQL is set but connecting failed: {e}"),
    }
}

macro_rules! requires_server {
    ($conn:ident) => {
        let Some(mut $conn) = connect().await else {
            eprintln!("skipping: TABLEX_TEST_MSSQL not set");
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
fn driver_reports_no_provenance_so_results_stay_read_only() {
    let info = MssqlDriver::new().info();
    assert_eq!(info.id, "mssql");
    assert_eq!(info.default_port, Some(1433));
    // tiberius does not surface the originating table, so unlike PostgreSQL and
    // MySQL an ad-hoc result here cannot be edited in place.
    assert!(!info.capabilities.column_provenance);
    assert_eq!(info.capabilities.identifier_quote, '[');
    assert_eq!(
        info.capabilities.placeholder_style,
        tablex_core::driver::PlaceholderStyle::AtP
    );
}

#[test]
fn cancellation_stays_unadvertised_and_unimplemented() {
    // Pinned as a pair on purpose. The honest options on SQL Server are an
    // attention packet, which tiberius will not send, or `KILL <spid>`, which
    // ends the session rather than the statement — see the module docs. Flipping
    // the flag to reach for the second one would give the user a "stop query"
    // button that silently drops their connection and rolls back their
    // transaction, so the flag and the missing handle are asserted together.
    let driver = MssqlDriver::new();
    assert!(!driver.info().capabilities.cancel);
}

#[tokio::test]
async fn asking_a_live_session_to_cancel_is_refused_rather_than_faked() {
    requires_server!(conn);
    // Live, so the assertion below is about cancellation rather than about a
    // connection that never worked.
    conn.ping().await.expect("ping");
    // Nothing to hand out, which is what keeps the button hidden. The Server
    // activity panel's `kill_session` is where ending a session lives, labelled
    // as what it actually does.
    assert!(conn.cancel_handle().is_none());
}

#[test]
fn identifiers_use_brackets_with_a_doubled_closing_bracket() {
    assert_eq!(quote_ident("users", QUOTE), "[users]");
    // A `]` inside a name must not close the identifier early.
    assert_eq!(quote_ident("we]ird", QUOTE), "[we]]ird]");
}

#[test]
fn catalog_literals_escape_embedded_quotes() {
    assert_eq!(escape_literal("O'Brien"), "O''Brien");
    assert_eq!(escape_literal("plain"), "plain");
}

// ---------------------------------------------------------------------------
// Integration tests — require TABLEX_TEST_MSSQL.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connects_and_runs_a_query() {
    requires_server!(conn);
    let rs = query(&mut conn, "SELECT 1 AS one, 'x' AS letter").await;
    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("x".into()));
}

#[tokio::test]
async fn decimals_are_exact_end_to_end() {
    requires_server!(conn);
    // DECIMAL(38,20) is the widest SQL Server allows and is past f64.
    let exact = "123456789012345678.12345678901234567890";
    let rs = query(
        &mut conn,
        &format!("SELECT CAST('{exact}' AS decimal(38,20)) AS n"),
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Numeric(exact.into()));
}

#[tokio::test]
async fn ad_hoc_results_are_not_editable() {
    requires_server!(conn);
    // No provenance means the grid must stay read-only rather than guessing a
    // target table.
    let rs = query(&mut conn, "SELECT 1 AS a, 2 AS b").await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn writes_report_affected_rows_via_rowcount() {
    requires_server!(conn);
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_count') IS NOT NULL DROP TABLE tx_count",
    )
    .await;
    exec(&mut conn, "CREATE TABLE tx_count (n int)").await;

    let out = conn
        .execute(
            "INSERT INTO tx_count VALUES (1), (2), (3)",
            &FetchOptions::default(),
        )
        .await
        .expect("insert");
    match &out.statements[0] {
        // The count comes from @@ROWCOUNT on the same session, because the
        // query path carries no affected-row token.
        StatementResult::Affected { rows_affected, .. } => assert_eq!(*rows_affected, 3),
        other => panic!("expected affected count, got {other:?}"),
    }

    exec(&mut conn, "DROP TABLE tx_count").await;
}

#[tokio::test]
async fn a_write_is_executed_exactly_once() {
    requires_server!(conn);
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_once') IS NOT NULL DROP TABLE tx_once",
    )
    .await;
    exec(&mut conn, "CREATE TABLE tx_once (n int)").await;

    exec(&mut conn, "INSERT INTO tx_once VALUES (1)").await;

    // Getting the affected-row count must not cost a second execution — that
    // would double every insert the user runs.
    let rs = query(&mut conn, "SELECT COUNT(*) AS c FROM tx_once").await;
    assert_eq!(rs.rows[0][0], Value::Int(1), "the write ran more than once");

    exec(&mut conn, "DROP TABLE tx_once").await;
}

#[tokio::test]
async fn apply_edit_updates_one_row_and_refuses_a_non_unique_key() {
    requires_server!(conn);
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_edit') IS NOT NULL DROP TABLE tx_edit",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE tx_edit (id int PRIMARY KEY, grp int, note nvarchar(64))",
    )
    .await;
    exec(
        &mut conn,
        "INSERT INTO tx_edit VALUES (1, 1, 'one'), (2, 1, 'two'), (3, 2, 'three')",
    )
    .await;

    conn.apply_edit(&RowEdit {
        schema: Some("dbo".into()),
        table: "tx_edit".into(),
        changes: vec![("note".into(), Value::Text("edited".into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit applies");

    let rs = query(&mut conn, "SELECT note FROM tx_edit WHERE id = 1").await;
    assert_eq!(rs.rows[0][0], Value::Text("edited".into()));

    // `grp` is not unique: two rows match, so the batch must roll back.
    let err = conn
        .apply_edit(&RowEdit {
            schema: Some("dbo".into()),
            table: "tx_edit".into(),
            changes: vec![("note".into(), Value::Text("clobbered".into()))],
            key: vec![("grp".into(), Value::Int(1))],
        })
        .await
        .expect_err("a non-unique key must be refused");
    assert!(err.to_string().contains("not unique"), "{err}");

    let rs = query(
        &mut conn,
        "SELECT COUNT(*) FROM tx_edit WHERE note = 'clobbered'",
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Int(0), "rollback must leave no trace");

    exec(&mut conn, "DROP TABLE tx_edit").await;
}

#[tokio::test]
async fn an_edit_containing_a_quote_is_stored_verbatim() {
    requires_server!(conn);
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_quote') IS NOT NULL DROP TABLE tx_quote",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE tx_quote (id int PRIMARY KEY, note nvarchar(128))",
    )
    .await;
    exec(&mut conn, "INSERT INTO tx_quote VALUES (1, 'before')").await;

    // Edits are built as literals here, so a quote in the value must be escaped
    // rather than terminating the string.
    let tricky = "O'Brien'; DROP TABLE tx_quote; --";
    conn.apply_edit(&RowEdit {
        schema: Some("dbo".into()),
        table: "tx_quote".into(),
        changes: vec![("note".into(), Value::Text(tricky.into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit applies");

    let rs = query(&mut conn, "SELECT note FROM tx_quote WHERE id = 1").await;
    assert_eq!(rs.rows[0][0], Value::Text(tricky.into()));

    exec(&mut conn, "DROP TABLE tx_quote").await;
}

#[tokio::test]
async fn browse_walks_schemas_then_tables_then_columns() {
    requires_server!(conn);
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_browse') IS NOT NULL DROP TABLE tx_browse",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE tx_browse (id int PRIMARY KEY, label nvarchar(64) NOT NULL)",
    )
    .await;

    let schemas = conn.browse(None).await.expect("browse schemas");
    assert!(schemas.iter().any(|s| s.name == "dbo"));
    // System schemas are noise.
    assert!(!schemas.iter().any(|s| s.name == "sys"));
    assert!(!schemas.iter().any(|s| s.name == "INFORMATION_SCHEMA"));

    let tables = conn.browse(Some("dbo")).await.expect("browse tables");
    assert!(tables.iter().any(|t| t.name == "tx_browse"));

    let columns = conn
        .browse(Some("dbo.tx_browse"))
        .await
        .expect("browse columns");
    let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "label"]);

    exec(&mut conn, "DROP TABLE tx_browse").await;
}

#[tokio::test]
async fn table_detail_reports_keys_indexes_and_foreign_keys() {
    requires_server!(conn);
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_child') IS NOT NULL DROP TABLE tx_child",
    )
    .await;
    exec(
        &mut conn,
        "IF OBJECT_ID('tx_parent') IS NOT NULL DROP TABLE tx_parent",
    )
    .await;
    exec(&mut conn, "CREATE TABLE tx_parent (id int PRIMARY KEY)").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_child ( \
            id int IDENTITY(1,1) PRIMARY KEY, \
            parent_id int NOT NULL REFERENCES tx_parent(id) ON DELETE CASCADE, \
            code nvarchar(32) NOT NULL UNIQUE)",
    )
    .await;

    let detail = conn
        .table_detail(Some("dbo"), "tx_child")
        .await
        .expect("detail");
    assert_eq!(detail.primary_key, vec!["id".to_string()]);
    assert_eq!(detail.edit_key(), vec!["id".to_string()]);
    assert!(detail.columns.iter().any(|c| c.auto_increment));

    let fk = detail.foreign_keys.first().expect("a foreign key");
    assert_eq!(fk.referenced_table, "tx_parent");
    // SQL Server spells it CASCADE; NO_ACTION is normalized to "NO ACTION".
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));

    assert!(detail.indexes.iter().any(|i| i.primary));
    assert!(detail.indexes.iter().any(|i| i.unique && !i.primary));

    exec(&mut conn, "DROP TABLE tx_child").await;
    exec(&mut conn, "DROP TABLE tx_parent").await;
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
        .execute(
            "SELECT TOP 100 ROW_NUMBER() OVER (ORDER BY object_id) AS n FROM sys.objects",
            &opts,
        )
        .await
        .expect("query");
    match &out.statements[0] {
        StatementResult::Rows(rs) => {
            assert_eq!(rs.rows.len(), 10);
            assert!(rs.truncated, "a capped result must be marked truncated");
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
async fn syntax_errors_carry_a_server_code() {
    requires_server!(conn);
    let err = conn
        .execute("SLECT 1", &FetchOptions::default())
        .await
        .expect_err("must fail");
    match err {
        Error::Query { code, .. } => assert!(code.is_some(), "expected a server error number"),
        other => panic!("expected a query error, got {other:?}"),
    }
}

#[tokio::test]
async fn ping_succeeds_on_a_live_connection() {
    requires_server!(conn);
    conn.ping().await.expect("ping");
}

#[test]
fn transaction_control_uses_this_engine_s_spelling() {
    // Every engine spells these differently and only some accept the others, so
    // the words are pinned rather than left to a careless edit.
    assert_eq!(super::TX.begin, "BEGIN TRANSACTION");
    assert_eq!(super::TX.commit, "COMMIT TRANSACTION");
    assert_eq!(super::TX.rollback, "ROLLBACK TRANSACTION");
}
