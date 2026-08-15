//! ClickHouse driver tests.
//!
//! Decoding is covered by unit tests in `types` that need no server. The
//! integration tests here are skipped unless `TABLEX_TEST_CLICKHOUSE` is set:
//!
//! ```text
//! TABLEX_TEST_CLICKHOUSE=clickhouse://default:@localhost:8123/default cargo test -p tablex-drivers
//! ```

use super::*;
use indexmap::IndexMap;
use tablex_core::{config::TlsConfig, Value};

fn test_config() -> Option<(ConnectionConfig, String)> {
    let url = std::env::var("TABLEX_TEST_CLICKHOUSE").ok()?;
    let rest = url
        .strip_prefix("clickhouse://")
        .or_else(|| url.strip_prefix("http://"))?;

    let (creds, hostpart) = rest.split_once('@')?;
    let (user, password) = creds.split_once(':').unwrap_or((creds, ""));
    let (hostport, database) = hostpart.split_once('/').unwrap_or((hostpart, "default"));
    let (host, port) = hostport.split_once(':').unwrap_or((hostport, "8123"));

    Some((
        ConnectionConfig {
            id: "chtest".into(),
            name: "chtest".into(),
            driver: "clickhouse".into(),
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
    match ClickhouseDriver::new()
        .connect(&config, Some(&password))
        .await
    {
        Ok(c) => Some(c),
        Err(e) => panic!("TABLEX_TEST_CLICKHOUSE is set but connecting failed: {e}"),
    }
}

macro_rules! requires_server {
    ($conn:ident) => {
        let Some(mut $conn) = connect().await else {
            eprintln!("skipping: TABLEX_TEST_CLICKHOUSE not set");
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
fn driver_advertises_the_http_port_and_no_transactions() {
    let info = ClickhouseDriver::new().info();
    assert_eq!(info.id, "clickhouse");
    // 8123 is the HTTP interface; 9000 is the native protocol this does not use.
    assert_eq!(info.default_port, Some(8123));
    assert!(!info.file_based);
    // Experimental and single-partition only, so not advertised.
    assert!(!info.capabilities.transactions);
    // No source table in JSONCompact metadata, and no row-level UPDATE.
    assert!(!info.capabilities.column_provenance);
    assert!(!info.capabilities.foreign_keys);
    // A ClickHouse database is its only container level — there is nothing
    // between it and a table — so the tree has no schema layer to render.
    assert!(!info.capabilities.schemas);
    assert!(info.capabilities.databases);
}

#[tokio::test]
async fn inline_edits_are_refused_with_an_explanation() {
    // ClickHouse mutations are asynchronous and rewrite whole parts, so an
    // inline cell edit cannot promise "this row, now, exactly once". Refusing
    // is more honest than issuing an ALTER that appears to succeed.
    ensure_crypto_provider();
    let mut conn = ClickhouseConnection {
        client: reqwest::Client::new(),
        base: "http://127.0.0.1:1/".into(),
        user: "default".into(),
        password: String::new(),
        database: "default".into(),
    };

    let err = conn
        .apply_edit(&RowEdit {
            schema: Some("default".into()),
            table: "t".into(),
            changes: vec![("a".into(), Value::Int(1))],
            key: vec![("id".into(), Value::Int(1))],
        })
        .await
        .expect_err("must refuse");

    assert_eq!(err.category(), tablex_core::ErrorCategory::Unsupported);
    assert!(err.to_string().contains("no row-level UPDATE"), "{err}");
    // The message must say what to do instead, not just decline.
    assert!(err.to_string().contains("ALTER TABLE"), "{err}");
}

#[test]
fn auth_failures_are_categorized_from_the_clickhouse_error_code() {
    // 516 is AUTHENTICATION_FAILED; the HTTP status alone would say only 500.
    let err = map_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "Code: 516. DB::Exception: default: Authentication failed",
    );
    assert_eq!(err.category(), tablex_core::ErrorCategory::Auth);

    // 81 is UNKNOWN_DATABASE — a connection problem, not a query problem.
    let err = map_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "Code: 81. DB::Exception: Database nope does not exist",
    );
    assert_eq!(err.category(), tablex_core::ErrorCategory::Connection);
}

#[test]
fn syntax_errors_keep_their_code_and_stay_query_errors() {
    let err = map_http_error(
        reqwest::StatusCode::BAD_REQUEST,
        "Code: 62. DB::Exception: Syntax error",
    );
    match err {
        Error::Query { code, .. } => assert_eq!(code.as_deref(), Some("62")),
        other => panic!("expected a query error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Integration tests — require TABLEX_TEST_CLICKHOUSE.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connects_and_runs_a_query() {
    requires_server!(conn);
    let rs = query(&mut conn, "SELECT 1 AS one, 'x' AS letter").await;
    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("x".into()));
}

#[tokio::test]
async fn wide_integers_survive_the_json_round_trip() {
    requires_server!(conn);
    // A JSON number is a double; without the server quoting 64-bit integers this
    // would come back rounded.
    let rs = query(&mut conn, "SELECT toInt64(9223372036854775807) AS n").await;
    assert_eq!(rs.rows[0][0], Value::Int(i64::MAX));

    let rs = query(&mut conn, "SELECT toUInt64(18446744073709551615) AS n").await;
    assert_eq!(rs.rows[0][0], Value::UInt(u64::MAX));
}

#[tokio::test]
async fn decimals_are_exact_end_to_end() {
    requires_server!(conn);
    let exact = "12345678901234567890.123456789012345678";
    let rs = query(
        &mut conn,
        &format!("SELECT toDecimal128('{exact}', 18) AS n"),
    )
    .await;
    assert_eq!(rs.rows[0][0], Value::Numeric(exact.into()));
}

#[tokio::test]
async fn nullable_columns_report_null_and_nullability() {
    requires_server!(conn);
    let rs = query(&mut conn, "SELECT CAST(NULL AS Nullable(String)) AS n").await;
    assert_eq!(rs.rows[0][0], Value::Null);
    // ClickHouse spells nullability in the type name.
    assert_eq!(rs.columns[0].nullable, Some(true));
}

#[tokio::test]
async fn arrays_decode_elementwise() {
    requires_server!(conn);
    let rs = query(&mut conn, "SELECT [1, 2, 3] AS a").await;
    match &rs.rows[0][0] {
        Value::Array(items) => assert_eq!(items.len(), 3),
        other => panic!("expected an array, got {other:?}"),
    }
}

#[tokio::test]
async fn results_are_never_editable() {
    requires_server!(conn);
    // No provenance and no row-level UPDATE, so the grid must stay read-only.
    let rs = query(&mut conn, "SELECT 1 AS a").await;
    assert!(!rs.editable);
    assert!(rs.key_columns.is_empty());
}

#[tokio::test]
async fn inserts_report_written_rows_from_the_summary_header() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_ch").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_ch (n Int64) ENGINE = MergeTree ORDER BY n",
    )
    .await;

    let out = conn
        .execute(
            "INSERT INTO tx_ch VALUES (1), (2), (3)",
            &FetchOptions::default(),
        )
        .await
        .expect("insert");
    match &out.statements[0] {
        // The body is empty for an INSERT; the count comes from
        // X-ClickHouse-Summary.
        StatementResult::Affected { rows_affected, .. } => assert_eq!(*rows_affected, 3),
        other => panic!("expected affected count, got {other:?}"),
    }

    exec(&mut conn, "DROP TABLE tx_ch").await;
}

#[tokio::test]
async fn browse_walks_databases_then_tables_then_columns() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_browse").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_browse (id Int64, label String) ENGINE = MergeTree ORDER BY id",
    )
    .await;

    let databases = conn.browse(None).await.expect("browse databases");
    assert!(databases.iter().any(|d| d.name == "default"));
    // System databases are noise.
    assert!(!databases.iter().any(|d| d.name == "system"));

    let tables = conn.browse(Some("default")).await.expect("browse tables");
    assert!(tables.iter().any(|t| t.name == "tx_browse"));

    let columns = conn
        .browse(Some("default.tx_browse"))
        .await
        .expect("browse columns");
    let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "label"]);

    exec(&mut conn, "DROP TABLE tx_browse").await;
}

