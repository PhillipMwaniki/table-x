//! MySQL driver tests.
//!
//! Decoding logic is covered by unit tests in `types` that need no server.
//! The integration tests here need a live MySQL or MariaDB and are skipped
//! unless `TABLEX_TEST_MYSQL` is set to a connection URL:
//!
//! ```text
//! TABLEX_TEST_MYSQL=mysql://root:password@localhost:3306/mysql cargo test -p tablex-drivers
//! ```
//!
//! Skipped rather than failed when unset, for the same reason as the PostgreSQL
//! suite: a missing database is a missing environment, not a broken driver.
//! Note that a skipped test still reports `ok`, so use `-- --nocapture` and look
//! for the skip line when you need to confirm they really ran.

use super::*;
use indexmap::IndexMap;
use tablex_core::{config::TlsConfig, Value};

fn test_config() -> Option<(ConnectionConfig, String)> {
    let url = std::env::var("TABLEX_TEST_MYSQL").ok()?;
    let rest = url
        .strip_prefix("mysql://")
        .or_else(|| url.strip_prefix("mariadb://"))?;

    let (creds, hostpart) = rest.split_once('@')?;
    let (user, password) = creds.split_once(':').unwrap_or((creds, ""));
    let (hostport, database) = hostpart.split_once('/').unwrap_or((hostpart, "mysql"));
    let (host, port) = hostport.split_once(':').unwrap_or((hostport, "3306"));

    Some((
        ConnectionConfig {
            id: "mytest".into(),
            name: "mytest".into(),
            driver: "mysql".into(),
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
    match MysqlDriver::new().connect(&config, Some(&password)).await {
        Ok(c) => Some(c),
        Err(e) => panic!("TABLEX_TEST_MYSQL is set but connecting failed: {e}"),
    }
}

macro_rules! requires_server {
    ($conn:ident) => {
        let Some(mut $conn) = connect().await else {
            eprintln!("skipping: TABLEX_TEST_MYSQL not set");
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
fn driver_advertises_provenance_and_backtick_quoting() {
    let info = MysqlDriver::new().info();
    assert_eq!(info.id, "mysql");
    assert_eq!(info.default_port, Some(3306));
    assert!(!info.file_based);
    // MySQL reports org_table/org_name, so ad-hoc results are editable.
    assert!(info.capabilities.column_provenance);
    // Backticks, not double quotes — unless ANSI_QUOTES is set, which is rare.
    assert_eq!(info.capabilities.identifier_quote, '`');
    assert_eq!(
        info.capabilities.placeholder_style,
        tablex_core::driver::PlaceholderStyle::Question
    );
}

#[test]
fn identifiers_are_backtick_quoted_and_escaped() {
    assert_eq!(quote_ident("users", QUOTE), "`users`");
    // A backtick inside an identifier must not break out of the quoting.
    assert_eq!(quote_ident("we`ird", QUOTE), "`we``ird`");
}

// ---------------------------------------------------------------------------
// Integration tests — require TABLEX_TEST_MYSQL.
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
    // DECIMAL(65,30) is the widest MySQL allows and is far past f64.
    let exact = "12345678901234567890123456789012345.123456789012345678901234567890";
    let rs = query(
        &mut conn,
        &format!("SELECT CAST('{exact}' AS DECIMAL(65,30)) AS n"),
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Numeric(exact.into()));
}

#[tokio::test]
async fn tinyint_1_decodes_as_boolean() {
    requires_server!(conn);
    exec(&mut conn, "DROP TEMPORARY TABLE IF EXISTS tx_bool").await;
    exec(&mut conn, "CREATE TEMPORARY TABLE tx_bool (flag BOOLEAN)").await;
    exec(&mut conn, "INSERT INTO tx_bool VALUES (TRUE), (FALSE)").await;

    let rs = query(&mut conn, "SELECT flag FROM tx_bool").await;
    // MySQL has no BOOLEAN type; it is TINYINT(1), and the UI should show it
    // as a boolean rather than as 1 and 0.
    assert_eq!(rs.rows[0][0], Value::Bool(true));
    assert_eq!(rs.rows[1][0], Value::Bool(false));
}

#[tokio::test]
async fn unsigned_bigint_does_not_wrap() {
    requires_server!(conn);
    // u64::MAX does not fit in i64; narrowing would show it as -1.
    let rs = query(
        &mut conn,
        "SELECT CAST(18446744073709551615 AS UNSIGNED) AS n",
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::UInt(u64::MAX));
}

#[tokio::test]
async fn blobs_stay_binary_and_text_stays_text() {
    requires_server!(conn);
    let rs = query(
        &mut conn,
        "SELECT CAST('abc' AS BINARY) AS b, CAST('abc' AS CHAR) AS t",
    )
    .await;
    // BLOB and TEXT share a type code and differ only by the binary flag.
    assert!(
        matches!(rs.rows[0][0], Value::Bytes(_)),
        "{:?}",
        rs.rows[0][0]
    );
    assert_eq!(rs.rows[0][1], Value::Text("abc".into()));
}

#[tokio::test]
async fn single_table_results_are_editable_and_joins_are_not() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_orders, tx_users").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_users (id INT PRIMARY KEY, email VARCHAR(255) NOT NULL)",
    )
    .await;
    exec(
        &mut conn,
        "CREATE TABLE tx_orders (id INT PRIMARY KEY, user_id INT, \
         FOREIGN KEY (user_id) REFERENCES tx_users(id) ON DELETE CASCADE)",
    )
    .await;
    exec(
        &mut conn,
        "INSERT INTO tx_users VALUES (1, 'a@example.com'), (2, 'b@example.com')",
    )
    .await;

    let rs = query(&mut conn, "SELECT id, email FROM tx_users ORDER BY id").await;
    assert!(
        rs.editable,
        "single-table result with its PK must be editable"
    );
    assert_eq!(rs.key_columns, vec!["id".to_string()]);

    let rs = query(&mut conn, "SELECT email FROM tx_users").await;
    assert!(!rs.editable, "a result without its key must be read-only");

    let rs = query(
        &mut conn,
        "SELECT u.id, o.id AS oid FROM tx_users u JOIN tx_orders o ON o.user_id = u.id",
    )
    .await;
    assert!(!rs.editable, "a join must never be editable");

    let rs = query(&mut conn, "SELECT COUNT(*) FROM tx_users").await;
    assert!(!rs.editable, "an aggregate has no source table");

    exec(&mut conn, "DROP TABLE tx_orders, tx_users").await;
}

#[tokio::test]
async fn apply_edit_updates_one_row_and_refuses_a_non_unique_key() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_edit").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_edit (id INT PRIMARY KEY, grp INT, note VARCHAR(64))",
    )
    .await;
    exec(
        &mut conn,
        "INSERT INTO tx_edit VALUES (1, 1, 'one'), (2, 1, 'two'), (3, 2, 'three')",
    )
    .await;

    let db = test_config().expect("config").0.database.unwrap();

    conn.apply_edit(&RowEdit {
        schema: Some(db.clone()),
        table: "tx_edit".into(),
        changes: vec![("note".into(), Value::Text("edited".into()))],
        key: vec![("id".into(), Value::Int(1))],
    })
    .await
    .expect("edit applies");

    let rs = query(&mut conn, "SELECT note FROM tx_edit WHERE id = 1").await;
    assert_eq!(rs.rows[0][0], Value::Text("edited".into()));

    // `grp` is not unique: this would hit two rows, so it must be refused.
    let err = conn
        .apply_edit(&RowEdit {
            schema: Some(db),
            table: "tx_edit".into(),
            changes: vec![("note".into(), Value::Text("clobbered".into()))],
            key: vec![("grp".into(), Value::Int(1))],
        })
        .await
        .expect_err("a non-unique key must be refused");
    assert!(err.to_string().contains("expected at most 1"), "{err}");

    let rs = query(
        &mut conn,
        "SELECT COUNT(*) FROM tx_edit WHERE note = 'clobbered'",
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Int(0), "rollback must leave no trace");

    exec(&mut conn, "DROP TABLE tx_edit").await;
}

#[tokio::test]
async fn browse_walks_databases_then_tables_then_columns() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_browse").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_browse (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL)",
    )
    .await;

    let db = test_config().expect("config").0.database.unwrap();

    let databases = conn.browse(None).await.expect("browse databases");
    // System catalogs are noise and must be hidden.
    assert!(!databases.iter().any(|d| d.name == "information_schema"));
    assert!(!databases.iter().any(|d| d.name == "performance_schema"));

    let tables = conn.browse(Some(&db)).await.expect("browse tables");
    assert!(tables.iter().any(|t| t.name == "tx_browse"));

    let columns = conn
        .browse(Some(&format!("{db}.tx_browse")))
        .await
        .expect("browse columns");
    let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "label"]);

    exec(&mut conn, "DROP TABLE tx_browse").await;
}

