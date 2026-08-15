//! The MCP server, driven over stdio the way a client drives it.
//!
//! These run the real binary and speak the real protocol, because the
//! interesting properties are not "does the function return the right value"
//! but "does the guard hold when a client asks it not to" — and a guard is only
//! worth anything at the process boundary.

use std::io::Write;
use std::process::{Command, Stdio};

struct Scratch {
    dir: std::path::PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("tablex-mcp-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch { dir }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(name)
    }

    fn url(&self, name: &str) -> String {
        format!(
            "sqlite://{}",
            self.path(name).display().to_string().replace('\\', "/")
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const SCHEMA: &str = "\
CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL, balance TEXT);
INSERT INTO users (id, email, balance) VALUES (1, 'a@example.com', '123456789012345678.1234567890');
INSERT INTO users (id, email, balance) VALUES (2, 'b@example.com', '0.5');
INSERT INTO users (id, email, balance) VALUES (3, 'c@example.com', '1.25');
";

fn seed(scratch: &Scratch, name: &str) -> String {
    let sql = scratch.path("schema.sql");
    std::fs::write(&sql, SCHEMA).expect("write schema");
    let url = scratch.url(name);
    let out = Command::new(env!("CARGO_BIN_EXE_tablex"))
        .args([
            "--quiet",
            "import",
            "--url",
            &url,
            "--file",
            &sql.display().to_string(),
        ])
        .output()
        .expect("seed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    url
}

/// Send these JSON-RPC lines and collect the replies, in order.
fn talk(args: &[&str], requests: &[&str]) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tablex"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tablex mcp");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write request");
        }
    }
    // Dropping stdin closes it, which is what ends the server's read loop.
    let out = child.wait_with_output().expect("wait");

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad JSON {l:?}: {e}")))
        .collect()
}

/// The text a tool call came back with.
fn tool_text(reply: &serde_json::Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn it_initializes_and_lists_its_tools() {
    let scratch = Scratch::new("handshake");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ],
    );

    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "tablex");
    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"query"), "{names:?}");
    assert!(names.contains(&"describe_table"), "{names:?}");
}

#[test]
fn a_notification_gets_no_reply() {
    // Answering one is a protocol error, not merely noise: a client counting
    // responses against its own requests would be permanently off by one.
    let scratch = Scratch::new("notify");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
        ],
    );

    assert_eq!(
        replies.len(),
        1,
        "the notification was answered: {replies:?}"
    );
    assert_eq!(replies[0]["id"], 7);
}

#[test]
fn a_write_is_refused_and_the_rows_are_still_there() {
    // The property the whole item exists for, checked at both ends: the call
    // reports a refusal, and the table is untouched afterwards.
    let scratch = Scratch::new("readonly");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query","arguments":{"sql":"DELETE FROM users"}}}"#,
        ],
    );

    assert_eq!(replies[0]["result"]["isError"], true);
    let text = tool_text(&replies[0]);
    // Named rather than vague, so an agent can rewrite instead of retrying.
    assert!(text.contains("read-only"), "{text}");
    assert!(text.contains("DELETE FROM users"), "{text}");

    let after = Command::new(env!("CARGO_BIN_EXE_tablex"))
        .args([
            "--quiet",
            "query",
            "--url",
            &url,
            "--format",
            "csv",
            "SELECT count(*) FROM users",
        ])
        .output()
        .expect("count");
    assert!(
        String::from_utf8_lossy(&after.stdout).contains('3'),
        "rows were deleted"
    );
}

#[test]
fn select_into_is_refused_too() {
    // It reads as a SELECT until the third word, which is exactly the case a
    // first-keyword check would wave through.
    let scratch = Scratch::new("into");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query","arguments":{"sql":"SELECT * INTO copies FROM users"}}}"#,
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], true, "{:?}", replies[0]);
}

#[test]
fn a_write_goes_through_when_writes_were_asked_for() {
    // The guard is a default, not a cage — otherwise --allow-writes would be a
    // flag that does nothing.
    let scratch = Scratch::new("writable");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url, "--allow-writes"],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query","arguments":{"sql":"DELETE FROM users WHERE id = 3"}}}"#,
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], false, "{:?}", replies[0]);
}

#[test]
fn the_row_cap_holds_against_a_larger_request() {
    // A client may ask for fewer than the cap and never for more.
    let scratch = Scratch::new("cap");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url, "--max-rows", "2"],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query","arguments":{"sql":"SELECT id FROM users","limit":9999}}}"#,
        ],
    );

    let payload: serde_json::Value = serde_json::from_str(&tool_text(&replies[0])).expect("json");
    assert_eq!(payload["results"][0]["row_count"], 2);
    // Said plainly, because a capped page that looks complete is how an agent
    // concludes a table has two rows in it.
    assert_eq!(payload["results"][0]["truncated"], true);
}

#[test]
fn exact_decimals_reach_the_agent_as_strings() {
    // A JSON number is a double to almost every consumer, so emitting one here
    // would hand the rounding to whatever reads it.
    let scratch = Scratch::new("exact");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query","arguments":{"sql":"SELECT balance FROM users WHERE id = 1"}}}"#,
        ],
    );

    let text = tool_text(&replies[0]);
    assert!(text.contains("\"123456789012345678.1234567890\""), "{text}");
}

#[test]
fn every_call_lands_in_the_audit_log_including_the_refused_one() {
    // A log that records only what succeeded cannot answer the question anyone
    // actually asks it.
    let scratch = Scratch::new("audit");
    let url = seed(&scratch, "app.db");
    let audit = scratch.path("audit.jsonl").display().to_string();

    talk(
        &["--quiet", "mcp", "--url", &url, "--audit", &audit],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query","arguments":{"sql":"SELECT 1"}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query","arguments":{"sql":"DROP TABLE users"}}}"#,
        ],
    );

    let log = std::fs::read_to_string(&audit).expect("audit log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines[0].contains("\"ok\":true"), "{}", lines[0]);
    assert!(lines[1].contains("\"ok\":false"), "{}", lines[1]);
    assert!(lines[1].contains("DROP TABLE users"), "{}", lines[1]);
}

#[test]
fn describe_table_answers_without_reading_any_rows() {
    let scratch = Scratch::new("describe");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"describe_table","arguments":{"table":"users"}}}"#,
        ],
    );

    let payload: serde_json::Value = serde_json::from_str(&tool_text(&replies[0])).expect("json");
    assert_eq!(payload["primary_key"][0], "id");
    let columns = payload["columns"].as_array().expect("columns");
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[1]["name"], "email");
    assert_eq!(columns[1]["nullable"], false);
}

#[test]
fn an_unknown_method_is_a_protocol_error_not_a_tool_error() {
    // They are read by different parts of a client: one means "this server
    // cannot do that", the other means "that attempt did not work".
    let scratch = Scratch::new("unknown");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#],
    );
    assert_eq!(replies[0]["error"]["code"], -32601);
}

#[test]
fn malformed_input_does_not_stop_the_server() {
    // A client that sends one bad line should not have to reconnect.
    let scratch = Scratch::new("malformed");
    let url = seed(&scratch, "app.db");

    let replies = talk(
        &["--quiet", "mcp", "--url", &url],
        &[
            "{not json at all",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ],
    );

    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["id"], 2, "the server stopped after a bad line");
}
