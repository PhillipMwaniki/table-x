//! End-to-end tests that run the real binary.
//!
//! `CARGO_BIN_EXE_tablex` is the compiled executable, so these exercise
//! argument parsing, the URL parser, the drivers, the writers and the exit
//! codes exactly as a user or a CI job would — not a library entry point that
//! happens to sit behind them.
//!
//! SQLite is the engine because it needs no server. What is being tested is the
//! CLI, and the CLI is the same code for every driver.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A scratch directory unique to each test, removed when it is done.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("tablex-cli-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// A `sqlite:` URL for a file in this scratch directory.
    ///
    /// Built with forward slashes because a Windows path in a URL is written
    /// that way, and this is the shape the parser is expected to handle.
    fn url(&self, name: &str) -> String {
        format!(
            "sqlite://{}",
            self.path(name).display().to_string().replace('\\', "/")
        )
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, contents).expect("write fixture");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn tablex(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tablex"))
        .args(args)
        .output()
        .expect("run tablex")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

/// Run and fail loudly with stderr attached, which is where the reason is.
fn ok(args: &[&str]) -> String {
    let out = tablex(args);
    assert!(
        out.status.success(),
        "tablex {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout_of(&out)
}

const SCHEMA: &str = "\
CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, balance TEXT);
INSERT INTO users (id, email, balance) VALUES (1, 'a@example.com', '123456789012345678.1234567890');
INSERT INTO users (id, email, balance) VALUES (2, 'b@example.com', '0.5');
";

#[test]
fn a_sql_file_loads_and_the_rows_come_back() {
    let scratch = Scratch::new("load");
    let db = scratch.url("app.db");
    scratch.write("schema.sql", SCHEMA);

    ok(&[
        "--quiet",
        "import",
        "--url",
        &db,
        "--file",
        &scratch.path("schema.sql").display().to_string(),
    ]);

    let rows = ok(&[
        "--quiet",
        "query",
        "--url",
        &db,
        "--format",
        "csv",
        "SELECT id, email FROM users ORDER BY id",
    ]);
    assert!(rows.contains("1,a@example.com"), "{rows}");
    assert!(rows.contains("2,b@example.com"), "{rows}");
}

#[test]
fn an_exact_decimal_survives_a_full_round_trip() {
    // The property the whole application is built around, checked at the one
    // place a CLI makes it easy to check: text in, file out, file in, text out.
    // Thirty significant digits is well past what an f64 can hold, so any
    // conversion to a float anywhere on that path shows up here.
    let scratch = Scratch::new("exact");
    let source = scratch.url("source.db");
    let target = scratch.url("target.db");
    scratch.write("schema.sql", SCHEMA);
    let schema_path = scratch.path("schema.sql").display().to_string();

    ok(&[
        "--quiet",
        "import",
        "--url",
        &source,
        "--file",
        &schema_path,
    ]);
    ok(&[
        "--quiet",
        "export",
        "--url",
        &source,
        "--table",
        "users",
        "--format",
        "csv",
        "-o",
        &scratch.path("users.csv").display().to_string(),
    ]);

    ok(&[
        "--quiet",
        "import",
        "--url",
        &target,
        "--file",
        &schema_path,
    ]);
    ok(&["--quiet", "query", "--url", &target, "DELETE FROM users"]);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &target,
        "--table",
        "users",
        "--file",
        &scratch.path("users.csv").display().to_string(),
    ]);

    let out = ok(&[
        "--quiet",
        "query",
        "--url",
        &target,
        "--format",
        "csv",
        "SELECT balance FROM users WHERE id = 1",
    ]);
    assert!(
        out.contains("123456789012345678.1234567890"),
        "a digit was lost on the way: {out}"
    );
}

#[test]
fn ndjson_keeps_exact_numbers_as_strings() {
    // A JSON number is a double to most consumers, so emitting one here would
    // hand the rounding straight to whatever reads the output.
    let scratch = Scratch::new("ndjson");
    let db = scratch.url("app.db");
    scratch.write("schema.sql", SCHEMA);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &db,
        "--file",
        &scratch.path("schema.sql").display().to_string(),
    ]);

    let out = ok(&[
        "--quiet",
        "query",
        "--url",
        &db,
        "--format",
        "ndjson",
        "SELECT balance FROM users WHERE id = 1",
    ]);
    assert!(out.contains("\"123456789012345678.1234567890\""), "{out}");
}

#[test]
fn a_csv_with_a_quoted_comma_imports_as_one_field() {
    let scratch = Scratch::new("csv");
    let db = scratch.url("app.db");
    scratch.write("schema.sql", SCHEMA);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &db,
        "--file",
        &scratch.path("schema.sql").display().to_string(),
    ]);

    scratch.write("more.csv", "id,email,balance\n7,\"Smith, Jo\",1.00\n");
    ok(&[
        "--quiet",
        "import",
        "--url",
        &db,
        "--table",
        "users",
        "--file",
        &scratch.path("more.csv").display().to_string(),
    ]);

    let out = ok(&[
        "--quiet",
        "query",
        "--url",
        &db,
        "--format",
        "csv",
        "SELECT email FROM users WHERE id = 7",
    ]);
    // Round-tripped back out through the CSV writer, so it is quoted again.
    assert!(out.contains("Smith, Jo"), "{out}");
}

