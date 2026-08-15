//! Laying a schema out as a diagram.
//!
//! The layout is here rather than in the UI for one reason: it has to be the
//! same every time. A diagram that reshuffles itself each time it is opened
//! cannot be learned, and half the value of looking at a schema this way is
//! that the shape becomes familiar. So every step is deterministic, ties break
//! by name, and nothing depends on hash iteration order.
//!
//! The layering runs the way people read a schema: tables that reference
//! nothing sit at the bottom, and each table sits one level above the highest
//! thing it points at. Lookup tables end up along the base, and the tables that
//! tie everything together rise to the top.

use crate::schema::ForeignKeyDef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One table and the foreign keys leaving it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTable {
    pub schema: Option<String>,
    pub name: String,
    pub foreign_keys: Vec<ForeignKeyDef>,
}

/// Every table in one schema, with its relations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaGraph {
    pub tables: Vec<GraphTable>,
}

/// A column shown inside a box.
///
/// Only key columns are drawn. A diagram of two hundred tables with every
/// column listed is a wall, and the columns that carry the relationships are
/// the ones the diagram exists to show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxColumn {
    pub name: String,
    /// This column points at another table.
    pub outgoing: bool,
    /// Another table points at this column.
    pub incoming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramBox {
    pub schema: Option<String>,
    pub table: String,
    pub columns: Vec<BoxColumn>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One relation, as indices into [`Diagram::boxes`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramEdge {
    pub from: usize,
    pub to: usize,
    pub label: String,
    /// A table that references itself. Drawn as a loop rather than a line
    /// between two points that are the same point.
    pub reflexive: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagram {
    pub boxes: Vec<DiagramBox>,
    pub edges: Vec<DiagramEdge>,
    pub width: f64,
    pub height: f64,
    /// Foreign keys whose target is not in this schema, named so their absence
    /// is visible. A diagram that silently drops a relation is a diagram that
    /// says the relation does not exist.
    pub dangling: Vec<String>,
}

const HEADER_HEIGHT: f64 = 24.0;
const ROW_HEIGHT: f64 = 16.0;
const CHAR_WIDTH: f64 = 7.0;
const MIN_WIDTH: f64 = 130.0;
const PADDING: f64 = 16.0;
const H_GAP: f64 = 44.0;
const V_GAP: f64 = 64.0;
const MARGIN: f64 = 24.0;

/// A table's key in the graph, case-folded because engines disagree about
/// whether `Users` and `users` are the same table and foreign keys are stored
/// however they were written.
fn key(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", s.to_lowercase(), table.to_lowercase()),
        None => table.to_lowercase(),
    }
}

/// Place every table, and route every relation between them.
pub fn layout(graph: &SchemaGraph) -> Diagram {
    if graph.tables.is_empty() {
        return Diagram::default();
    }

    // Sorted first, so the whole layout is a function of the schema and not of
    // the order the catalog happened to return rows in.
    let mut tables: Vec<&GraphTable> = graph.tables.iter().collect();
    tables.sort_by(|a, b| {
        key(a.schema.as_deref(), &a.name).cmp(&key(b.schema.as_deref(), &b.name))
    });

    let index: HashMap<String, usize> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| (key(t.schema.as_deref(), &t.name), i))
        .collect();

    // --- edges ------------------------------------------------------------
    let mut edges = Vec::new();
    let mut dangling = Vec::new();
    // Column name → what touches it, per table.
    let mut outgoing: Vec<Vec<String>> = vec![Vec::new(); tables.len()];
    let mut incoming: Vec<Vec<String>> = vec![Vec::new(); tables.len()];

    for (from, table) in tables.iter().enumerate() {
        for fk in &table.foreign_keys {
            // A foreign key with no schema of its own lives in its table's.
            let target_schema = fk
                .referenced_schema
                .as_deref()
                .or(table.schema.as_deref());
            let Some(&to) = index.get(&key(target_schema, &fk.referenced_table)) else {
                dangling.push(format!(
                    "{}.{} → {}",
                    table.name,
                    fk.columns.join(", "),
                    fk.referenced_table
                ));
                continue;
            };

            outgoing[from].extend(fk.columns.iter().cloned());
            incoming[to].extend(fk.referenced_columns.iter().cloned());
            edges.push(DiagramEdge {
                from,
                to,
                label: fk.columns.join(", "),
                reflexive: from == to,
            });
        }
    }

    // --- levels -----------------------------------------------------------
    let levels = assign_levels(&tables, &index);
    let depth = levels.iter().copied().max().unwrap_or(0) + 1;

    // --- boxes ------------------------------------------------------------
    let mut boxes: Vec<DiagramBox> = tables
        .iter()
        .enumerate()
        .map(|(i, table)| {
            // One entry per column, whichever direction it was touched from,
            // in first-seen order so a composite key keeps its order.
            let mut columns: Vec<BoxColumn> = Vec::new();
            for (name, is_outgoing) in outgoing[i]
                .iter()
                .map(|c| (c, true))
                .chain(incoming[i].iter().map(|c| (c, false)))
            {
                match columns.iter_mut().find(|c| c.name.eq_ignore_ascii_case(name)) {
                    Some(existing) => {
                        existing.outgoing |= is_outgoing;
                        existing.incoming |= !is_outgoing;
                    }
                    None => columns.push(BoxColumn {
                        name: name.clone(),
                        outgoing: is_outgoing,
                        incoming: !is_outgoing,
                    }),
                }
            }

            let widest = columns
                .iter()
                .map(|c| c.name.len())
                .chain(std::iter::once(table.name.len()))
                .max()
                .unwrap_or(0);

            DiagramBox {
                schema: table.schema.clone(),
                table: table.name.clone(),
                height: HEADER_HEIGHT + columns.len() as f64 * ROW_HEIGHT + 4.0,
                width: (widest as f64 * CHAR_WIDTH + PADDING * 2.0).max(MIN_WIDTH),
                columns,
                x: 0.0,
                y: 0.0,
            }
        })
        .collect();

    // --- ordering within each level ---------------------------------------
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (i, &level) in levels.iter().enumerate() {
        rows[level].push(i);
    }
    order_rows(&mut rows, &edges);

    // --- coordinates ------------------------------------------------------
    // Level 0 is drawn at the bottom: the tables that reference nothing are the
    // ones everything else stands on.
    let mut y = MARGIN;
    let mut width: f64 = 0.0;
    for row in rows.iter().rev() {
        let mut x = MARGIN;
        let mut tallest: f64 = 0.0;
        for &i in row {
            boxes[i].x = x;
            boxes[i].y = y;
            x += boxes[i].width + H_GAP;
            tallest = tallest.max(boxes[i].height);
        }
        width = width.max(x - H_GAP + MARGIN);
        y += tallest + V_GAP;
    }

    Diagram {
        boxes,
        edges,
        width,
        height: y - V_GAP + MARGIN,
        dangling,
    }
}

/// How high above the base each table sits.
///
/// A table's level is one more than the highest thing it points at. Cycles are
/// real in schemas — two tables that reference each other, or a table that
/// references itself — so this iterates to a fixed point with a hard bound
/// rather than recursing, which would not terminate on one.
fn assign_levels(tables: &[&GraphTable], index: &HashMap<String, usize>) -> Vec<usize> {
    let mut levels = vec![0usize; tables.len()];

    for _ in 0..tables.len() {
        let mut changed = false;
        for (from, table) in tables.iter().enumerate() {
            for fk in &table.foreign_keys {
                let target_schema = fk
                    .referenced_schema
                    .as_deref()
                    .or(table.schema.as_deref());
                let Some(&to) = index.get(&key(target_schema, &fk.referenced_table)) else {
                    continue;
                };
                // A self-reference cannot lift a table above itself.
                if to == from {
                    continue;
                }
                if levels[from] <= levels[to] {
                    levels[from] = levels[to] + 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    levels
}

/// Order each level to put connected tables near each other.
///
/// The barycenter heuristic: a table moves to the average position of the
/// things it connects to on the level below. Two passes is enough to be worth
/// it — this reduces crossings, it does not minimize them, and minimizing them
/// is NP-hard for no visible gain over this.
fn order_rows(rows: &mut [Vec<usize>], edges: &[DiagramEdge]) {
    for _ in 0..2 {
        // Positions from the level below, which was ordered on the last pass.
        let mut position: HashMap<usize, f64> = HashMap::new();
        for row in rows.iter() {
            for (slot, &i) in row.iter().enumerate() {
                position.insert(i, slot as f64);
            }
        }

        for row in rows.iter_mut() {
            let barycenter = |node: usize| -> Option<f64> {
                let mut sum = 0.0;
                let mut count = 0.0;
                for edge in edges.iter().filter(|e| !e.reflexive) {
                    let other = if edge.from == node {
                        edge.to
                    } else if edge.to == node {
                        edge.from
                    } else {
                        continue;
                    };
                    if let Some(&p) = position.get(&other) {
                        sum += p;
                        count += 1.0;
                    }
                }
                (count > 0.0).then(|| sum / count)
            };

            // Unconnected tables keep their place at the end rather than
            // sorting to the front, where they would push the related ones
            // apart. Ties keep the existing order, which is by name.
            let mut keyed: Vec<(usize, f64, usize)> = row
                .iter()
                .enumerate()
                .map(|(slot, &node)| (node, barycenter(node).unwrap_or(f64::MAX), slot))
                .collect();
            keyed.sort_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.2.cmp(&b.2))
            });
            *row = keyed.into_iter().map(|(node, _, _)| node).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fk(columns: &[&str], table: &str, referenced: &[&str]) -> ForeignKeyDef {
        ForeignKeyDef {
            name: format!("fk_{table}"),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            referenced_schema: None,
            referenced_table: table.into(),
            referenced_columns: referenced.iter().map(|c| c.to_string()).collect(),
            on_delete: None,
            on_update: None,
        }
    }

    fn table(name: &str, keys: Vec<ForeignKeyDef>) -> GraphTable {
        GraphTable {
            schema: None,
            name: name.into(),
            foreign_keys: keys,
        }
    }

    #[test]
    fn a_referenced_table_sits_below_the_one_referencing_it() {
        let graph = SchemaGraph {
            tables: vec![
                table("orders", vec![fk(&["user_id"], "users", &["id"])]),
                table("users", Vec::new()),
            ],
        };
        let diagram = layout(&graph);

        let orders = diagram.boxes.iter().find(|b| b.table == "orders").unwrap();
        let users = diagram.boxes.iter().find(|b| b.table == "users").unwrap();
        // Lower on the page means a larger y; users is what orders stands on.
        assert!(users.y > orders.y, "{} vs {}", users.y, orders.y);
    }

    #[test]
    fn the_layout_is_the_same_every_time() {
        // A diagram that reshuffles cannot be learned, and half the point of
        // looking at a schema this way is that its shape becomes familiar.
        let tables = vec![
            table("orders", vec![fk(&["user_id"], "users", &["id"])]),
            table("users", Vec::new()),
            table("items", vec![fk(&["order_id"], "orders", &["id"])]),
            table("tags", Vec::new()),
        ];
        let forward = layout(&SchemaGraph {
            tables: tables.clone(),
        });
        let reversed = layout(&SchemaGraph {
            tables: tables.into_iter().rev().collect(),
        });

        let places = |d: &Diagram| {
            d.boxes
                .iter()
                .map(|b| (b.table.clone(), b.x, b.y))
                .collect::<Vec<_>>()
        };
        assert_eq!(places(&forward), places(&reversed));
    }

    #[test]
    fn a_cycle_terminates_and_still_lays_out() {
        // Two tables that reference each other is a real schema, not a
        // malformed one — and a recursive level assignment would hang on it.
        let graph = SchemaGraph {
            tables: vec![
                table("a", vec![fk(&["b_id"], "b", &["id"])]),
                table("b", vec![fk(&["a_id"], "a", &["id"])]),
            ],
        };
        let diagram = layout(&graph);
        assert_eq!(diagram.boxes.len(), 2);
        assert_eq!(diagram.edges.len(), 2);
    }

    #[test]
    fn a_self_reference_is_marked_and_does_not_lift_the_table() {
        let graph = SchemaGraph {
            tables: vec![table(
                "employees",
                vec![fk(&["manager_id"], "employees", &["id"])],
            )],
        };
        let diagram = layout(&graph);
        assert_eq!(diagram.edges.len(), 1);
        assert!(diagram.edges[0].reflexive);
        assert_eq!(diagram.edges[0].from, diagram.edges[0].to);
        // Nothing else is on the diagram, so it stays on the base row.
        assert_eq!(diagram.boxes[0].y, MARGIN);
    }

    #[test]
    fn a_key_to_a_table_outside_the_schema_is_reported_not_dropped() {
        // Silently dropping it would draw a schema where the relation does not
        // exist, which is a different and wrong schema.
        let graph = SchemaGraph {
            tables: vec![table(
                "orders",
                vec![fk(&["tenant_id"], "tenants", &["id"])],
            )],
        };
        let diagram = layout(&graph);
        assert!(diagram.edges.is_empty());
        assert_eq!(diagram.dangling.len(), 1);
        assert!(diagram.dangling[0].contains("tenants"));
    }

    #[test]
    fn only_key_columns_are_drawn_and_both_directions_are_marked() {
        let graph = SchemaGraph {
            tables: vec![
                table("orders", vec![fk(&["user_id"], "users", &["id"])]),
                table("users", Vec::new()),
            ],
        };
        let diagram = layout(&graph);

        let orders = diagram.boxes.iter().find(|b| b.table == "orders").unwrap();
        assert_eq!(orders.columns.len(), 1);
        assert_eq!(orders.columns[0].name, "user_id");
        assert!(orders.columns[0].outgoing && !orders.columns[0].incoming);

        let users = diagram.boxes.iter().find(|b| b.table == "users").unwrap();
        assert_eq!(users.columns[0].name, "id");
        assert!(users.columns[0].incoming && !users.columns[0].outgoing);
    }

    #[test]
    fn a_column_pointed_at_and_pointing_out_is_marked_both_ways() {
        let graph = SchemaGraph {
            tables: vec![
                table("a", vec![fk(&["id"], "b", &["id"])]),
                table("b", vec![fk(&["id"], "a", &["id"])]),
            ],
        };
        let diagram = layout(&graph);
        let a = diagram.boxes.iter().find(|b| b.table == "a").unwrap();
        assert_eq!(a.columns.len(), 1, "one column, touched twice");
        assert!(a.columns[0].outgoing && a.columns[0].incoming);
    }

    #[test]
    fn boxes_on_a_row_do_not_overlap() {
        let graph = SchemaGraph {
            tables: (0..6)
                .map(|i| table(&format!("t{i}"), Vec::new()))
                .collect(),
        };
        let diagram = layout(&graph);
        let mut row: Vec<&DiagramBox> = diagram.boxes.iter().collect();
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        for pair in row.windows(2) {
            assert!(
                pair[0].x + pair[0].width <= pair[1].x,
                "{} overlaps {}",
                pair[0].table,
                pair[1].table
            );
        }
        assert!(diagram.width >= row.last().unwrap().x);
    }

    #[test]
    fn table_names_are_matched_without_regard_to_case() {
        // Engines disagree about whether Users and users are the same table,
        // and a foreign key stores whatever was written.
        let graph = SchemaGraph {
            tables: vec![
                table("Orders", vec![fk(&["user_id"], "USERS", &["id"])]),
                table("users", Vec::new()),
            ],
        };
        let diagram = layout(&graph);
        assert_eq!(diagram.edges.len(), 1, "{:?}", diagram.dangling);
        assert!(diagram.dangling.is_empty());
    }

    #[test]
    fn an_empty_schema_is_an_empty_diagram_not_a_panic() {
        let diagram = layout(&SchemaGraph::default());
        assert!(diagram.boxes.is_empty());
        assert_eq!(diagram.width, 0.0);
    }

    #[test]
    fn a_chain_stacks_one_level_per_link() {
        let graph = SchemaGraph {
            tables: vec![
                table("c", vec![fk(&["b_id"], "b", &["id"])]),
                table("b", vec![fk(&["a_id"], "a", &["id"])]),
                table("a", Vec::new()),
            ],
        };
        let diagram = layout(&graph);
        let y = |name: &str| {
            diagram
                .boxes
                .iter()
                .find(|b| b.table == name)
                .unwrap()
                .y
        };
        assert!(y("a") > y("b"), "a should be under b");
        assert!(y("b") > y("c"), "b should be under c");
    }
}
