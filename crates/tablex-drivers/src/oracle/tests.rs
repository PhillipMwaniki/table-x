//! Oracle driver tests.
//!
//! Everything here runs without a server, and that is a deliberate limit rather
//! than a claim of coverage: this driver has not been run against a live Oracle
//! at the time of writing. What can be checked without one is what this driver
//! *writes* — the literals it escapes, the statements it builds, the types it
//! declares — and those are exactly the parts that are wrong silently.
//!
//! What cannot be checked here is whether the catalogue queries return what
//! they claim. `ALL_TAB_COLUMNS` and its relatives are written from the
//! documentation; the first run against a real database is what confirms them.
//!
//! An integration test needs both an Oracle and Instant Client on the machine,
//! which is why there is no `TABLEX_TEST_ORACLE` harness here yet.

use super::*;
use tablex_core::Value;

#[test]
fn the_driver_declares_what_it_is() {
    let info = OracleDriver::new().info();
    assert_eq!(info.id, "oracle");
    assert_eq!(info.default_port, Some(1521));
    assert!(!info.file_based);
    assert_eq!(
        info.capabilities.placeholder_style,
        tablex_core::driver::PlaceholderStyle::Colon
    );
    assert_eq!(info.capabilities.identifier_quote, '"');
}

#[test]
fn a_result_cannot_be_edited_because_a_column_has_no_provenance() {
    // ODPI reports a column's type and not the table it came from, so an
    // ad-hoc result has nothing to write back to. Claiming otherwise would put
    // an editable grid in front of somebody whose edits cannot be addressed.
    assert!(!OracleDriver::new().info().capabilities.column_provenance);
}

#[test]
fn ddl_is_not_transactional_here() {
    // Oracle commits implicitly before and after every DDL statement, so a set
    // that fails halfway stays half applied. The structure editor needs to know
    // that before it offers to apply one.
    assert!(
        !OracleDriver::new()
            .info()
            .capabilities
            .ddl
            .transactional_ddl
    );
}

#[test]
fn every_declared_type_is_one_oracle_would_accept() {
    // Not a spell-check: a type offered in the column form goes straight into a
    // CREATE TABLE, so a typo here is a statement that fails on the server.
    let info = OracleDriver::new().info();
    assert!(info.column_types.contains(&"VARCHAR2(255)".to_string()));
    assert!(info.column_types.contains(&"NUMBER(10,2)".to_string()));
    assert!(info.column_types.contains(&"CLOB".to_string()));
    // Not `VARCHAR`, which Oracle reserves and warns against, and not `TEXT`,
    // `INT4` or `DATETIME`, which are other engines' spellings.
    for wrong in ["TEXT", "DATETIME", "INT4", "SERIAL", "VARCHAR"] {
        assert!(
            !info.column_types.iter().any(|t| t == wrong),
            "{wrong} is not an Oracle type"
        );
    }
}

// Literals --------------------------------------------------------------------

#[test]
fn a_quote_in_a_string_is_doubled() {
    // The one escaping rule Oracle has for a literal, and the one that decides
    // whether `O'Hara` is a name or a syntax error.
    assert_eq!(types::literal(&Value::Text("O'Hara".into())), "'O''Hara'");
}

#[test]
fn an_exact_number_goes_in_unquoted_and_undamaged() {
    // Quoting it would make the server parse it as text and convert it back,
    // and going through a float would lose the digits past the sixteenth. A
    // NUMBER holds thirty-eight.
    assert_eq!(
        types::literal(&Value::Numeric("12345678901234567890.12345".into())),
        "12345678901234567890.12345"
    );
}

#[test]
fn something_shaped_like_a_number_but_not_one_is_quoted() {
    // `1e5`, `0x10` and `1.2.3` are not decimal literals; letting them through
    // unquoted would be putting user text into a statement unescaped.
    for text in ["1e5", "0x10", "1.2.3", "12; DROP TABLE t"] {
        let sql = types::literal(&Value::Numeric(text.into()));
        assert!(
            sql.starts_with('\''),
            "{text} should have been quoted: {sql}"
        );
    }
}

#[test]
fn null_is_the_keyword_rather_than_the_word() {
    assert_eq!(types::literal(&Value::Null), "NULL");
    // And the string "NULL" is still a string.
    assert_eq!(types::literal(&Value::Text("NULL".into())), "'NULL'");
}

#[test]
fn a_boolean_becomes_one_or_zero() {
    // Oracle had no SQL boolean before 23c, and a column that holds one is a
    // NUMBER(1) or a CHAR(1).
    assert_eq!(types::literal(&Value::Bool(true)), "1");
    assert_eq!(types::literal(&Value::Bool(false)), "0");
}

