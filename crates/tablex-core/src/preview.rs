//! A statement that would write, rewritten as a `SELECT` that reads the same
//! rows.
//!
//! `UPDATE orders SET status = 'shipped' WHERE id = 5` becomes a query for the
//! rows it would touch, with `status` shown again as it would become. `DELETE`
//! becomes the rows that would go; `INSERT … VALUES` becomes the rows that
//! would arrive. Running the preview costs a read and writes nothing, which is
//! the point: the statement can be checked against real rows before the one
//! that cannot be taken back.
//!
//! The rewrite works on the statement's own text. The expressions are copied
//! verbatim, comments and spacing included, into a shape every engine here
//! accepts; nothing is parsed further than the clause boundaries. That keeps
//! the preview honest about what it is — the engine evaluates the same
//! expressions the write would — and it means a statement this cannot place
//! with confidence is refused rather than approximated. `UPDATE … FROM`, a
//! multi-table `DELETE`, `ON CONFLICT`, `RETURNING`: each is declined with a
//! reason, and the statement can still be run.
//!
//! One thing the preview cannot promise: an expression with a side effect or
//! a moving answer — `nextval`, `now()`, `rand()` — is evaluated now, and
//! evaluated again by the write, so the value shown may not be the one stored.

use crate::error::{Error, Result};
use crate::sql::{quote_ident, scan, split_statements, Item, ItemKind};
use serde::Serialize;

/// What the target engine needs the rewrite to look like.
#[derive(Debug, Clone, Copy)]
pub struct Dialect {
    /// The identifier quote, for the `column (new)` aliases.
    pub quote: char,
    /// Oracle's two demands: a `SELECT` with no table needs `FROM DUAL`, and
    /// `*` cannot be followed by more columns unless it is qualified.
    pub oracle: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Update,
    Insert,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub kind: Kind,
    /// The statement to run instead.
    pub select: String,
    /// What the rows are, phrased for the strip above them.
    pub note: String,
}

/// The alias given to the table when the engine needs one and the statement
/// did not supply it. Unlikely to collide with a real column, and visible in
/// the preview only as `preview_.*`.
const ALIAS: &str = "preview_";

fn unsupported(why: &str) -> Error {
    Error::Unsupported(format!("No preview for this statement: {why}"))
}

/// Rewrite one writing statement as a read.
pub fn preview(sql: &str, dialect: Dialect) -> Result<Preview> {
    let statements = split_statements(sql);
    let statement = match statements.as_slice() {
        [one] => one.trim().trim_end_matches(';').trim(),
        [] => return Err(unsupported("there is nothing to preview")),
        _ => return Err(unsupported("preview one statement at a time")),
    };

    let items = scan(statement);
    let top: Vec<Item> = items.iter().copied().filter(|i| i.depth == 0).collect();
    let first = top
        .first()
        .and_then(|i| word(i))
        .ok_or_else(|| unsupported("it does not start with a keyword"))?;

    match first.as_str() {
        "UPDATE" => update(statement, &top, dialect),
        "DELETE" => delete(statement, &top),
        "INSERT" => insert(statement, &top, dialect),
        _ => Err(unsupported("only UPDATE, INSERT and DELETE have one")),
    }
}

/// The uppercased word an item is, if it is one.
fn word(item: &Item) -> Option<String> {
    match item.kind {
        ItemKind::Word(w) => Some(w.to_ascii_uppercase()),
        _ => None,
    }
}

/// Index in `top` of the first `keyword` at or after `from`.
fn find(top: &[Item], keyword: &str, from: usize) -> Option<usize> {
    top.iter()
        .enumerate()
        .skip(from)
        .find(|(_, item)| word(item).as_deref() == Some(keyword))
        .map(|(i, _)| i)
}

/// The first of `keywords` present at or after `from`, for refusing shapes
/// this does not handle.
fn any_of<'k>(top: &[Item], keywords: &[&'k str], from: usize) -> Option<&'k str> {
    top.iter()
        .skip(from)
        .find_map(|item| word(item).and_then(|w| keywords.iter().find(|k| **k == w).copied()))
}