#[tokio::test]
async fn the_sorting_key_is_reported_as_an_index_not_a_primary_key() {
    requires_server!(conn);
    exec(&mut conn, "DROP TABLE IF EXISTS tx_key").await;
    exec(
        &mut conn,
        "CREATE TABLE tx_key (id Int64, v String) ENGINE = MergeTree ORDER BY id",
    )
    .await;

    let detail = conn
        .table_detail(Some("default"), "tx_key")
        .await
        .expect("detail");

    // A ClickHouse sorting key is not unique, so calling it a primary key would
    // let the grid believe a row is uniquely addressable when it is not.
    assert!(detail.primary_key.is_empty());
    assert!(detail.edit_key().is_empty());

    let index = detail.indexes.first().expect("a sorting key");
    assert_eq!(index.columns, vec!["id".to_string()]);
    assert!(
        !index.unique,
        "a sorting key must not be reported as unique"
    );

    exec(&mut conn, "DROP TABLE tx_key").await;
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
        .execute("SELECT number FROM system.numbers LIMIT 100", &opts)
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
async fn syntax_errors_carry_a_code() {
    requires_server!(conn);
    let err = conn
        .execute("SLECT 1", &FetchOptions::default())
        .await
        .expect_err("must fail");
    assert_eq!(err.category(), tablex_core::ErrorCategory::Query);
}

#[tokio::test]
async fn ping_succeeds_on_a_live_connection() {
    requires_server!(conn);
    conn.ping().await.expect("ping");
}
