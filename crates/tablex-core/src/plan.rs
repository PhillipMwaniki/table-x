//! Query plans, in one shape across five engines that agree on nothing.
//!
//! Every engine will describe how it intends to run a statement, and every
//! engine describes it differently: PostgreSQL emits a JSON tree, MySQL a JSON
//! tree of a different shape that also varies by version, SQLite and SQL Server
//! flat rows with parent pointers, ClickHouse indented text. What survives the
//! translation is the part worth looking at — what each step does, how many rows
//! it expects, what it costs, and what feeds it.
//!
//! Two things this module refuses to do. It does not compare costs across
//! engines: PostgreSQL's cost is in arbitrary page-fetch units, SQL Server's is
//! a different arbitrary unit, and a number that means nothing on its own means
//! less next to another one. Costs are only ever shown as a share of the plan
//! they came from. And it does not throw away the engine's own output — a
//! parser can only lift what it was taught to look for, so [`Plan::raw`] keeps
//! the text as the server wrote it.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// One step in a plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanNode {
    /// What this step does: `Seq Scan`, `Hash Join`, `SEARCH`.
    pub label: String,
    /// What it does it to, and under what condition.
    pub detail: Option<String>,
    /// Rows the planner expects.
    pub rows: Option<f64>,
    /// Rows there actually were. Only present when the statement was run.
    pub actual_rows: Option<f64>,
    /// Cost of this step *and everything below it*, in the engine's own units.
    pub cost: Option<f64>,
    /// Milliseconds this step and its children actually took.
    pub actual_ms: Option<f64>,
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    pub fn new(label: impl Into<String>) -> Self {
        PlanNode {
            label: label.into(),
            ..Default::default()
        }
    }
}

// Note: the derived numbers a reader actually looks at — a step's cost with its
// children's removed, and how far an estimate missed by — live in `src/lib/plan.ts`
// rather than here. They exist to decide which row to highlight and how wide to
// draw a bar, which is presentation; having them in both places would mean two
// definitions of "expensive" that could drift apart.

/// A parsed plan, and the text it was parsed from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub root: PlanNode,
    /// Whether the statement was actually run, so the numbers are measured
    /// rather than guessed. The difference matters enough to label.
    pub analyzed: bool,
    /// The engine's own output, unmodified.
    pub raw: String,
}

// ---------------------------------------------------------------------------
// PostgreSQL — EXPLAIN (FORMAT JSON)
// ---------------------------------------------------------------------------

/// Parse `EXPLAIN (FORMAT JSON)` output.
pub fn from_postgres_json(text: &str) -> Result<PlanNode> {
    let doc: Json = serde_json::from_str(text)
        .map_err(|e| Error::query(format!("could not read the plan: {e}")))?;

    // The document is an array with one entry per statement; the app explains
    // one statement at a time.
    let plan = doc
        .get(0)
        .and_then(|entry| entry.get("Plan"))
        .ok_or_else(|| Error::query("the plan came back without a Plan node"))?;

    Ok(pg_node(plan))
}