/// A table reference as `UPDATE` and `DELETE` write it: a name, perhaps an
/// alias, and nothing this cannot reuse as-is in a `FROM`.
struct TableRef<'a> {
    text: &'a str,
    alias: Option<&'a str>,
}

fn table_ref(text: &str) -> Result<TableRef<'_>> {
    let text = text.trim();
    let parts: Vec<&str> = text.split_whitespace().collect();
    let alias = match parts.as_slice() {
        [] => return Err(unsupported("no table is named")),
        [_] => None,
        [_, alias] => Some(*alias),
        [_, as_, alias] if as_.eq_ignore_ascii_case("AS") => Some(*alias),
        _ => {
            return Err(unsupported(
                "the table reference is more than a name and an alias",
            ))
        }
    };
    // Modifiers that are legal here and not in a FROM.
    if parts.first().is_some_and(|p| {
        ["ONLY", "LOW_PRIORITY", "IGNORE", "TOP", "QUICK"]
            .contains(&p.to_ascii_uppercase().as_str())
    }) {
        return Err(unsupported("the table reference carries a modifier"));
    }
    Ok(TableRef { text, alias })
}

/// `col`, `"col"`, `` `col` ``, `[col]` or `t.col`, as the bare name.
fn bare_name(text: &str) -> String {
    let last = text.rsplit('.').next().unwrap_or(text).trim();
    last.trim_matches(|c| matches!(c, '"' | '`' | '[' | ']'))
        .replace("\"\"", "\"")
}

/// Pieces of `sql[start..end]` split at the top-level commas within it.
fn split_at_commas<'a>(sql: &'a str, items: &[Item], start: usize, end: usize) -> Vec<&'a str> {
    let mut pieces = Vec::new();
    let mut from = start;
    for item in items {
        if item.kind == ItemKind::Comma && item.depth == 0 && item.start >= start && item.end <= end
        {
            pieces.push(&sql[from..item.start]);
            from = item.end;
        }
    }
    pieces.push(&sql[from..end]);
    pieces
}

fn update(sql: &str, top: &[Item], dialect: Dialect) -> Result<Preview> {
    let set = find(top, "SET", 1).ok_or_else(|| unsupported("it has no SET"))?;
    if let Some(k) = any_of(
        top,
        &["JOIN", "RETURNING", "ORDER", "LIMIT", "OUTPUT", "WITH"],
        1,
    ) {
        return Err(unsupported(&format!("it uses {k}")));
    }
    if find(top, "FROM", set).is_some() {
        return Err(unsupported("it updates from another table"));
    }
    if top[1..set].iter().any(|i| i.kind == ItemKind::Comma) {
        return Err(unsupported("it updates more than one table"));
    }

    let table = table_ref(&sql[top[0].end..top[set].start])?;
    let where_at = find(top, "WHERE", set + 1);
    let assignments_end = where_at.map_or(sql.len(), |w| top[w].start);
    let where_text = where_at.map(|w| &sql[top[w].start..]);

    let mut shown = Vec::new();
    for assignment in split_at_commas(sql, top, top[set].end, assignments_end) {
        let assignment = assignment.trim();
        let offset = assignment.as_ptr() as usize - sql.as_ptr() as usize;
        let equals = top
            .iter()
            .find(|i| {
                i.kind == ItemKind::Equals
                    && i.start >= offset
                    && i.end <= offset + assignment.len()
            })
            .ok_or_else(|| unsupported("an assignment has no ="))?;
        let column = sql[offset..equals.start].trim();
        let expr = sql[equals.end..offset + assignment.len()].trim();
        if column.starts_with('(') {
            return Err(unsupported("it assigns a row of columns at once"));
        }
        if expr.eq_ignore_ascii_case("DEFAULT") {
            return Err(unsupported(
                "it assigns a column its default, which only the engine knows",
            ));
        }
        let label = format!("{} (new)", bare_name(column));
        shown.push(format!(
            "({expr}) AS {}",
            quote_ident(&label, dialect.quote)
        ));
    }
    if shown.is_empty() {
        return Err(unsupported("it assigns nothing"));
    }

    // Oracle will not take `*, expr`; it wants the star qualified, which
    // means the table needs a name to qualify it with.
    let (star, from) = if dialect.oracle {
        match table.alias {
            Some(alias) => (format!("{alias}.*"), table.text.to_string()),
            None => (format!("{ALIAS}.*"), format!("{} {ALIAS}", table.text)),
        }
    } else {
        ("*".to_string(), table.text.to_string())
    };

    let mut select = format!("SELECT {star}, {} FROM {from}", shown.join(", "));
    if let Some(w) = where_text {
        select.push(' ');
        select.push_str(w.trim());
    }
    Ok(Preview {
        kind: Kind::Update,
        select,
        note:
            "The rows the UPDATE would change, each assigned column shown again as it would become."
                .into(),
    })
}