#[test]
fn a_date_carries_its_own_format_rather_than_trusting_the_session() {
    // NLS_DATE_FORMAT differs between sessions, so an unformatted date literal
    // means different things to two users of the same database.
    let date = chrono::NaiveDate::from_ymd_opt(2026, 3, 14).expect("date");
    assert_eq!(types::literal(&Value::Date(date)), "DATE '2026-03-14'");

    let stamp = date.and_hms_opt(9, 30, 0).expect("time");
    let sql = types::literal(&Value::DateTime(stamp));
    assert!(
        sql.starts_with("TO_TIMESTAMP('2026-03-14 09:30:00"),
        "{sql}"
    );
    assert!(sql.contains("YYYY-MM-DD HH24:MI:SS"), "{sql}");
}

#[test]
fn binary_goes_in_as_hex_rather_than_as_text() {
    assert_eq!(
        types::literal(&Value::Bytes(vec![0x00, 0x0f, 0xff])),
        "HEXTORAW('000FFF')"
    );
}

// Statement building ----------------------------------------------------------

#[test]
fn a_key_matches_null_with_is_null() {
    // `= NULL` is never true in SQL, so a row whose key column is NULL would
    // silently match nothing and the edit would appear to do nothing.
    let key = vec![
        ("id".to_string(), Value::Int(1)),
        ("tenant".to_string(), Value::Null),
    ];
    assert_eq!(predicate(&key), r#""id" = 1 AND "tenant" IS NULL"#);
}

#[test]
fn an_identifier_is_quoted_and_its_own_quotes_doubled() {
    // Oracle folds unquoted identifiers to upper case, so a lower-case column
    // name only resolves when it is quoted.
    let key = vec![("odd\"name".to_string(), Value::Int(1))];
    assert_eq!(predicate(&key), r#""odd""name" = 1"#);
}

#[test]
fn a_catalogue_lookup_escapes_the_name_it_is_given() {
    // These go into the WHERE clause of a catalogue query as literals, and a
    // schema really can be named with an apostrophe in it.
    assert_eq!(literal("O'Brien"), "'O''Brien'");
}

// Type mapping ----------------------------------------------------------------

#[test]
fn a_declared_type_keeps_the_size_it_was_given() {
    // ALL_TAB_COLUMNS stores the parts separately, so a bare `VARCHAR2` loses
    // half of what the column is — and a schema comparison built on the bare
    // name would call two different columns identical.
    let varchar = vec![
        Value::Text("NAME".into()),
        Value::Text("VARCHAR2".into()),
        Value::Int(255),
        Value::Null,
        Value::Null,
    ];
    assert_eq!(declared_type(&varchar), "VARCHAR2(255)");

    let money = vec![
        Value::Text("AMOUNT".into()),
        Value::Text("NUMBER".into()),
        Value::Int(22),
        Value::Int(10),
        Value::Int(2),
    ];
    assert_eq!(declared_type(&money), "NUMBER(10,2)");

    let counter = vec![
        Value::Text("N".into()),
        Value::Text("NUMBER".into()),
        Value::Int(22),
        Value::Int(10),
        Value::Int(0),
    ];
    assert_eq!(declared_type(&counter), "NUMBER(10)");

    // An unconstrained NUMBER is written without parentheses; `NUMBER(0)` is
    // not a type.
    let plain = vec![
        Value::Text("N".into()),
        Value::Text("NUMBER".into()),
        Value::Int(22),
        Value::Null,
        Value::Null,
    ];
    assert_eq!(declared_type(&plain), "NUMBER");

    // A type with no size of its own is left alone.
    let date = vec![
        Value::Text("CREATED".into()),
        Value::Text("DATE".into()),
        Value::Int(7),
        Value::Null,
        Value::Null,
    ];
    assert_eq!(declared_type(&date), "DATE");
}

// Errors ----------------------------------------------------------------------

#[test]
fn a_missing_client_library_is_answered_with_what_to_install() {
    // The first thing most people will meet, and an error code is no help with
    // it: nothing is wrong with the database, the connection or the password.
    let error = classify("DPI-1047: Cannot locate a 64-bit Oracle Client library".into());
    let text = error.to_string();
    assert!(text.contains("Instant Client"), "{text}");
    assert!(text.contains("PATH"), "{text}");
    assert!(matches!(error, Error::Config(_)), "{error:?}");
}

#[test]
fn a_bad_password_is_an_auth_failure_rather_than_a_query_error() {
    // The category is what decides whether the UI offers to retry, and
    // retrying identical credentials burns login attempts against a policy
    // that locks the account.
    let error = classify("ORA-01017: invalid username/password".into());
    assert!(matches!(error, Error::Auth(_)), "{error:?}");
}

#[test]
fn a_listener_that_is_not_there_is_a_connection_failure() {
    let error = classify("ORA-12541: TNS:no listener".into());
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
}

#[test]
fn the_servers_answer_to_a_cancel_is_not_reported_as_a_failure() {
    // ORA-01013 is the user getting what they asked for. A red alert for the
    // button they just pressed working is the wrong signal.
    let error = classify("ORA-01013: user requested cancel".into());
    assert!(matches!(error, Error::Cancelled), "{error:?}");
}
