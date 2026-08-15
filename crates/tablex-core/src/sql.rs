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
/// - `BEGIN ... END` routine bodies, which is how the engines *without* dollar
///   quoting write a trigger or a procedure
///
/// Trailing empty statements are dropped, so a trailing `;` does not produce a
/// spurious empty execution.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut splitter = Splitter::new();
    let mut out = splitter.push(sql);
    out.extend(splitter.finish());
    out
}

/// Where the scanner is, so it can stop mid-input and pick up where it left off.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Normal,
    /// Inside `'…'`, `"…"`, or `` `…` ``.
    Quoted(u8),
    /// Inside a SQL Server `[…]` identifier.
    Bracketed,
    LineComment,
    /// Block comments nest in PostgreSQL, so the depth is tracked.
    BlockComment(usize),
    /// Inside `$tag$ … $tag$`, holding the tag to match.
    DollarQuoted(String),
}

/// Splits SQL into statements as the text arrives.
///
/// [`split_statements`] is this fed once. The incremental form exists for
/// importing a dump file: a multi-gigabyte script cannot be read into memory to
/// be split, and its statements have to reach the server while the rest of the
/// file is still being read.
///
/// The buffer holds only the statement currently being scanned — everything
/// before it is handed out and dropped — so memory tracks the largest single
/// statement rather than the size of the file.
pub struct Splitter {
    buffer: String,
    /// How far into `buffer` the scanner has consumed.
    scanned: usize,
    state: State,
    /// Nesting inside a `BEGIN ... END` routine body.
    depth: usize,
}

impl Default for Splitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Splitter {
    pub fn new() -> Self {
        Splitter {
            buffer: String::new(),
            scanned: 0,
            state: State::Normal,
            depth: 0,
        }
    }