fn delete(sql: &str, top: &[Item]) -> Result<Preview> {
    if top.get(1).and_then(word).as_deref() != Some("FROM") {
        return Err(unsupported("it is not DELETE FROM one table"));
    }
    if let Some(k) = any_of(
        top,
        &[
            "USING",
            "JOIN",
            "RETURNING",
            "ORDER",
            "LIMIT",
            "OUTPUT",
            "TOP",
            "WITH",
        ],
        2,
    ) {
        return Err(unsupported(&format!("it uses {k}")));
    }
    let where_at = find(top, "WHERE", 2);
    let table_end = where_at.map_or(sql.len(), |w| top[w].start);
    if top[2..]
        .iter()
        .any(|i| i.kind == ItemKind::Comma && i.start < table_end)
    {
        return Err(unsupported("it deletes from more than one table"));
    }
    let table = table_ref(&sql[top[1].end..table_end])?;

    let mut select = format!("SELECT * FROM {}", table.text);
    if let Some(w) = where_at {
        select.push(' ');
        select.push_str(sql[top[w].start..].trim());
    }
    Ok(Preview {
        kind: Kind::Delete,
        select,
        note: "The rows the DELETE would remove.".into(),
    })
}

fn insert(sql: &str, top: &[Item], dialect: Dialect) -> Result<Preview> {
    if top.get(1).and_then(word).as_deref() != Some("INTO") {
        return Err(unsupported("it is not INSERT INTO"));
    }
    if let Some(k) = any_of(
        top,
        &["ON", "RETURNING", "OUTPUT", "SET", "DEFAULT", "WITH"],
        2,
    ) {
        let why = match k {
            "ON" => "it has an ON CONFLICT or ON DUPLICATE KEY clause".to_string(),
            "SET" => "it uses the SET form".to_string(),
            "DEFAULT" => "it inserts default values, which only the engine knows".to_string(),
            other => format!("it uses {other}"),
        };
        return Err(unsupported(&why));
    }

    // The source: either a VALUES list or a SELECT, whichever comes first.
    let values_at = find(top, "VALUES", 2);
    let select_at = find(top, "SELECT", 2);
    let source = match (values_at, select_at) {
        (Some(v), Some(s)) => v.min(s),
        (Some(v), None) => v,
        (None, Some(s)) => s,
        (None, None) => return Err(unsupported("it has neither VALUES nor a SELECT")),
    };

    if Some(source) == select_at {
        // The rows that would arrive are exactly what the SELECT returns.
        return Ok(Preview {
            kind: Kind::Insert,
            select: sql[top[source].start..].trim().to_string(),
            note: "The rows the INSERT would add: what its SELECT returns.".into(),
        });
    }

    let head = &sql[top[1].end..top[source].start];
    let open = head
        .find('(')
        .ok_or_else(|| unsupported("name the columns to preview the rows"))?;
    let close = head
        .rfind(')')
        .filter(|c| *c > open)
        .ok_or_else(|| unsupported("the column list is not closed"))?;
    let list = &head[open + 1..close];
    let list_items = scan(list);
    let columns: Vec<String> = split_at_commas(list, &list_items, 0, list.len())
        .into_iter()
        .map(|c| bare_name(c.trim()))
        .collect();
    if columns.iter().any(|c| c.is_empty()) {
        return Err(unsupported("the column list has a gap"));
    }

    let rows_text = &sql[top[source].end..];
    let rows_items = scan(rows_text);
    let mut selects = Vec::new();
    for row in split_at_commas(rows_text, &rows_items, 0, rows_text.len()) {
        let row = row.trim();
        if !(row.starts_with('(') && row.ends_with(')')) {
            return Err(unsupported("a row is not a parenthesised list of values"));
        }
        let inner = &row[1..row.len() - 1];
        let inner_items = scan(inner);
        let values: Vec<&str> = split_at_commas(inner, &inner_items, 0, inner.len())
            .into_iter()
            .map(str::trim)
            .collect();
        if values.len() != columns.len() {
            return Err(unsupported(&format!(
                "a row has {} values for {} columns",
                values.len(),
                columns.len()
            )));
        }
        if values.iter().any(|v| v.eq_ignore_ascii_case("DEFAULT")) {
            return Err(unsupported(
                "a row uses DEFAULT, which only the engine knows",
            ));
        }
        let named: Vec<String> = values
            .iter()
            .zip(&columns)
            .map(|(v, c)| format!("{v} AS {}", quote_ident(c, dialect.quote)))
            .collect();
        let mut one = format!("SELECT {}", named.join(", "));
        if dialect.oracle {
            one.push_str(" FROM DUAL");
        }
        selects.push(one);
    }

    Ok(Preview {
        kind: Kind::Insert,
        select: selects.join(" UNION ALL "),
        note: "The rows the INSERT would add.".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: Dialect = Dialect {
        quote: '"',
        oracle: false,
    };
    const ORACLE: Dialect = Dialect {
        quote: '"',
        oracle: true,
    };
    const MSSQL: Dialect = Dialect {
        quote: '[',
        oracle: false,
    };

    fn ok(sql: &str, dialect: Dialect) -> Preview {
        preview(sql, dialect).expect("previewable")
    }

    fn refused(sql: &str) -> String {
        match preview(sql, PG) {
            Err(Error::Unsupported(why)) => why,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn update_shows_the_rows_and_the_new_values() {
        let p = ok(
            "UPDATE orders SET status = 'shipped', total = total * 1.1 WHERE id = 5;",
            PG,
        );
        assert_eq!(p.kind, Kind::Update);
        assert_eq!(
            p.select,
            "SELECT *, ('shipped') AS \"status (new)\", (total * 1.1) AS \"total (new)\" FROM orders WHERE id = 5"
        );
    }

    #[test]
    fn update_without_where_previews_every_row() {
        let p = ok("update t set a = 1", PG);
        assert_eq!(p.select, "SELECT *, (1) AS \"a (new)\" FROM t");
    }

    #[test]
    fn update_keeps_an_alias_and_a_qualified_column() {
        let p = ok("UPDATE public.orders o SET o.note = 'x' WHERE o.id = 1", PG);
        assert_eq!(
            p.select,
            "SELECT *, ('x') AS \"note (new)\" FROM public.orders o WHERE o.id = 1"
        );
    }

    #[test]
    fn update_expression_may_contain_commas_and_comparisons() {
        // The comma inside the call must not split the assignment, and the
        // `<=` must not be read as an assignment.
        let p = ok(
            "UPDATE t SET a = coalesce(b, c), d = CASE WHEN e <= 3 THEN 1 ELSE 0 END WHERE f >= 2",
            PG,
        );
        assert_eq!(
            p.select,
            "SELECT *, (coalesce(b, c)) AS \"a (new)\", (CASE WHEN e <= 3 THEN 1 ELSE 0 END) AS \"d (new)\" FROM t WHERE f >= 2"
        );
    }

    #[test]
    fn update_quotes_the_alias_for_the_engine() {
        let p = ok("UPDATE t SET [name] = 'x'", MSSQL);
        assert_eq!(p.select, "SELECT *, ('x') AS [name (new)] FROM t");
    }

    #[test]
    fn update_on_oracle_qualifies_the_star() {
        assert_eq!(
            ok("UPDATE t SET a = 1", ORACLE).select,
            "SELECT preview_.*, (1) AS \"a (new)\" FROM t preview_"
        );
        assert_eq!(
            ok("UPDATE t x SET a = 1", ORACLE).select,
            "SELECT x.*, (1) AS \"a (new)\" FROM t x"
        );
    }

    #[test]
    fn update_from_and_joins_are_refused() {
        assert!(refused("UPDATE t SET a = s.a FROM s WHERE s.id = t.id").contains("another table"));
        assert!(refused("UPDATE t JOIN s ON s.id = t.id SET t.a = 1").contains("JOIN"));
        assert!(refused("UPDATE t, s SET t.a = 1").contains("more than one table"));
        assert!(refused("UPDATE t SET a = 1 RETURNING id").contains("RETURNING"));
        assert!(refused("UPDATE t SET a = DEFAULT").contains("default"));
        assert!(refused("UPDATE t SET (a, b) = (1, 2)").contains("row of columns"));
    }

    #[test]
    fn delete_shows_the_rows_that_would_go() {
        let p = ok("DELETE FROM orders WHERE created < '2020-01-01'", PG);
        assert_eq!(p.kind, Kind::Delete);
        assert_eq!(
            p.select,
            "SELECT * FROM orders WHERE created < '2020-01-01'"
        );
        assert_eq!(ok("delete from t", PG).select, "SELECT * FROM t");
    }

    #[test]
    fn delete_shapes_this_cannot_place_are_refused() {
        assert!(refused("DELETE t FROM t JOIN s ON s.id = t.id").contains("not DELETE FROM"));
        assert!(refused("DELETE FROM t USING s WHERE s.id = t.id").contains("USING"));
        assert!(refused("DELETE FROM t ORDER BY id LIMIT 5").contains("ORDER"));
    }

    #[test]
    fn insert_values_become_one_select_per_row() {
        let p = ok(
            "INSERT INTO t (a, \"b c\") VALUES (1, 'x'), (2, upper('y'))",
            PG,
        );
        assert_eq!(p.kind, Kind::Insert);
        assert_eq!(
            p.select,
            "SELECT 1 AS \"a\", 'x' AS \"b c\" UNION ALL SELECT 2 AS \"a\", upper('y') AS \"b c\""
        );
    }

    #[test]
    fn insert_values_on_oracle_read_from_dual() {
        assert_eq!(
            ok("INSERT INTO t (a) VALUES (1)", ORACLE).select,
            "SELECT 1 AS \"a\" FROM DUAL"
        );
    }

    #[test]
    fn insert_select_is_its_own_preview() {
        let p = ok(
            "INSERT INTO archive (id, note) SELECT id, note FROM t WHERE old",
            PG,
        );
        assert_eq!(p.select, "SELECT id, note FROM t WHERE old");
    }

    #[test]
    fn insert_needs_named_columns_and_matching_rows() {
        assert!(refused("INSERT INTO t VALUES (1, 2)").contains("name the columns"));
        assert!(refused("INSERT INTO t (a, b) VALUES (1)").contains("1 values for 2 columns"));
        assert!(
            refused("INSERT INTO t (a) VALUES (1) ON CONFLICT DO NOTHING").contains("ON CONFLICT")
        );
        assert!(refused("INSERT INTO t (a) VALUES (DEFAULT)").contains("DEFAULT"));
        assert!(refused("INSERT INTO t DEFAULT VALUES").contains("default"));
    }

    #[test]
    fn only_writes_and_only_one_at_a_time() {
        assert!(refused("SELECT 1").contains("only UPDATE"));
        assert!(refused("UPDATE t SET a = 1; DELETE FROM t").contains("one statement"));
        assert!(refused("").contains("nothing"));
    }

    #[test]
    fn keywords_inside_literals_do_not_count() {
        // A string that mentions FROM must not make this an UPDATE … FROM.
        let p = ok(
            "UPDATE t SET note = 'copied FROM elsewhere' WHERE id = 1",
            PG,
        );
        assert!(p.select.ends_with("FROM t WHERE id = 1"));
    }
}
