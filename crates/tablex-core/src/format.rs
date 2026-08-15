//! Pretty-printing SQL.
//!
//! This is a formatter, not a parser. It tokenizes with the same lexical rules
//! the splitter uses — strings, comments, quoted identifiers, dollar-quoted
//! bodies — and then re-emits those tokens with line breaks and indentation.
//! It never reorders, adds, or removes anything, which is the property the
//! tests hold it to: the token stream before and after must be identical apart
//! from whitespace.
//!
//! Not parsing is a deliberate limit. A real parser would let it align
//! expressions and wrap long conditions intelligently; it would also have to
//! know five dialects' grammars, and be wrong on the sixth. Re-emitting tokens
//! is correct everywhere and useful almost everywhere.

use crate::sql::split_statements;

/// Clause keywords that begin a new line at the top level.
const CLAUSES: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "HAVING",
    "WINDOW",
    "VALUES",
    "SET",
    "RETURNING",
    "LIMIT",
    "OFFSET",
    "FETCH",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "WITH",
];

/// Two-word clause openers, matched as a pair so `GROUP` alone does not break.
const CLAUSE_PAIRS: &[(&str, &str)] = &[("GROUP", "BY"), ("ORDER", "BY"), ("PARTITION", "BY")];

/// Join keywords, which start a line and read better slightly indented.
const JOINS: &[&str] = &["JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL"];

/// Words uppercased wherever they appear outside strings and identifiers.
const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASCADE",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT",
    "DATABASE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DO",
    "DROP",
    "ELSE",
    "END",
    "ESCAPE",
    "EXCEPT",
    "EXISTS",
    "FALSE",
    "FETCH",
    "FILTER",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "ILIKE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NATURAL",
    "NOT",
    "NULL",
    "NULLS",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "PARTITION",
    "PRIMARY",
    "PROCEDURE",
    "REFERENCES",
    "RETURNING",
    "RIGHT",
    "ROLLBACK",
    "SCHEMA",
    "SELECT",
    "SET",
    "TABLE",
    "THEN",
    "TRIGGER",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
];

/// Statements shorter than this keep to one line.
///
/// `SELECT id FROM users` broken across four lines is not clearer than
/// `SELECT id FROM users`, and a formatter that insists otherwise gets turned
/// off.
const INLINE_WIDTH: usize = 72;

const INDENT: &str = "  ";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word: keyword, identifier, or number.
    Word(String),
    /// A quoted string, quoted identifier, or dollar-quoted body — verbatim.
    Literal(String),
    Comment(String),
    Punct(char),
    /// An operator run such as `<=`, `||`, `::`.
    Operator(String),
}

impl Token {
    fn text(&self) -> &str {
        match self {
            Token::Word(s) | Token::Literal(s) | Token::Comment(s) | Token::Operator(s) => s,
            Token::Punct(_) => "",
        }
    }

    /// The word this token is, uppercased, for keyword tests.
    fn keyword(&self) -> Option<String> {
        match self {
            Token::Word(s) if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => {
                Some(s.to_ascii_uppercase())
            }
            _ => None,
        }
    }
}

/// Format one or more statements.
///
/// Statements are separated by a blank line, which is how a script reads when
/// each statement is a paragraph.
pub fn format_sql(sql: &str) -> String {
    let statements = split_statements(sql);
    if statements.is_empty() {
        return String::new();
    }

    statements
        .iter()
        .map(|statement| format_one(statement))
        .collect::<Vec<_>>()
        .join(";\n\n")
        + ";"
}

fn format_one(sql: &str) -> String {
    let tokens = tokenize(sql);
    if tokens.is_empty() {
        return String::new();
    }

    let inline = render_inline(&tokens);
    // A comment forces the multi-line form: everything after it on one line
    // would be swallowed by a `--`.
    let has_comment = tokens.iter().any(|t| matches!(t, Token::Comment(_)));
    if !has_comment && inline.len() <= INLINE_WIDTH {
        return inline;
    }

    render_block(&tokens)
}

/// One line, normalized spacing and keyword case. Also the width probe.
fn render_inline(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i > 0 && needs_space(&tokens[i - 1], token) {
            out.push(' ');
        }
        out.push_str(&emit(token));
    }
    out
}