#[test]
fn diff_reports_the_statements_that_reconcile_two_schemas() {
    let scratch = Scratch::new("diff");
    let before = scratch.url("before.db");
    let after = scratch.url("after.db");

    scratch.write(
        "a.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);",
    );
    scratch.write("b.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, created TEXT);\nCREATE TABLE audit (id INTEGER PRIMARY KEY);");
    ok(&[
        "--quiet",
        "import",
        "--url",
        &before,
        "--file",
        &scratch.path("a.sql").display().to_string(),
    ]);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &after,
        "--file",
        &scratch.path("b.sql").display().to_string(),
    ]);

    let script = ok(&["--quiet", "diff", "--from", &before, "--to", &after]);
    assert!(script.contains("ADD COLUMN"), "{script}");
    assert!(script.contains("created"), "{script}");
    assert!(script.contains("CREATE TABLE"), "{script}");
    assert!(script.contains("audit"), "{script}");
}

#[test]
fn the_drift_check_exits_non_zero_only_when_the_schemas_differ() {
    // This is what makes it usable as a CI gate, so the two cases are the test.
    let scratch = Scratch::new("drift");
    let a = scratch.url("a.db");
    let b = scratch.url("b.db");
    scratch.write("a.sql", "CREATE TABLE t (id INTEGER PRIMARY KEY);");
    scratch.write(
        "b.sql",
        "CREATE TABLE t (id INTEGER PRIMARY KEY, extra TEXT);",
    );
    ok(&[
        "--quiet",
        "import",
        "--url",
        &a,
        "--file",
        &scratch.path("a.sql").display().to_string(),
    ]);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &b,
        "--file",
        &scratch.path("b.sql").display().to_string(),
    ]);

    let same = tablex(&["--quiet", "diff", "--from", &a, "--to", &a, "--exit-code"]);
    assert!(
        same.status.success(),
        "identical schemas must pass the gate"
    );

    let differs = tablex(&["--quiet", "diff", "--from", &a, "--to", &b, "--exit-code"]);
    assert!(!differs.status.success(), "drift must fail the gate");
}

#[test]
fn a_destructive_statement_is_labelled_in_the_script() {
    let scratch = Scratch::new("destructive");
    let a = scratch.url("a.db");
    let b = scratch.url("b.db");
    scratch.write(
        "a.sql",
        "CREATE TABLE keep (id INTEGER PRIMARY KEY);\nCREATE TABLE gone (id INTEGER PRIMARY KEY);",
    );
    scratch.write("b.sql", "CREATE TABLE keep (id INTEGER PRIMARY KEY);");
    ok(&[
        "--quiet",
        "import",
        "--url",
        &a,
        "--file",
        &scratch.path("a.sql").display().to_string(),
    ]);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &b,
        "--file",
        &scratch.path("b.sql").display().to_string(),
    ]);

    let script = ok(&["--quiet", "diff", "--from", &a, "--to", &b]);
    assert!(script.contains("DROP TABLE"), "{script}");
    // The comment is what a reviewer reads before running the thing.
    assert!(script.contains("is lost"), "{script}");
}

#[test]
fn a_bad_url_fails_with_a_message_naming_what_is_accepted() {
    let out = tablex(&["query", "--url", "oracle://host/db", "SELECT 1"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("postgres"), "{stderr}");
}

#[test]
fn a_failing_statement_exits_non_zero_and_says_why() {
    let scratch = Scratch::new("failure");
    let db = scratch.url("app.db");
    scratch.write("schema.sql", SCHEMA);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &db,
        "--file",
        &scratch.path("schema.sql").display().to_string(),
    ]);

    let out = tablex(&["--quiet", "query", "--url", &db, "SELECT * FROM nope"]);
    assert!(
        !out.status.success(),
        "a failed query must not report success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("nope"),
        "the engine's own complaint should survive: {stderr}"
    );
}

#[test]
fn data_goes_to_stdout_and_everything_else_to_stderr() {
    // So `tablex query … > out.csv` produces a file of rows and nothing else.
    let scratch = Scratch::new("streams");
    let db = scratch.url("app.db");
    scratch.write("schema.sql", SCHEMA);
    ok(&[
        "--quiet",
        "import",
        "--url",
        &db,
        "--file",
        &scratch.path("schema.sql").display().to_string(),
    ]);

    // Without --quiet, so the timing note is printed somewhere.
    let out = tablex(&[
        "query",
        "--url",
        &db,
        "--format",
        "csv",
        "SELECT id FROM users ORDER BY id",
    ]);
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.starts_with("id"), "{stdout}");
    assert!(
        !stdout.contains("ms"),
        "a summary leaked into the data: {stdout}"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("ms"));
}