#[tokio::test]
async fn table_detail_reports_keys_indexes_and_foreign_keys() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_child, tx_parent").await;
    exec(&mut conn, "CREATE TABLE tx_parent (id INT PRIMARY KEY)").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_child ( \
            id INT PRIMARY KEY AUTO_INCREMENT, \
            parent_id INT NOT NULL, \
            code VARCHAR(32) NOT NULL UNIQUE, \
            FOREIGN KEY (parent_id) REFERENCES tx_parent(id) ON DELETE CASCADE)",
    )
    .await;

    let db = test_config().expect("config").0.database.unwrap();
    let detail = conn
        .table_detail(Some(&db), "tx_child")
        .await
        .expect("detail");

    assert_eq!(detail.primary_key, vec!["id".to_string()]);
    assert_eq!(detail.edit_key(), vec!["id".to_string()]);
    assert!(detail.columns.iter().any(|c| c.auto_increment));

    let fk = detail.foreign_keys.first().expect("a foreign key");
    assert_eq!(fk.referenced_table, "tx_parent");
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));

    assert!(detail.indexes.iter().any(|i| i.primary));
    assert!(detail.indexes.iter().any(|i| i.unique && !i.primary));

    exec(&mut conn, "DROP TABLE tx_child, tx_parent").await;
}