    /// Bytes currently held: one statement at most, never the whole input.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Feed more text, returning whatever statements are now complete.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        self.scan(false)
    }

    /// Flush the trailing statement, if the input ended without a semicolon.
    ///
    /// A dump's last statement usually has one; a hand-typed one usually does
    /// not, and dropping it would be the kind of silent loss this codebase
    /// avoids.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = self.scan(true);
        let rest = self.buffer.split_off(0);
        self.scanned = 0;
        self.state = State::Normal;
        self.depth = 0;
        push_statement(&mut out, &rest);
        out
    }

    /// Scan the buffer, emitting complete statements.
    ///
    /// `at_end` decides what happens at a token that needs more bytes to
    /// classify — a lone `-`, an unterminated word, the start of a `$tag$`.
    /// Mid-stream the scanner stops and waits; at the end of input there is
    /// nothing more coming, so it takes the bytes as they are.
    fn scan(&mut self, at_end: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = self.scanned;

        while i < self.buffer.len() {
            let bytes = self.buffer.as_bytes();
            let c = bytes[i];

            match &self.state {
                State::Quoted(quote) => {
                    let quote = *quote;
                    if c == b'\\' && quote == b'\'' {
                        // A backslash escape needs the byte after it, which may
                        // not have arrived.
                        if i + 1 >= bytes.len() && !at_end {
                            break;
                        }
                        i += 2;
                        continue;
                    }
                    if c == quote {
                        // A doubled quote is an escaped quote, not a terminator,
                        // and telling them apart needs the next byte.
                        if i + 1 >= bytes.len() && !at_end {
                            break;
                        }
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        self.state = State::Normal;
                    }
                    i += 1;
                }

                State::Bracketed => {
                    if c == b']' {
                        if i + 1 >= bytes.len() && !at_end {
                            break;
                        }
                        if bytes.get(i + 1) == Some(&b']') {
                            i += 2;
                            continue;
                        }
                        self.state = State::Normal;
                    }
                    i += 1;
                }

                State::LineComment => {
                    if c == b'\n' {
                        self.state = State::Normal;
                    }
                    i += 1;
                }

                State::BlockComment(depth) => {
                    let depth = *depth;
                    if i + 1 >= bytes.len() && !at_end {
                        break;
                    }
                    if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        self.state = State::BlockComment(depth + 1);
                        i += 2;
                    } else if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        self.state = if depth == 1 {
                            State::Normal
                        } else {
                            State::BlockComment(depth - 1)
                        };
                        i += 2;
                    } else {
                        i += 1;
                    }
                }

                State::DollarQuoted(tag) => {
                    if c == b'$' {
                        // The whole tag has to be present to match it.
                        if i + tag.len() > bytes.len() {
                            if !at_end {
                                break;
                            }
                            i += 1;
                            continue;
                        }
                        if self.buffer[i..].starts_with(tag.as_str()) {
                            let len = tag.len();
                            self.state = State::Normal;
                            i += len;
                            continue;
                        }
                    }
                    i += 1;
                }

                State::Normal => match c {
                    b'\'' | b'"' | b'`' => {
                        self.state = State::Quoted(c);
                        i += 1;
                    }
                    b'[' => {
                        self.state = State::Bracketed;
                        i += 1;
                    }
                    b'-' => {
                        if i + 1 >= bytes.len() && !at_end {
                            break;
                        }
                        if bytes.get(i + 1) == Some(&b'-') {
                            self.state = State::LineComment;
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    b'/' => {
                        if i + 1 >= bytes.len() && !at_end {
                            break;
                        }
                        if bytes.get(i + 1) == Some(&b'*') {
                            self.state = State::BlockComment(1);
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    b'$' => {
                        match dollar_tag_end(bytes, i) {
                            Some(tag_end) => {
                                let tag = self.buffer[i..tag_end].to_string();
                                self.state = State::DollarQuoted(tag);
                                i = tag_end;
                            }
                            // No closing `$` yet: either a parameter marker like
                            // `$1`, or a tag still arriving. Only the end of
                            // input can tell them apart.
                            None if at_end => i += 1,
                            None => break,
                        }
                    }
                    b';' if self.depth == 0 => {
                        let statement: String = self.buffer.drain(..i).collect();
                        // Drop the semicolon itself.
                        self.buffer.drain(..1);
                        push_statement(&mut out, &statement);
                        i = 0;
                    }
                    c if c.is_ascii_alphabetic() || c == b'_' => {
                        let word_start = i;
                        let mut end = i;
                        while end < bytes.len()
                            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                        {
                            end += 1;
                        }
                        // A word running to the end of the buffer may be half a
                        // word; `BEGI` must not be mistaken for anything.
                        if end == bytes.len() && !at_end {
                            break;
                        }

                        let word = &self.buffer[word_start..end];
                        if self.depth > 0 {
                            if word.eq_ignore_ascii_case("BEGIN")
                                || word.eq_ignore_ascii_case("CASE")
                            {
                                self.depth += 1;
                            } else if word.eq_ignore_ascii_case("END")
                                && closes_a_block(&self.buffer, end)
                            {
                                self.depth -= 1;
                            }
                        } else if opens_a_routine_body(&self.buffer[..word_start], word) {
                            self.depth = 1;
                        }
                        i = end;
                    }
                    _ => i += 1,
                },
            }
        }

        self.scanned = i.min(self.buffer.len());
        out
    }
}

/// Whether `BEGIN` at this point opens a routine body rather than a transaction.
///
/// `BEGIN;` on its own starts a transaction and is a statement in itself. The
/// same keyword inside `CREATE TRIGGER ... BEGIN` opens a body whose semicolons
/// belong to the body. The difference is entirely in what came before it.
fn opens_a_routine_body(prefix: &str, word: &str) -> bool {
    if !word.eq_ignore_ascii_case("BEGIN") {
        return false;
    }
    let upper = prefix.to_ascii_uppercase();
    let starts_a_definition = upper.trim_start().starts_with("CREATE")
        || upper.trim_start().starts_with("ALTER")
        || upper.trim_start().starts_with("REPLACE");
    starts_a_definition
        && (contains_word(&upper, "TRIGGER")
            || contains_word(&upper, "PROCEDURE")
            || contains_word(&upper, "FUNCTION"))
}

/// Whether an `END` closes a counted block.
///
/// `END IF`, `END LOOP`, `END WHILE`, and `END REPEAT` close constructs whose
/// openers are not counted — `IF` is also a MySQL function, so counting it would
/// break on `IF(a, b, c)`. Skipping their `END`s keeps the count balanced;
/// `END CASE` is not in the list because `CASE` *is* counted.
fn closes_a_block(sql: &str, after_end: usize) -> bool {
    let rest = sql[after_end..].trim_start();
    !["IF", "LOOP", "WHILE", "REPEAT"]
        .iter()
        .any(|kw| starts_with_word(rest, kw))
}

fn starts_with_word(text: &str, word: &str) -> bool {
    text.len() >= word.len()
        && text[..word.len()].eq_ignore_ascii_case(word)
        && !text[word.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Whole-word search in already-uppercased text.
fn contains_word(upper: &str, word: &str) -> bool {
    upper.match_indices(word).any(|(at, _)| {
        let before_ok = at == 0
            || !upper[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = at + word.len();
        let after_ok = !upper[after..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        before_ok && after_ok
    })
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
        // `SELECT * INTO new_table FROM t` creates a table and reads as a
        // SELECT until the third word. Every other statement that can contain
        // INTO is already on this list, so matching it costs nothing.
        "INTO",
    ];
    bare_words(sql)
        .into_iter()
        .any(|w| WRITE_KEYWORDS.contains(&w.to_ascii_uppercase().as_str()))
}

/// Identifier-like tokens that appear outside strings, comments, and quoted
/// identifiers. Used for keyword scanning where a match inside a literal would
/// be a false positive.
fn bare_words(sql: &str) -> Vec<&str> {
    bare_words_with_depth(sql)
        .into_iter()
        .map(|(word, _)| word)
        .collect()
}

/// The same tokens, each with the parenthesis nesting it appeared at.
///
/// Depth is what separates a clause that governs the statement from one that
/// governs a subquery: the `WHERE` in `DELETE FROM t WHERE id = 1` bounds the
/// delete, and the one in `DELETE FROM t` — with a `WHERE` only inside some
/// parenthesised expression — does not.
fn bare_words_with_depth(sql: &str) -> Vec<(&str, usize)> {
    let bytes = sql.as_bytes();
    let mut words = Vec::new();
    let mut depth = 0usize;
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
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
                {
                    i += 1;
                }
                words.push((&sql[start..i], depth));
            }
            _ => i += 1,
        }
    }
    words
}

/// What a statement would destroy, if anything.
///
/// This is the input to a confirmation gate, so it answers a narrower question
/// than [`looks_like_write`]: not "does this change the database" but "would
/// this lose something, and how much". An `INSERT` is a write and not a hazard;
/// a `DELETE` with no `WHERE` is the reason the gate exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hazard {
    /// What it would do, phrased for someone reading it under time pressure.
    pub summary: String,
    /// Whether it affects everything rather than a chosen subset.
    ///
    /// The distinction is the whole point. `DELETE FROM orders WHERE id = 5`
    /// and `DELETE FROM orders` are the same statement missing seven
    /// characters, and only one of them ends a career.
    pub unbounded: bool,
}

