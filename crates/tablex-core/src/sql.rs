//! SQL text utilities shared by every driver.

/// Split a submission into individual statements on top-level semicolons.
///
/// Naive splitting on `;` corrupts any statement containing a semicolon inside a
/// string, a comment, or a function body — and users paste those constantly.
/// This scanner tracks the lexical contexts where a semicolon is *not* a
/// separator:
///
/// - single- and double-quoted strings, including doubled-quote escapes (`''`)
/// - backtick identifiers (MySQL) and bracket identifiers (SQL Server)
/// - `--` line comments and `/* */` block comments, which nest in PostgreSQL
/// - PostgreSQL dollar-quoted bodies (`$$ ... $$`, `$tag$ ... $tag$`), which is
///   how virtually every stored procedure is written
///
/// Trailing empty statements are dropped, so a trailing `;` does not produce a
/// spurious empty execution.
pub fn split_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' | b'"' | b'`' => {
                let quote = c;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        // A doubled quote is an escaped quote, not a terminator.
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    // Backslash escapes apply inside strings, not identifiers.
                    if bytes[i] == b'\\' && quote == b'\'' && i + 1 < bytes.len() {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
            }
            b'[' => {
                // SQL Server bracket identifier: [My Table]. No escapes beyond ]].
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b']' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                // PostgreSQL block comments nest, so track depth rather than
                // stopping at the first */.
                let mut depth = 1;
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                if let Some(tag_end) = dollar_tag_end(bytes, i) {
                    let tag = &bytes[i..tag_end];
                    i = tag_end;
                    // Scan for the matching closing tag.
                    while i < bytes.len() {
                        if bytes[i] == b'$' && bytes[i..].starts_with(tag) {
                            i += tag.len();
                            break;
                        }
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            b';' => {
                push_statement(&mut out, &sql[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }

    push_statement(&mut out, &sql[start..]);
    out
}

/// If a dollar-quote tag starts at `i`, return the index just past it.
/// Tags are `$$` or `$name$` where `name` is an identifier.
fn dollar_tag_end(bytes: &[u8], i: usize) -> Option<usize> {
    debug_assert_eq!(bytes[i], b'$');
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'$' => return Some(j + 1),
            c if c.is_ascii_alphanumeric() || c == b'_' => j += 1,
            // Anything else means this `$` was a parameter marker or an operator.
            _ => return None,
        }
    }
    None
}

fn push_statement(out: &mut Vec<String>, s: &str) {
    let trimmed = s.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

/// Whether a submission appears to modify data or schema.
///
/// This backs the per-connection read-only flag, which exists to stop someone
/// running a migration against production by mistake. It is **a guard, not a
/// security boundary**: it is a keyword scan, and a determined statement can
/// evade it (`SELECT pg_terminate_backend(...)` has side effects and reads as a
/// SELECT). Real protection is a database role without write permission.
///
/// It deliberately errs toward calling things writes. A false positive costs a
/// user one toggle; a false negative costs them a table.
///
/// Keywords are matched only outside strings, comments, and quoted identifiers,
/// so `SELECT 'update'` and `SELECT update_time FROM t` are both reads.
pub fn looks_like_write(sql: &str) -> bool {
    const WRITE_KEYWORDS: &[&str] = &[
        "INSERT", "UPDATE", "DELETE", "MERGE", "UPSERT", "REPLACE", "TRUNCATE", "DROP", "CREATE",
        "ALTER", "RENAME", "GRANT", "REVOKE", "COMMENT", "VACUUM", "REINDEX", "CLUSTER", "COPY",
        "CALL", "DO", "EXECUTE", "SET", "RESET", "LOCK", "REFRESH", "ATTACH", "DETACH",
    ];
    bare_words(sql)
        .into_iter()
        .any(|w| WRITE_KEYWORDS.contains(&w.to_ascii_uppercase().as_str()))
}

/// Identifier-like tokens that appear outside strings, comments, and quoted
/// identifiers. Used for keyword scanning where a match inside a literal would
/// be a false positive.
fn bare_words(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let mut words = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' | b'"' | b'`' => {
                let quote = c;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'\\' && quote == b'\'' && i + 1 < bytes.len() {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
            }
            b'[' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                i += 1;
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                let mut depth = 1;
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                if let Some(tag_end) = dollar_tag_end(bytes, i) {
                    let tag = &bytes[i..tag_end];
                    i = tag_end;
                    while i < bytes.len() {
                        if bytes[i] == b'$' && bytes[i..].starts_with(tag) {
                            i += tag.len();
                            break;
                        }
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
                {
                    i += 1;
                }
                words.push(&sql[start..i]);
            }
            _ => i += 1,
        }
    }
    words
}

/// Quote an identifier for interpolation into generated SQL.
///
/// Used when building `UPDATE` statements for inline edits, where table and
/// column names come from catalog metadata rather than from a parameter. The
/// quote character is per-database, and the escape is always doubling.
pub fn quote_ident(name: &str, quote: char) -> String {
    let mut s = String::with_capacity(name.len() + 2);
    s.push(quote);
    let close = match quote {
        '[' => ']',
        other => other,
    };
    for ch in name.chars() {
        if ch == close {
            s.push(close);
        }
        s.push(ch);
    }
    s.push(close);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_statements() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2"),
            vec!["SELECT 1", "SELECT 2"]
        );
    }

    #[test]
    fn trailing_semicolon_does_not_create_an_empty_statement() {
        assert_eq!(split_statements("SELECT 1;"), vec!["SELECT 1"]);
        assert_eq!(split_statements("SELECT 1;  \n  "), vec!["SELECT 1"]);
        assert!(split_statements("   ;;  ").is_empty());
    }

    #[test]
    fn semicolons_inside_strings_are_not_separators() {
        let sql = "INSERT INTO t VALUES ('a;b'); SELECT 1";
        assert_eq!(
            split_statements(sql),
            vec!["INSERT INTO t VALUES ('a;b')", "SELECT 1"]
        );
    }

    #[test]
    fn doubled_quotes_are_escapes_not_terminators() {
        // 'it''s; fine' is one string containing a semicolon.
        let sql = "SELECT 'it''s; fine'; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT 'it''s; fine'", "SELECT 2"]
        );
    }

    #[test]
    fn semicolons_inside_comments_are_ignored() {
        let sql = "SELECT 1; -- trailing; comment\nSELECT 2";
        assert_eq!(split_statements(sql).len(), 2);

        let sql = "SELECT 1 /* a; b */ + 1; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT 1 /* a; b */ + 1", "SELECT 2"]
        );
    }

    #[test]
    fn comments_are_preserved_in_the_statement_they_precede() {
        // Comments must survive verbatim: optimizer hints are written as comments
        // (`/*+ INDEX(t idx) */` in Oracle/MySQL, pg_hint_plan in PostgreSQL), so
        // stripping them would silently change the query plan.
        let parts = split_statements("SELECT 1; -- why\nSELECT 2");
        assert_eq!(parts[1], "-- why\nSELECT 2");

        let parts = split_statements("/*+ INDEX(t idx) */ SELECT * FROM t");
        assert_eq!(parts[0], "/*+ INDEX(t idx) */ SELECT * FROM t");
    }

    #[test]
    fn block_comments_nest() {
        // PostgreSQL allows nesting; stopping at the first */ would split wrongly.
        let sql = "SELECT 1 /* outer /* inner; */ still; comment */ + 1; SELECT 2";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2, "got {parts:?}");
        assert_eq!(parts[1], "SELECT 2");
    }

    #[test]
    fn dollar_quoted_bodies_survive_intact() {
        // This is the case that matters most: nearly every stored procedure has
        // semicolons in its body.
        let sql = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN; RETURN 1; END; $$ LANGUAGE plpgsql; SELECT 2";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2, "got {parts:?}");
        assert!(parts[0].contains("RETURN 1;"));
        assert_eq!(parts[1], "SELECT 2");
    }

    #[test]
    fn tagged_dollar_quotes_survive_intact() {
        let sql = "SELECT $tag$ a; b $tag$; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT $tag$ a; b $tag$", "SELECT 2"]
        );
    }

    #[test]
    fn dollar_placeholders_are_not_mistaken_for_quotes() {
        // $1 is a PostgreSQL parameter, not the start of a dollar-quoted body.
        let sql = "SELECT * FROM t WHERE a = $1; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT * FROM t WHERE a = $1", "SELECT 2"]
        );
    }

    #[test]
    fn quoted_identifiers_may_contain_semicolons() {
        let sql = r#"SELECT "we;ird" FROM t; SELECT 2"#;
        assert_eq!(
            split_statements(sql),
            vec![r#"SELECT "we;ird" FROM t"#, "SELECT 2"]
        );

        let sql = "SELECT `we;ird` FROM t; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT `we;ird` FROM t", "SELECT 2"]
        );

        let sql = "SELECT [we;ird] FROM t; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT [we;ird] FROM t", "SELECT 2"]
        );
    }

    #[test]
    fn unterminated_string_does_not_hang_or_panic() {
        // Users run half-typed SQL constantly; this must terminate cleanly.
        let parts = split_statements("SELECT 'unterminated");
        assert_eq!(parts, vec!["SELECT 'unterminated"]);
    }

    #[test]
    fn reads_are_not_flagged_as_writes() {
        for sql in [
            "SELECT * FROM users",
            "  select 1  ",
            "EXPLAIN ANALYZE SELECT * FROM t",
            "WITH recent AS (SELECT * FROM events) SELECT * FROM recent",
            "SHOW TABLES",
            "VALUES (1), (2)",
        ] {
            assert!(!looks_like_write(sql), "false positive on: {sql}");
        }
    }

    #[test]
    fn writes_are_flagged() {
        for sql in [
            "INSERT INTO t VALUES (1)",
            "update t set a = 1",
            "DELETE FROM t",
            "DROP TABLE t",
            "TRUNCATE t",
            "ALTER TABLE t ADD COLUMN c int",
            "GRANT SELECT ON t TO bob",
        ] {
            assert!(looks_like_write(sql), "missed write: {sql}");
        }
    }

    #[test]
    fn a_write_hidden_behind_a_cte_is_still_a_write() {
        // The statement opens with WITH and reads like a SELECT, but it deletes.
        let sql = "WITH doomed AS (SELECT id FROM t WHERE stale) \
                   DELETE FROM t USING doomed WHERE t.id = doomed.id";
        assert!(looks_like_write(sql));
    }

    #[test]
    fn keywords_inside_literals_are_not_writes() {
        // The classic false positive: a read that merely mentions a keyword.
        assert!(!looks_like_write("SELECT 'drop table users' AS note"));
        assert!(!looks_like_write("SELECT * FROM t WHERE msg = 'delete me'"));
        assert!(!looks_like_write("SELECT 1 -- delete this later"));
        assert!(!looks_like_write("SELECT 1 /* insert notes */"));
    }

    #[test]
    fn identifiers_containing_keywords_are_not_writes() {
        // `update_time` must not read as UPDATE; word boundaries matter.
        assert!(!looks_like_write("SELECT update_time, created FROM t"));
        assert!(!looks_like_write("SELECT dropped_at FROM t"));
        assert!(!looks_like_write(r#"SELECT "delete" FROM t"#));
    }

    #[test]
    fn identifiers_are_quoted_and_escaped() {
        assert_eq!(quote_ident("users", '"'), r#""users""#);
        assert_eq!(quote_ident("my`table", '`'), "`my``table`");
        // An identifier containing the quote character must not break out of it.
        assert_eq!(quote_ident(r#"a"b"#, '"'), r#""a""b""#);
        assert_eq!(quote_ident("a]b", '['), "[a]]b]");
    }
}