fn render_block(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut line_open = false;

    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        let word = token.keyword();

        // Two-word clauses are matched before single words so `GROUP BY` breaks
        // once rather than twice.
        let pair = word.as_deref().and_then(|w| {
            tokens.get(i + 1).and_then(|next| {
                next.keyword().and_then(|n| {
                    CLAUSE_PAIRS
                        .iter()
                        .find(|(a, b)| *a == w && *b == n)
                        .map(|_| format!("{w} {n}"))
                })
            })
        });

        let starts_line = depth == 0
            && line_open
            && (pair.is_some()
                || word.as_deref().is_some_and(|w| CLAUSES.contains(&w))
                || word.as_deref().is_some_and(|w| JOINS.contains(&w)));

        // AND and OR read as a list when each sits at the start of its line.
        let starts_condition =
            line_open && word.as_deref().is_some_and(|w| w == "AND" || w == "OR");

        if let Token::Comment(_) = token {
            if line_open {
                out.push('\n');
                out.push_str(&INDENT.repeat(depth));
            }
            out.push_str(token.text());
            out.push('\n');
            out.push_str(&INDENT.repeat(depth));
            line_open = false;
            i += 1;
            continue;
        }

        if starts_line || starts_condition {
            out.push('\n');
            let extra = usize::from(
                starts_condition || word.as_deref().is_some_and(|w| JOINS.contains(&w)),
            );
            out.push_str(&INDENT.repeat(depth + extra));
        } else if line_open && needs_space(&tokens[i - 1], token) {
            out.push(' ');
        }

        if let Some(pair) = pair {
            out.push_str(&pair);
            line_open = true;
            i += 2;
            continue;
        }

        match token {
            Token::Punct('(') => {
                out.push('(');
                depth += 1;
            }
            Token::Punct(')') => {
                depth = depth.saturating_sub(1);
                out.push(')');
            }
            // A comma ends a line inside a list; the next token opens the next.
            Token::Punct(',') => {
                out.push(',');
                out.push('\n');
                out.push_str(&INDENT.repeat(depth + 1));
                line_open = false;
                i += 1;
                continue;
            }
            _ => out.push_str(&emit(token)),
        }

        line_open = true;
        i += 1;
    }

    out.trim_end().to_string()
}

fn emit(token: &Token) -> String {
    match token {
        Token::Word(word) => {
            let upper = word.to_ascii_uppercase();
            // Only alphabetic words are candidates; a number is left alone.
            if word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && KEYWORDS.contains(&upper.as_str())
            {
                upper
            } else {
                word.clone()
            }
        }
        Token::Punct(c) => c.to_string(),
        other => other.text().to_string(),
    }
}

/// Whether two adjacent tokens need a space between them.
fn needs_space(previous: &Token, next: &Token) -> bool {
    match (previous, next) {
        // Nothing hugs an opening bracket from the inside, and a closing one
        // takes nothing before it.
        (Token::Punct('('), _) => false,
        (_, Token::Punct(')')) => false,
        (_, Token::Punct(',')) => false,
        (_, Token::Punct(';')) => false,
        (Token::Punct('.'), _) | (_, Token::Punct('.')) => false,
        // A function call is `count(` rather than `count (`.
        (Token::Word(_), Token::Punct('(')) => false,
        (Token::Operator(op), _) if op == "::" => false,
        (_, Token::Operator(op)) if op == "::" => false,
        _ => true,
    }
}

/// Split SQL into tokens, keeping literals and comments byte-for-byte.
fn tokenize(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];

        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Comments, before the operator scan claims the `-` or `/`.
        if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(Token::Comment(sql[start..i].trim_end().to_string()));
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let start = i;
            let mut depth = 1;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            out.push(Token::Comment(sql[start..i].to_string()));
            continue;
        }

        // Quoted things travel verbatim: their contents are data, not syntax.
        if matches!(c, b'\'' | b'"' | b'`' | b'[') {
            let closing = if c == b'[' { b']' } else { c };
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && c == b'\'' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == closing {
                    if bytes.get(i + 1) == Some(&closing) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Token::Literal(sql[start..i].to_string()));
            continue;
        }

        if c == b'$' {
            if let Some(tag_end) = dollar_tag_end(bytes, i) {
                let tag = &sql[i..tag_end];
                let start = i;
                i = tag_end;
                while i < bytes.len() {
                    if bytes[i] == b'$' && sql[i..].starts_with(tag) {
                        i += tag.len();
                        break;
                    }
                    i += 1;
                }
                out.push(Token::Literal(sql[start..i].to_string()));
                continue;
            }
        }

        if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'@' || c == b'#' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'_' | b'$' | b'@' | b'#'))
            {
                i += 1;
            }
            // A decimal point inside a number belongs to the number.
            if bytes.get(i) == Some(&b'.')
                && sql[start..i].chars().all(|c| c.is_ascii_digit())
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
            {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            out.push(Token::Word(sql[start..i].to_string()));
            continue;
        }

        if matches!(c, b'(' | b')' | b',' | b';' | b'.') {
            out.push(Token::Punct(c as char));
            i += 1;
            continue;
        }

        // Everything else is an operator run: `<=`, `||`, `::`, `->>`.
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_alphanumeric()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(
                bytes[i],
                b'(' | b')' | b',' | b';' | b'.' | b'_' | b'\'' | b'"' | b'`' | b'['
            )
        {
            i += 1;
        }
        out.push(Token::Operator(sql[start..i].to_string()));
    }

    out
}