/// Words that sit between a keyword and the name it applies to.
const NOISE: &[&str] = &[
    "IF",
    "EXISTS",
    "TABLE",
    "ONLY",
    "FROM",
    "INTO",
    "CONCURRENTLY",
    "CASCADE",
];

/// What one statement would destroy.
///
/// Conservative in the direction that costs least: a `WHERE` this cannot see as
/// top-level is treated as absent, so an unusual statement is more likely to be
/// flagged than waved through. A needless confirmation costs a click.
pub fn hazard(statement: &str) -> Option<Hazard> {
    let words = bare_words_with_depth(statement);
    let top: Vec<&str> = words
        .iter()
        .filter(|(_, depth)| *depth == 0)
        .map(|(word, _)| *word)
        .collect();

    let first = top.first()?.to_ascii_uppercase();
    // A top-level WHERE is what bounds a statement; one inside parentheses
    // belongs to a subquery and bounds nothing.
    let bounded = top.iter().any(|w| w.eq_ignore_ascii_case("WHERE"));
    let name = |after: usize| -> Option<String> {
        top.iter()
            .skip(after)
            .find(|w| !NOISE.iter().any(|n| w.eq_ignore_ascii_case(n)))
            .map(|w| w.to_string())
    };

    match first.as_str() {
        "DROP" => {
            let kind = top.get(1).map(|k| k.to_ascii_lowercase())?;
            let what = name(2).map(|n| format!(" {n}")).unwrap_or_default();
            Some(Hazard {
                summary: format!("drops the {kind}{what} and everything in it"),
                unbounded: true,
            })
        }

        "TRUNCATE" => Some(Hazard {
            summary: match name(1) {
                Some(table) => format!("empties {table} completely"),
                None => "empties the table completely".into(),
            },
            unbounded: true,
        }),

        "DELETE" => Some(Hazard {
            summary: match (name(1), bounded) {
                (Some(table), true) => format!("deletes matching rows from {table}"),
                (Some(table), false) => format!("deletes every row in {table} — there is no WHERE"),
                (None, true) => "deletes matching rows".into(),
                (None, false) => "deletes every row — there is no WHERE".into(),
            },
            unbounded: !bounded,
        }),

        "UPDATE" => Some(Hazard {
            summary: match (name(1), bounded) {
                (Some(table), true) => format!("updates matching rows in {table}"),
                (Some(table), false) => {
                    format!("updates every row in {table} — there is no WHERE")
                }
                (None, true) => "updates matching rows".into(),
                (None, false) => "updates every row — there is no WHERE".into(),
            },
            unbounded: !bounded,
        }),

        "ALTER" => {
            // Only the destructive half of ALTER. Adding a column is a write
            // and loses nothing; dropping one loses a column of data.
            let drops_column = top.windows(2).any(|pair| {
                pair[0].eq_ignore_ascii_case("DROP") && pair[1].eq_ignore_ascii_case("COLUMN")
            });
            drops_column.then(|| Hazard {
                summary: match name(2) {
                    Some(table) => format!("drops a column from {table}, and its values with it"),
                    None => "drops a column, and its values with it".into(),
                },
                unbounded: true,
            })
        }

        // MERGE always has an ON clause, so it is bounded by construction — but
        // it can delete, so it is still worth confirming.
        "MERGE" => Some(Hazard {
            summary: match name(1) {
                Some(table) => format!("merges into {table}, which may delete rows"),
                None => "merges rows, which may delete some".into(),
            },
            unbounded: false,
        }),

        _ => None,
    }
}