#[tokio::test]
async fn row_cap_truncates_and_says_so() {
    requires_server!(conn);
    exec(&mut conn, "DROP TEMPORARY TABLE IF EXISTS tx_many").await;
    exec(&mut conn, "CREATE TEMPORARY TABLE tx_many (n INT)").await;
    exec(
        &mut conn,
        "INSERT INTO tx_many (n) WITH RECURSIVE s(n) AS \
         (SELECT 1 UNION ALL SELECT n+1 FROM s WHERE n < 100) SELECT n FROM s",
    )
    .await;

    let opts = FetchOptions {
        max_rows: Some(10),
        offset: 0,
        timeout_secs: None,
    };
    let out = conn
        .execute("SELECT n FROM tx_many ORDER BY n", &opts)
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
async fn writes_report_affected_rows() {
    requires_server!(conn);
    exec(&mut conn, "DROP TEMPORARY TABLE IF EXISTS tx_count").await;
    exec(&mut conn, "CREATE TEMPORARY TABLE tx_count (n INT)").await;

    let out = conn
        .execute(
            "INSERT INTO tx_count VALUES (1), (2), (3)",
            &FetchOptions::default(),
        )
        .await
        .expect("insert");
    match &out.statements[0] {
        StatementResult::Affected { rows_affected, .. } => assert_eq!(*rows_affected, 3),
        other => panic!("expected affected count, got {other:?}"),
    }
}

#[tokio::test]
async fn syntax_errors_carry_a_sqlstate() {
    requires_server!(conn);
    let err = conn
        .execute("SLECT 1", &FetchOptions::default())
        .await
        .expect_err("must fail");
    match err {
        Error::Query { code, .. } => {
            // 42000 is MySQL's syntax-error class.
            assert_eq!(code.as_deref(), Some("42000"));
        }
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
    assert_eq!(super::TX.begin, "START TRANSACTION");
    assert_eq!(super::TX.commit, "COMMIT");
    assert_eq!(super::TX.rollback, "ROLLBACK");
}

/// A statement that will not finish on its own, so a passing cancellation test
/// cannot be passing because the query happened to end.
///
/// `SLEEP()` is deliberately not used: MySQL documents it as *returning 1* when
/// interrupted, so the statement can complete successfully and the test would be
/// asserting nothing. A cross join over the data dictionary has no such special
/// case — it is an ordinary scan the server abandons when told to.
const NEVER_FINISHES: &str = "SELECT COUNT(*) FROM information_schema.columns a \
     JOIN information_schema.columns b ON 1 = 1 \
     JOIN information_schema.columns c ON 1 = 1";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_statement_can_be_cancelled_without_losing_the_session() {
    requires_server!(conn);

    let handle = conn.cancel_handle().expect("MySQL advertises cancellation");

    // The connection comes back out with the result: the point of `KILL QUERY`
    // over `KILL` is that there is still a session to hand back.
    let running = tokio::spawn(async move {
        let opts = FetchOptions {
            max_rows: None,
            offset: 0,
            timeout_secs: None,
        };
        let result = conn.execute(NEVER_FINISHES, &opts).await;
        (conn, result)
    });

    // Long enough for the statement to be running on the server, short enough
    // that a broken cancel fails the test rather than hanging it.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    handle.cancel().await.expect("cancel");

    let (mut conn, result) = tokio::time::timeout(std::time::Duration::from_secs(30), running)
        .await
        .expect("the statement should have stopped, not run to the timeout")
        .expect("the task should not panic");

    let err = result.expect_err("an interrupted statement does not return rows");
    // Reported as cancelled rather than as a query error: the user asked for
    // this, and a red error message for something that worked is wrong.
    assert_eq!(
        err.category(),
        tablex_core::ErrorCategory::Cancelled,
        "{err:?}"
    );

    // The property that distinguishes this from `KILL`: the session survived, so
    // the tab the user was working in is still connected.
    let rs = query(&mut conn, "SELECT 1 AS one").await;
    assert_eq!(rs.rows[0][0], Value::Int(1));
}

#[tokio::test]
async fn cancelling_an_idle_connection_is_harmless() {
    // A click often lands after the statement has already finished. `KILL QUERY`
    // against a session running nothing is a no-op on the server, and treating
    // it as a failure would report the good outcome as the bad one.
    requires_server!(conn);
    let handle = conn.cancel_handle().expect("handle");
    handle.cancel().await.expect("cancelling nothing succeeds");

    let rs = query(&mut conn, "SELECT 1 AS one").await;
    assert_eq!(rs.rows[0][0], Value::Int(1));
}

#[test]
fn cancellation_is_advertised_because_kill_query_is_wired_up() {
    // The UI draws the stop button from this flag, so advertising it without an
    // implementation would be a button that fails.
    assert!(MysqlDriver::new().info().capabilities.cancel);
}