/// If a dollar-quote tag starts at `i`, the index just past it.
fn dollar_tag_end(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'$' => return Some(j + 1),
            c if c.is_ascii_alphanumeric() || c == b'_' => j += 1,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens with whitespace and keyword case normalized away, which is the
    /// invariant every format must preserve.
    ///
    /// A trailing semicolon is dropped: terminating each statement is a thing
    /// the formatter deliberately does, and it is the one difference allowed.
    fn shape(sql: &str) -> Vec<String> {
        let mut tokens: Vec<String> = tokenize(sql)
            .iter()
            .map(|t| match t {
                Token::Word(w) => w.to_ascii_uppercase(),
                Token::Punct(c) => c.to_string(),
                other => other.text().to_string(),
            })
            .collect();
        if tokens.last().is_some_and(|t| t == ";") {
            tokens.pop();
        }
        tokens
    }

    #[test]
    fn a_short_statement_stays_on_one_line() {
        // Four lines is not clearer than one, and a formatter that insists
        // otherwise is one people turn off.
        assert_eq!(format_sql("select id from users"), "SELECT id FROM users;");
    }

    #[test]
    fn keywords_are_uppercased_and_identifiers_are_not() {
        assert_eq!(
            format_sql("select Id, userName from Users where Id = 1"),
            "SELECT Id, userName FROM Users WHERE Id = 1;"
        );
    }

    #[test]
    fn a_long_statement_breaks_at_its_clauses() {
        let out = format_sql(
            "select id, email, created_at from customers where active = true and country = 'KE' order by created_at desc",
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("SELECT"), "{out}");
        assert!(lines.iter().any(|l| l.starts_with("FROM")), "{out}");
        assert!(lines.iter().any(|l| l.starts_with("WHERE")), "{out}");
        // ORDER BY breaks once, as a pair, not twice.
        assert!(lines.iter().any(|l| l.starts_with("ORDER BY")), "{out}");
        assert!(!lines.iter().any(|l| l.trim() == "BY"), "{out}");
        // Conditions line up under each other rather than running on.
        assert!(
            lines.iter().any(|l| l.trim_start().starts_with("AND")),
            "{out}"
        );
    }

    #[test]
    fn formatting_never_changes_the_statement() {
        // The property that matters: only whitespace and keyword case move.
        let inputs = [
            "select a,b from t where x>1 and y<=2 order by a desc limit 10",
            "insert into t (a, b) values (1, 'two'), (3, 'four')",
            "update t set a = a + 1 where id in (select id from other where flag)",
            "select count(*), max(price::numeric) from orders group by customer_id having count(*) > 3",
        ];
        for input in inputs {
            let formatted = format_sql(input);
            assert_eq!(shape(input), shape(&formatted), "{input}\n---\n{formatted}");
        }
    }

    #[test]
    fn formatting_twice_changes_nothing_the_second_time() {
        let sql = "select id, email, created_at from customers where active = true and country = 'KE' order by created_at desc";
        let once = format_sql(sql);
        assert_eq!(format_sql(&once), once);
    }

    #[test]
    fn string_contents_are_left_alone() {
        // A keyword inside a string is data. Uppercasing it would change what
        // the query means.
        let out = format_sql("select * from t where name = 'select from where'");
        assert!(out.contains("'select from where'"), "{out}");
    }

    #[test]
    fn quoted_identifiers_keep_their_case() {
        let out = format_sql(r#"select "Order", `group`, [Select] from t"#);
        assert!(out.contains(r#""Order""#), "{out}");
        assert!(out.contains("`group`"), "{out}");
        assert!(out.contains("[Select]"), "{out}");
    }

    #[test]
    fn a_comment_forces_the_block_form_and_survives_intact() {
        // On one line, everything after a `--` would be commented out.
        let out = format_sql("select 1 -- why\n, 2");
        assert!(out.contains("-- why"), "{out}");
        assert!(out.lines().count() > 1, "{out}");
    }

    #[test]
    fn a_dollar_quoted_body_is_not_reformatted() {
        let body = "$$ BEGIN; select 1; END; $$";
        let out = format_sql(&format!(
            "create function f() returns int as {body} language plpgsql"
        ));
        assert!(out.contains(body), "{out}");
    }

    #[test]
    fn several_statements_are_separated_by_a_blank_line() {
        let out = format_sql("select 1; select 2");
        assert_eq!(out, "SELECT 1;\n\nSELECT 2;");
    }

    #[test]
    fn function_calls_do_not_gain_a_space() {
        assert_eq!(
            format_sql("select count(*) from t"),
            "SELECT count(*) FROM t;"
        );
    }

    #[test]
    fn decimals_are_one_token_rather_than_three() {
        // `1.5` split at the point would be re-emitted as `1 . 5`.
        assert_eq!(format_sql("select 1.5 from t"), "SELECT 1.5 FROM t;");
    }

    #[test]
    fn empty_input_produces_nothing() {
        assert_eq!(format_sql("   "), "");
        assert_eq!(format_sql(""), "");
    }
}