fn pg_node(json: &Json) -> PlanNode {
    let text = |key: &str| json.get(key).and_then(Json::as_str).map(str::to_string);
    let num = |key: &str| json.get(key).and_then(Json::as_f64);

    // A join's type is half its name — "Hash Join" alone does not say whether
    // rows without a match survive it.
    let mut label = text("Node Type").unwrap_or_else(|| "?".into());
    if let Some(join) = text("Join Type") {
        if join != "Inner" {
            label = format!("{label} ({join})");
        }
    }

    // Relation first, then whichever condition applies: these are alternatives
    // in practice, and showing all of them would bury the one that is set.
    let mut detail = Vec::new();
    if let Some(relation) = text("Relation Name") {
        match text("Alias") {
            Some(alias) if alias != relation => detail.push(format!("on {relation} as {alias}")),
            _ => detail.push(format!("on {relation}")),
        }
    }
    if let Some(index) = text("Index Name") {
        detail.push(format!("using {index}"));
    }
    for key in [
        "Index Cond",
        "Hash Cond",
        "Merge Cond",
        "Join Filter",
        "Filter",
    ] {
        if let Some(cond) = text(key) {
            detail.push(cond);
            break;
        }
    }

    PlanNode {
        label,
        detail: (!detail.is_empty()).then(|| detail.join(" · ")),
        rows: num("Plan Rows"),
        actual_rows: num("Actual Rows"),
        cost: num("Total Cost"),
        actual_ms: num("Actual Total Time"),
        children: json
            .get("Plans")
            .and_then(Json::as_array)
            .map(|kids| kids.iter().map(pg_node).collect())
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// MySQL / MariaDB — EXPLAIN FORMAT=JSON
// ---------------------------------------------------------------------------

/// Parse `EXPLAIN FORMAT=JSON` output.
///
/// Walked structurally rather than against a known schema. MySQL's shape is
/// irregular — a step may be a `table`, a `nested_loop` array, an
/// `ordering_operation`, a `grouping_operation`, a `union_result` — and it
/// changes between 5.7, 8.0, and MariaDB. A parser written against one version's
/// field list is a parser that renders nothing on the next; one that follows the
/// nesting renders something useful on all of them.
pub fn from_mysql_json(text: &str) -> Result<PlanNode> {
    let doc: Json = serde_json::from_str(text)
        .map_err(|e| Error::query(format!("could not read the plan: {e}")))?;

    let block = doc
        .get("query_block")
        .ok_or_else(|| Error::query("the plan came back without a query_block"))?;

    Ok(my_node("query_block", block))
}

fn my_node(name: &str, json: &Json) -> PlanNode {
    let mut node = PlanNode::new(prettify(name));
    let mut children = Vec::new();

    let Some(object) = json.as_object() else {
        return node;
    };

    // A `table` step is the one with numbers on it; everything else is a
    // container that exists to hold ordering, grouping, or a join.
    if let Some(table) = object.get("table_name").and_then(Json::as_str) {
        node.label = object
            .get("access_type")
            .and_then(Json::as_str)
            .map(|a| format!("{} scan", prettify(a)))
            .unwrap_or_else(|| "Table".into());

        let mut detail = vec![format!("on {table}")];
        if let Some(key) = object.get("key").and_then(Json::as_str) {
            detail.push(format!("using {key}"));
        }
        if let Some(cond) = object.get("attached_condition").and_then(Json::as_str) {
            detail.push(cond.to_string());
        }
        node.detail = Some(detail.join(" · "));
        node.rows = object
            .get("rows_examined_per_scan")
            .or_else(|| object.get("rows"))
            .and_then(as_number);
    }

    // Costs are strings in MySQL's JSON, which is why every read goes through
    // `as_number` rather than `as_f64`.
    node.cost = object
        .get("cost_info")
        .and_then(|c| {
            c.get("query_cost")
                .or_else(|| c.get("prefix_cost"))
                .or_else(|| c.get("read_cost"))
        })
        .and_then(as_number);

    for (key, value) in object {
        match value {
            // An array of steps: a nested loop's operands, or a union's arms.
            // The array's key names a real step — the join itself — so it gets
            // a node, with the operands beneath it. Flattening them into
            // siblings would lose the fact that they are being joined at all.
            Json::Array(items) if items.iter().any(Json::is_object) => {
                let mut group = PlanNode::new(prettify(key));
                group.children = items
                    .iter()
                    .filter(|i| i.is_object())
                    // A wrapper whose only content is one named step should not
                    // add a level of its own — `{"table": {...}}` is the table.
                    .map(|item| unwrap_single(key, item))
                    .collect();
                children.push(group);
            }
            Json::Object(_) if key != "cost_info" => {
                children.push(my_node(key, value));
            }
            _ => {}
        }
    }

    node.children = children;
    node
}

/// Collapse `{"table": {...}}` to the node inside it.
fn unwrap_single(fallback: &str, json: &Json) -> PlanNode {
    if let Some(object) = json.as_object() {
        if object.len() == 1 {
            let (key, value) = object.iter().next().expect("length checked");
            if value.is_object() {
                return my_node(key, value);
            }
        }
    }
    my_node(fallback, json)
}

/// MySQL writes numbers as strings about half the time.
fn as_number(json: &Json) -> Option<f64> {
    json.as_f64().or_else(|| json.as_str()?.parse().ok())
}

/// `nested_loop` → `Nested loop`.
fn prettify(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

// ---------------------------------------------------------------------------
// SQLite and SQL Server — flat rows with parent pointers
// ---------------------------------------------------------------------------

/// One row of a plan that arrives as a table rather than a tree.
#[derive(Debug, Clone)]
pub struct PlanRow {
    pub id: i64,
    /// The row this one hangs under. A row whose parent is absent from the set
    /// is treated as a root.
    pub parent: i64,
    pub label: String,
    pub detail: Option<String>,
    pub rows: Option<f64>,
    pub cost: Option<f64>,
}

/// Assemble rows into a tree by their parent pointers.
///
/// Rows arrive in execution order and a parent always precedes its children, but
/// this does not rely on that: a cycle or a forward reference would otherwise
/// hang the UI rather than render a slightly odd tree.
pub fn from_parent_rows(rows: Vec<PlanRow>, root_label: &str) -> PlanNode {
    let mut root = PlanNode::new(root_label);
    if rows.is_empty() {
        return root;
    }

    // Index children by parent, then build depth-first from the roots. Building
    // by index rather than by recursive lookup is what makes a malformed set
    // finite: every row is placed at most once.
    let ids: std::collections::HashSet<i64> = rows.iter().map(|r| r.id).collect();
    let mut by_parent: std::collections::HashMap<i64, Vec<usize>> =
        std::collections::HashMap::new();
    let mut roots = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if row.parent != row.id && ids.contains(&row.parent) {
            by_parent.entry(row.parent).or_default().push(index);
        } else {
            roots.push(index);
        }
    }

    fn build(
        index: usize,
        rows: &[PlanRow],
        by_parent: &std::collections::HashMap<i64, Vec<usize>>,
        seen: &mut std::collections::HashSet<usize>,
    ) -> PlanNode {
        let row = &rows[index];
        let mut node = PlanNode {
            label: row.label.clone(),
            detail: row.detail.clone(),
            rows: row.rows,
            cost: row.cost,
            ..Default::default()
        };
        if !seen.insert(index) {
            return node;
        }
        if let Some(kids) = by_parent.get(&row.id) {
            // Collected first: the recursion mutates `seen`, so the filter and
            // the build cannot share a borrow of it.
            let pending: Vec<usize> = kids.iter().copied().filter(|k| !seen.contains(k)).collect();
            node.children = pending
                .into_iter()
                .map(|k| build(k, rows, by_parent, seen))
                .collect();
        }
        node
    }

    let mut seen = std::collections::HashSet::new();
    let children: Vec<PlanNode> = roots
        .into_iter()
        .map(|index| build(index, &rows, &by_parent, &mut seen))
        .collect();

    // A single root needs no wrapper above it.
    if children.len() == 1 {
        return children.into_iter().next().expect("length checked");
    }
    root.children = children;
    root
}

// ---------------------------------------------------------------------------
// ClickHouse — indented text
// ---------------------------------------------------------------------------

/// Parse an indented text plan, where depth is the leading whitespace.
pub fn from_indented(text: &str, root_label: &str) -> PlanNode {
    let mut root = PlanNode::new(root_label);
    // (depth, index into a flat arena) — a stack of the current ancestors.
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut nodes: Vec<PlanNode> = Vec::new();
    let mut parents: Vec<Option<usize>> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        let depth = trimmed.len() - trimmed.trim_start().len();
        let body = trimmed.trim_start();

        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            stack.pop();
        }
        let parent = stack.last().and_then(|(_, path)| path.last().copied());

        // ClickHouse writes `Node (detail)`; splitting there is what separates
        // the operation from what it operates on.
        let (label, detail) = match body.split_once(" (") {
            Some((head, tail)) => (
                head.to_string(),
                Some(tail.trim_end_matches(')').to_string()),
            ),
            None => (body.to_string(), None),
        };

        nodes.push(PlanNode {
            label,
            detail,
            ..Default::default()
        });
        parents.push(parent);
        stack.push((depth, vec![nodes.len() - 1]));
    }

    // Assemble from the leaves up, so each child is complete before it moves.
    for index in (0..nodes.len()).rev() {
        if let Some(parent) = parents[index] {
            let child = std::mem::take(&mut nodes[index]);
            nodes[parent].children.insert(0, child);
        }
    }

    let mut tops: Vec<PlanNode> = nodes
        .into_iter()
        .zip(parents)
        .filter(|(node, parent)| parent.is_none() && !node.label.is_empty())
        .map(|(node, _)| node)
        .collect();

    if tops.len() == 1 {
        return tops.pop().expect("length checked");
    }
    root.children = tops;
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_postgres_plan_becomes_a_tree() {
        let json = r#"[{"Plan": {
            "Node Type": "Hash Join",
            "Join Type": "Left",
            "Total Cost": 120.5,
            "Plan Rows": 1000,
            "Hash Cond": "(a.id = b.a_id)",
            "Plans": [
              {"Node Type": "Seq Scan", "Relation Name": "a", "Alias": "a",
               "Total Cost": 30.0, "Plan Rows": 500, "Filter": "(x > 1)"},
              {"Node Type": "Seq Scan", "Relation Name": "b", "Alias": "b",
               "Total Cost": 20.0, "Plan Rows": 400}
            ]}}]"#;

        let root = from_postgres_json(json).unwrap();
        // The join type is part of the name: "Hash Join" alone does not say
        // whether unmatched rows survive.
        assert_eq!(root.label, "Hash Join (Left)");
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].detail.as_deref(), Some("on a · (x > 1)"));
        assert_eq!(root.rows, Some(1000.0));
    }

    #[test]
    fn an_inner_join_is_not_labelled_with_its_type() {
        // Every join is inner unless it says otherwise; saying so is noise.
        let json = r#"[{"Plan": {"Node Type": "Nested Loop", "Join Type": "Inner"}}]"#;
        assert_eq!(from_postgres_json(json).unwrap().label, "Nested Loop");
    }

    #[test]
    fn a_missing_plan_is_an_error_not_an_empty_tree() {
        // An empty tree would render as a working plan with nothing in it.
        assert!(from_postgres_json("[]").is_err());
        assert!(from_postgres_json("not json").is_err());
    }

    #[test]
    fn a_mysql_plan_is_walked_by_its_shape_not_its_schema() {
        // The nesting differs between 5.7, 8.0, and MariaDB; following the
        // structure renders something useful on all of them.
        let json = r#"{"query_block": {
            "select_id": 1,
            "cost_info": {"query_cost": "12.40"},
            "nested_loop": [
              {"table": {"table_name": "orders", "access_type": "ALL",
                         "rows_examined_per_scan": 900,
                         "attached_condition": "(orders.total > 10)"}},
              {"table": {"table_name": "users", "access_type": "eq_ref",
                         "key": "PRIMARY", "rows_examined_per_scan": 1}}
            ]}}"#;

        let root = from_mysql_json(json).unwrap();
        assert_eq!(root.cost, Some(12.40));
        let loop_node = &root.children[0];
        assert_eq!(loop_node.label, "Nested loop");
        assert_eq!(loop_node.children.len(), 2);
        assert_eq!(loop_node.children[0].label, "ALL scan");
        assert_eq!(
            loop_node.children[0].detail.as_deref(),
            Some("on orders · (orders.total > 10)")
        );
        assert_eq!(
            loop_node.children[1].detail.as_deref(),
            Some("on users · using PRIMARY")
        );
        assert_eq!(loop_node.children[0].rows, Some(900.0));
    }

    #[test]
    fn parent_rows_assemble_into_a_tree() {
        let rows = vec![
            PlanRow {
                id: 1,
                parent: 0,
                label: "SCAN a".into(),
                detail: None,
                rows: None,
                cost: None,
            },
            PlanRow {
                id: 2,
                parent: 1,
                label: "SEARCH b".into(),
                detail: None,
                rows: None,
                cost: None,
            },
            PlanRow {
                id: 3,
                parent: 1,
                label: "USE TEMP B-TREE".into(),
                detail: None,
                rows: None,
                cost: None,
            },
        ];
        let root = from_parent_rows(rows, "Query");
        // One root needs no wrapper above it.
        assert_eq!(root.label, "SCAN a");
        assert_eq!(root.children.len(), 2);
    }

    #[test]
    fn a_cycle_in_the_rows_terminates() {
        // Malformed input should cost a slightly odd tree, not a hung window.
        let rows = vec![
            PlanRow {
                id: 1,
                parent: 2,
                label: "A".into(),
                detail: None,
                rows: None,
                cost: None,
            },
            PlanRow {
                id: 2,
                parent: 1,
                label: "B".into(),
                detail: None,
                rows: None,
                cost: None,
            },
        ];
        let root = from_parent_rows(rows, "Query");
        fn count(node: &PlanNode) -> usize {
            1 + node.children.iter().map(count).sum::<usize>()
        }
        assert!(count(&root) <= 3, "a cycle produced {} nodes", count(&root));
    }

    #[test]
    fn several_roots_get_a_wrapper() {
        let rows = vec![
            PlanRow {
                id: 1,
                parent: 0,
                label: "A".into(),
                detail: None,
                rows: None,
                cost: None,
            },
            PlanRow {
                id: 2,
                parent: 0,
                label: "B".into(),
                detail: None,
                rows: None,
                cost: None,
            },
        ];
        let root = from_parent_rows(rows, "Query");
        assert_eq!(root.label, "Query");
        assert_eq!(root.children.len(), 2);
    }

    #[test]
    fn indentation_becomes_depth() {
        let text = "Expression (Projection)\n  Aggregating\n    ReadFromMergeTree (db.events)";
        let root = from_indented(text, "Plan");
        assert_eq!(root.label, "Expression");
        assert_eq!(root.detail.as_deref(), Some("Projection"));
        assert_eq!(root.children[0].label, "Aggregating");
        assert_eq!(root.children[0].children[0].label, "ReadFromMergeTree");
        assert_eq!(
            root.children[0].children[0].detail.as_deref(),
            Some("db.events")
        );
    }

    #[test]
    fn siblings_at_the_same_indent_stay_siblings() {
        let text = "Union\n  ReadFromMergeTree (a)\n  ReadFromMergeTree (b)";
        let root = from_indented(text, "Plan");
        assert_eq!(root.children.len(), 2);
        // And in the order the server printed them.
        assert_eq!(root.children[0].detail.as_deref(), Some("a"));
        assert_eq!(root.children[1].detail.as_deref(), Some("b"));
    }

    #[test]
    fn an_empty_plan_does_not_panic() {
        assert_eq!(from_indented("", "Plan").children.len(), 0);
        assert_eq!(from_parent_rows(Vec::new(), "Plan").label, "Plan");
    }
}