/// Every hazard in a submission, in the order the statements would run.
pub fn hazards(sql: &str) -> Vec<Hazard> {
    split_statements(sql)
        .iter()
        .filter_map(|s| hazard(s))
        .collect()
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

    #[test]
    fn a_delete_without_a_where_is_the_case_the_gate_exists_for() {
        // Seven characters apart, and only one of them ends a career.
        let bounded = hazard("DELETE FROM orders WHERE id = 5").expect("a delete is a hazard");
        assert!(!bounded.unbounded);
        assert!(
            bounded.summary.contains("matching rows"),
            "{}",
            bounded.summary
        );

        let unbounded = hazard("DELETE FROM orders").expect("a delete is a hazard");
        assert!(unbounded.unbounded);
        assert!(
            unbounded.summary.contains("every row"),
            "{}",
            unbounded.summary
        );
        assert!(
            unbounded.summary.contains("orders"),
            "{}",
            unbounded.summary
        );
    }

    #[test]
    fn an_update_without_a_where_is_flagged_the_same_way() {
        assert!(
            hazard("UPDATE users SET active = false")
                .expect("hazard")
                .unbounded
        );
        assert!(
            !hazard("UPDATE users SET active = false WHERE id = 1")
                .expect("hazard")
                .unbounded
        );
    }

    #[test]
    fn a_where_inside_a_subquery_does_not_bound_the_statement() {
        // It governs the subquery. Treating it as a bound is how an unbounded
        // delete gets waved through by a scanner that only looks for the word.
        let h = hazard("DELETE FROM orders WHERE user_id IN (SELECT id FROM users WHERE active)")
            .expect("hazard");
        assert!(!h.unbounded, "a real top-level WHERE was missed");

        let h = hazard("UPDATE t SET x = (SELECT max(y) FROM u WHERE u.id = 1)").expect("hazard");
        assert!(
            h.unbounded,
            "a subquery's WHERE bounded the update: {}",
            h.summary
        );
    }

    #[test]
    fn a_where_in_a_comment_or_a_string_bounds_nothing() {
        assert!(
            hazard("DELETE FROM t -- WHERE id = 1")
                .expect("hazard")
                .unbounded
        );
        assert!(
            hazard("DELETE FROM t /* WHERE id = 1 */")
                .expect("hazard")
                .unbounded
        );
        assert!(
            hazard("UPDATE t SET note = 'where'")
                .expect("hazard")
                .unbounded
        );
    }

    #[test]
    fn drop_and_truncate_are_always_unbounded() {
        let drop = hazard("DROP TABLE orders").expect("hazard");
        assert!(drop.unbounded);
        assert!(drop.summary.contains("orders"), "{}", drop.summary);

        assert!(hazard("DROP TABLE IF EXISTS orders")
            .expect("hazard")
            .summary
            .contains("orders"));
        assert!(hazard("TRUNCATE TABLE orders").expect("hazard").unbounded);
        assert!(hazard("DROP DATABASE app")
            .expect("hazard")
            .summary
            .contains("database"));
    }

    #[test]
    fn adding_a_column_is_not_a_hazard_and_dropping_one_is() {
        // A write that loses nothing does not need a gate; the gate is for
        // losing things, and a gate that fires on everything gets clicked
        // through without reading.
        assert_eq!(hazard("ALTER TABLE users ADD COLUMN nickname TEXT"), None);

        let dropped = hazard("ALTER TABLE users DROP COLUMN nickname").expect("hazard");
        assert!(dropped.unbounded);
        assert!(dropped.summary.contains("users"), "{}", dropped.summary);
    }

    #[test]
    fn reads_and_harmless_writes_are_not_hazards() {
        for sql in [
            "SELECT * FROM users",
            "INSERT INTO users (id) VALUES (1)",
            "CREATE TABLE t (id INTEGER)",
            "CREATE INDEX ix ON t (id)",
            "EXPLAIN SELECT * FROM users",
        ] {
            assert_eq!(hazard(sql), None, "{sql}");
        }
    }

    #[test]
    fn every_statement_in_a_submission_is_examined() {
        // The dangerous one is rarely the first, and a gate that reads only the
        // first statement is a gate that misses it.
        let found = hazards("SELECT 1; INSERT INTO t VALUES (1); DELETE FROM t; SELECT 2");
        assert_eq!(found.len(), 1);
        assert!(found[0].unbounded);
    }

    #[test]
    fn a_merge_is_confirmed_but_not_called_unbounded() {
        // It always has an ON clause, so it is bounded by construction — and it
        // can still delete, so it is still worth a look.
        let h = hazard("MERGE INTO target USING source ON target.id = source.id").expect("hazard");
        assert!(!h.unbounded);
        assert!(h.summary.contains("target"), "{}", h.summary);
    }
    #[test]
    fn select_into_creates_a_table_and_is_not_a_read() {
        // It reads as a SELECT until the third word, which is exactly why a
        // first-keyword check is not enough.
        assert!(looks_like_write("SELECT * INTO archive FROM orders"));
        // And an ordinary select still reads as one.
        assert!(!looks_like_write("SELECT into_date FROM t"));
        assert!(!looks_like_write("SELECT 'into' FROM t"));
    }
    use super::*;

    /// Feed the same SQL one byte at a time, which puts a chunk boundary in
    /// every position a token could straddle.
    fn split_byte_by_byte(sql: &str) -> Vec<String> {
        let mut splitter = Splitter::new();
        let mut out = Vec::new();
        for ch in sql.chars() {
            out.extend(splitter.push(&ch.to_string()));
        }
        out.extend(splitter.finish());
        out
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        // Every construct the scanner tracks, in one string: quotes with
        // doubling and backslashes, both comment kinds, a bracket identifier, a
        // dollar-quoted body, and a routine body with its own semicolons.
        let sql = "SELECT 'a;b', \"c;d\", `e;f`, [g;h]; \
                   -- a; comment\n\
                   /* nested /* b; */ still */ SELECT 1; \
                   CREATE TRIGGER t AFTER UPDATE ON u BEGIN SELECT 1; END; \
                   CREATE FUNCTION f() RETURNS int AS $$ BEGIN; RETURN 1; END; $$ LANGUAGE plpgsql; \
                   SELECT 'it''s'; SELECT 'back\\\\slash'";

        let whole = split_statements(sql);
        assert_eq!(
            split_byte_by_byte(sql),
            whole,
            "a chunk boundary must not change how anything is read"
        );
        // And the whole-input answer is itself right. Six, not seven: a
        // comment belongs to the statement that follows it rather than
        // standing as one of its own.
        assert_eq!(whole.len(), 6, "{whole:?}");
    }

    #[test]
    fn a_statement_split_across_chunks_arrives_once_and_whole() {
        let mut splitter = Splitter::new();
        assert!(splitter.push("SELECT 'a").is_empty(), "no terminator yet");
        assert!(
            splitter.push("b;c' AS x").is_empty(),
            "the ; is inside a string"
        );
        let done = splitter.push("; SELECT 2");
        assert_eq!(done, vec!["SELECT 'ab;c' AS x".to_string()]);
        assert_eq!(splitter.finish(), vec!["SELECT 2".to_string()]);
    }

    #[test]
    fn the_buffer_does_not_grow_with_the_file() {
        // What makes importing a large dump possible: statements are handed out
        // and dropped, so what is held is one statement, not the file.
        let mut splitter = Splitter::new();
        for _ in 0..1_000 {
            let done = splitter.push("INSERT INTO t VALUES (1);");
            assert_eq!(done.len(), 1);
        }
        assert!(
            splitter.buffered() < 64,
            "held {} bytes after 1000 statements",
            splitter.buffered()
        );
    }

    #[test]
    fn a_trailing_statement_without_a_semicolon_is_not_lost() {
        let mut splitter = Splitter::new();
        assert!(splitter.push("SELECT 1").is_empty());
        assert_eq!(splitter.finish(), vec!["SELECT 1".to_string()]);
    }

    #[test]
    fn finishing_twice_yields_nothing_the_second_time() {
        let mut splitter = Splitter::new();
        splitter.push("SELECT 1");
        assert_eq!(splitter.finish().len(), 1);
        assert!(
            splitter.finish().is_empty(),
            "the buffer was already flushed"
        );
    }

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
    fn a_trigger_body_is_one_statement() {
        // SQLite, MySQL, and SQL Server all write bodies this way, with no
        // dollar quoting to lean on. Splitting inside the body sends `END` to
        // the server on its own, which is a syntax error and leaves a
        // half-created trigger behind.
        let sql = "CREATE TRIGGER t AFTER UPDATE ON users BEGIN \
                   UPDATE audit SET n = n + 1; DELETE FROM tmp; END; SELECT 1";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2, "got {parts:?}");
        assert!(parts[0].ends_with("END"), "got {:?}", parts[0]);
        assert_eq!(parts[1], "SELECT 1");
    }

    #[test]
    fn a_case_expression_inside_a_body_does_not_end_it_early() {
        // CASE ... END is counted, or its END would close the body and the rest
        // of the trigger would be sent as separate statements.
        let sql = "CREATE TRIGGER t AFTER INSERT ON users BEGIN \
                   UPDATE a SET x = CASE WHEN 1 THEN 2 ELSE 3 END; DELETE FROM b; END; SELECT 1";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2, "got {parts:?}");
        assert!(parts[0].contains("DELETE FROM b"), "got {:?}", parts[0]);
    }

    #[test]
    fn end_if_does_not_close_the_body() {
        // MySQL's IF block closes with `END IF`, and its opener is not counted —
        // `IF(a,b,c)` is also a function call, so counting `IF` would break far
        // more than it fixed.
        let sql = "CREATE PROCEDURE p() BEGIN IF 1 THEN SELECT 1; END IF; SELECT 2; END; SELECT 3";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2, "got {parts:?}");
        assert!(parts[0].contains("SELECT 2"), "got {:?}", parts[0]);
        assert_eq!(parts[1], "SELECT 3");
    }

    #[test]
    fn a_bare_begin_still_starts_a_transaction_statement() {
        // `BEGIN;` on its own is a statement, not the start of a body. Treating
        // it as one would swallow the entire rest of the submission.
        let sql = "BEGIN; UPDATE t SET a = 1; COMMIT;";
        assert_eq!(
            split_statements(sql),
            vec!["BEGIN", "UPDATE t SET a = 1", "COMMIT"]
        );
    }

    #[test]
    fn a_column_named_begin_does_not_open_a_body() {
        let sql = "SELECT begin_at FROM t; SELECT 2";
        assert_eq!(
            split_statements(sql),
            vec!["SELECT begin_at FROM t", "SELECT 2"]
        );
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
