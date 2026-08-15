//! Comparing two schemas, and writing the statements that reconcile them.
//!
//! The direction is fixed and stated everywhere it could be ambiguous:
//! [`diff`] takes `from` and `to` and reports what would have to happen to
//! **`from`** to make it look like **`to`**. Getting this backwards generates a
//! script that destroys the wrong side, so it is named in every signature and
//! pinned by its own test.
//!
//! Two things this deliberately does not do.
//!
//! It does not detect renames. A renamed column is indistinguishable from one
//! dropped and another added — the catalog records no link between them — so
//! guessing would mean sometimes emitting `ALTER … RENAME` for two unrelated
//! columns and silently discarding a real one's data. Reported as a drop and an
//! add, a rename is obvious to the person reading the script, who knows which
//! it was.
//!
//! And it does not run anything. The output is a script to read, because the
//! question worth asking about generated DDL is not "are you sure" but "does
//! this say what you meant", and only the statements themselves can answer it.

use crate::schema::{ColumnDef, ForeignKeyDef, IndexDef, TableDetail};
use crate::sql::quote_ident;
use serde::{Deserialize, Serialize};

/// One side of a comparison.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    /// How this side is named in the report — a schema, a database, a
    /// connection. Only ever displayed.
    pub label: String,
    pub tables: Vec<TableDetail>,
}

/// A single field of a column that differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    TableAdded {
        table: String,
        columns: Vec<ColumnDef>,
        primary_key: Vec<String>,
    },
    TableRemoved {
        table: String,
    },
    ColumnAdded {
        table: String,
        column: ColumnDef,
    },
    ColumnRemoved {
        table: String,
        column: String,
    },
    ColumnChanged {
        table: String,
        column: String,
        /// The column as it must end up, for the generated statement.
        to: ColumnDef,
        differences: Vec<FieldChange>,
    },
    IndexAdded {
        table: String,
        index: IndexDef,
    },
    IndexRemoved {
        table: String,
        index: String,
    },
    ForeignKeyAdded {
        table: String,
        key: ForeignKeyDef,
    },
    ForeignKeyRemoved {
        table: String,
        key: String,
    },
    PrimaryKeyChanged {
        table: String,
        from: Vec<String>,
        to: Vec<String>,
    },
}

impl Change {
    /// Whether applying this loses something that cannot be recovered.
    pub fn destructive(&self) -> bool {
        matches!(
            self,
            Change::TableRemoved { .. } | Change::ColumnRemoved { .. }
        )
    }

    /// The table this change is about, for grouping the report.
    pub fn table(&self) -> &str {
        match self {
            Change::TableAdded { table, .. }
            | Change::TableRemoved { table }
            | Change::ColumnAdded { table, .. }
            | Change::ColumnRemoved { table, .. }
            | Change::ColumnChanged { table, .. }
            | Change::IndexAdded { table, .. }
            | Change::IndexRemoved { table, .. }
            | Change::ForeignKeyAdded { table, .. }
            | Change::ForeignKeyRemoved { table, .. }
            | Change::PrimaryKeyChanged { table, .. } => table,
        }
    }
}

/// What would have to happen to `from` to make it look like `to`.
pub fn diff(from: &SchemaSnapshot, to: &SchemaSnapshot) -> Vec<Change> {
    let mut changes = Vec::new();

    let find = |snapshot: &SchemaSnapshot, name: &str| -> Option<TableDetail> {
        snapshot
            .tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .cloned()
    };

    // Sorted so the script reads the same every time it is generated. A diff
    // that reorders itself between runs cannot be reviewed against the last one.
    let mut names: Vec<&str> = from
        .tables
        .iter()
        .chain(to.tables.iter())
        .map(|t| t.name.as_str())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by_key(|n| n.to_lowercase());

    for name in names {
        match (find(from, name), find(to, name)) {
            (None, Some(wanted)) => changes.push(Change::TableAdded {
                table: wanted.name.clone(),
                columns: wanted.columns.clone(),
                primary_key: wanted.primary_key.clone(),
            }),
            (Some(existing), None) => changes.push(Change::TableRemoved {
                table: existing.name,
            }),
            (Some(existing), Some(wanted)) => {
                compare_table(&existing, &wanted, &mut changes);
            }
            (None, None) => {}
        }
    }

    changes
}

fn compare_table(from: &TableDetail, to: &TableDetail, out: &mut Vec<Change>) {
    let table = to.name.clone();

    // --- columns ----------------------------------------------------------
    for wanted in &to.columns {
        match from
            .columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&wanted.name))
        {
            None => out.push(Change::ColumnAdded {
                table: table.clone(),
                column: wanted.clone(),
            }),
            Some(existing) => {
                let differences = compare_column(existing, wanted);
                if !differences.is_empty() {
                    out.push(Change::ColumnChanged {
                        table: table.clone(),
                        column: wanted.name.clone(),
                        to: wanted.clone(),
                        differences,
                    });
                }
            }
        }
    }
    for existing in &from.columns {
        if !to
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&existing.name))
        {
            out.push(Change::ColumnRemoved {
                table: table.clone(),
                column: existing.name.clone(),
            });
        }
    }

    // --- primary key ------------------------------------------------------
    if !same_columns(&from.primary_key, &to.primary_key) {
        out.push(Change::PrimaryKeyChanged {
            table: table.clone(),
            from: from.primary_key.clone(),
            to: to.primary_key.clone(),
        });
    }

    // --- indexes ----------------------------------------------------------
    // The primary key's own index is skipped: it is reported as a primary key
    // change or not at all, and emitting both would produce a script that drops
    // a constraint by way of its index.
    let indexes = |detail: &TableDetail| -> Vec<IndexDef> {
        detail.indexes.iter().filter(|i| !i.primary).cloned().collect()
    };
    for wanted in indexes(to) {
        match indexes(from)
            .into_iter()
            .find(|i| i.name.eq_ignore_ascii_case(&wanted.name))
        {
            None => out.push(Change::IndexAdded {
                table: table.clone(),
                index: wanted,
            }),
            // An index changed in place is a drop and a create; there is no
            // ALTER INDEX that changes its columns on any engine here.
            Some(existing)
                if !same_columns(&existing.columns, &wanted.columns)
                    || existing.unique != wanted.unique =>
            {
                out.push(Change::IndexRemoved {
                    table: table.clone(),
                    index: existing.name,
                });
                out.push(Change::IndexAdded {
                    table: table.clone(),
                    index: wanted,
                });
            }
            Some(_) => {}
        }
    }
    for existing in indexes(from) {
        if !indexes(to)
            .iter()
            .any(|i| i.name.eq_ignore_ascii_case(&existing.name))
        {
            out.push(Change::IndexRemoved {
                table: table.clone(),
                index: existing.name,
            });
        }
    }

    // --- foreign keys -----------------------------------------------------
    for wanted in &to.foreign_keys {
        if !from
            .foreign_keys
            .iter()
            .any(|k| same_key(k, wanted))
        {
            out.push(Change::ForeignKeyAdded {
                table: table.clone(),
                key: wanted.clone(),
            });
        }
    }
    for existing in &from.foreign_keys {
        if !to.foreign_keys.iter().any(|k| same_key(k, existing)) {
            out.push(Change::ForeignKeyRemoved {
                table: table.clone(),
                key: existing.name.clone(),
            });
        }
    }
}

/// What differs between two versions of a column.
///
/// Compared field by field rather than by equality so the report can say *what*
/// changed. "orders.total differs" sends someone to read two catalogs; "type:
/// numeric(10,2) → numeric(12,2)" does not.
fn compare_column(from: &ColumnDef, to: &ColumnDef) -> Vec<FieldChange> {
    let mut out = Vec::new();

    if !from.type_name.eq_ignore_ascii_case(&to.type_name) {
        out.push(FieldChange {
            field: "type".into(),
            from: from.type_name.clone(),
            to: to.type_name.clone(),
        });
    }
    if from.nullable != to.nullable {
        out.push(FieldChange {
            field: "nullable".into(),
            from: from.nullable.to_string(),
            to: to.nullable.to_string(),
        });
    }
    if normalize_default(&from.default) != normalize_default(&to.default) {
        out.push(FieldChange {
            field: "default".into(),
            from: from.default.clone().unwrap_or_else(|| "none".into()),
            to: to.default.clone().unwrap_or_else(|| "none".into()),
        });
    }
    if from.auto_increment != to.auto_increment {
        out.push(FieldChange {
            field: "auto increment".into(),
            from: from.auto_increment.to_string(),
            to: to.auto_increment.to_string(),
        });
    }

    // Ordinal is deliberately not compared. Column order differs between two
    // schemas that reached the same state by different routes, no engine here
    // can reorder a column without rewriting the table, and reporting it would
    // bury the differences that matter under ones that do not.
    out
}

/// PostgreSQL writes back a default with its type attached, so `'active'` and
/// `'active'::text` are the same default written twice.
fn normalize_default(value: &Option<String>) -> Option<String> {
    let text = value.as_ref()?.trim();
    let base = text.split("::").next().unwrap_or(text).trim();
    Some(base.trim_matches('(').trim_matches(')').to_lowercase())
}

fn same_columns(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Foreign keys are matched on what they do, not what they are called.
///
/// Constraint names are frequently auto-generated and differ between two
/// databases holding the identical constraint; matching on names would report
/// every one of them as removed and re-added.
fn same_key(a: &ForeignKeyDef, b: &ForeignKeyDef) -> bool {
    same_columns(&a.columns, &b.columns)
        && a.referenced_table.eq_ignore_ascii_case(&b.referenced_table)
        && same_columns(&a.referenced_columns, &b.referenced_columns)
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

/// How an engine spells the statements a migration needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect {
    pub quote: char,
    pub alter_column: AlterColumnStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlterColumnStyle {
    /// `ALTER COLUMN x TYPE t`, and nullability as a separate statement.
    Postgres,
    /// `MODIFY COLUMN x t NOT NULL` — the whole definition, restated.
    MySql,
    /// `ALTER COLUMN x t NOT NULL` — like MySQL, different keyword.
    TSql,
}

impl Dialect {
    pub fn for_driver(driver: &str) -> Dialect {
        match driver {
            "mysql" | "mariadb" => Dialect {
                quote: '`',
                alter_column: AlterColumnStyle::MySql,
            },
            "mssql" => Dialect {
                quote: '[',
                alter_column: AlterColumnStyle::TSql,
            },
            // SQLite's ALTER is very limited, but what it does support it
            // spells like PostgreSQL.
            _ => Dialect {
                quote: '"',
                alter_column: AlterColumnStyle::Postgres,
            },
        }
    }
}

/// One statement of a migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Statement {
    pub sql: String,
    /// Whether running it loses data that cannot be recovered.
    pub destructive: bool,
    /// Anything the reader needs to know that the statement does not say.
    pub note: Option<String>,
}

/// Order matters, and this is the order.
///
/// Foreign keys come off first because a column or table cannot be dropped
/// while one points at it, and go back on last because they cannot be added
/// until both sides exist. Tables are dropped at the very end, after everything
/// that might have referenced them is gone.
fn phase(change: &Change) -> u8 {
    match change {
        Change::ForeignKeyRemoved { .. } => 0,
        Change::IndexRemoved { .. } => 1,
        Change::TableAdded { .. } => 2,
        Change::ColumnAdded { .. } => 3,
        Change::ColumnChanged { .. } => 4,
        Change::PrimaryKeyChanged { .. } => 5,
        Change::ColumnRemoved { .. } => 6,
        Change::IndexAdded { .. } => 7,
        Change::ForeignKeyAdded { .. } => 8,
        Change::TableRemoved { .. } => 9,
    }
}

/// Turn a set of changes into statements that apply them.
pub fn migration(changes: &[Change], dialect: Dialect) -> Vec<Statement> {
    let mut ordered: Vec<&Change> = changes.iter().collect();
    // Stable, so within a phase the changes keep the order `diff` produced —
    // which is by table name, so the script reads in the same order as the
    // report beside it.
    ordered.sort_by_key(|c| phase(c));

    ordered
        .into_iter()
        .filter_map(|change| statement_for(change, dialect))
        .collect()
}

fn statement_for(change: &Change, dialect: Dialect) -> Option<Statement> {
    let q = |name: &str| quote_ident(name, dialect.quote);

    let plain = |sql: String| {
        Some(Statement {
            sql,
            destructive: false,
            note: None,
        })
    };

    match change {
        Change::TableAdded {
            table,
            columns,
            primary_key,
        } => {
            let mut parts: Vec<String> = columns
                .iter()
                .map(|c| format!("  {}", column_definition(c, dialect)))
                .collect();
            if !primary_key.is_empty() {
                let keys: Vec<String> = primary_key.iter().map(|c| q(c)).collect();
                parts.push(format!("  PRIMARY KEY ({})", keys.join(", ")));
            }
            plain(format!(
                "CREATE TABLE {} (\n{}\n);",
                q(table),
                parts.join(",\n")
            ))
        }

        Change::TableRemoved { table } => Some(Statement {
            sql: format!("DROP TABLE {};", q(table)),
            destructive: true,
            note: Some(format!("Every row in {table} is lost.")),
        }),

        Change::ColumnAdded { table, column } => {
            // A NOT NULL column added to a table with rows needs a default, and
            // the engine will refuse it otherwise. Saying so here beats the
            // reader finding out when the script stops halfway.
            let note = (!column.nullable && column.default.is_none()).then(|| {
                format!(
                    "{} is NOT NULL with no default; this fails if {table} has rows.",
                    column.name
                )
            });
            Some(Statement {
                sql: format!(
                    "ALTER TABLE {} ADD COLUMN {};",
                    q(table),
                    column_definition(column, dialect)
                ),
                destructive: false,
                note,
            })
        }

        Change::ColumnRemoved { table, column } => Some(Statement {
            sql: format!("ALTER TABLE {} DROP COLUMN {};", q(table), q(column)),
            destructive: true,
            note: Some(format!("Every value in {table}.{column} is lost.")),
        }),

        Change::ColumnChanged {
            table,
            column,
            to,
            differences,
        } => {
            let note = differences
                .iter()
                .map(|d| format!("{}: {} → {}", d.field, d.from, d.to))
                .collect::<Vec<_>>()
                .join(", ");

            let sql = match dialect.alter_column {
                AlterColumnStyle::Postgres => {
                    // Type and nullability are separate statements here, and
                    // only the ones that changed are emitted.
                    let mut lines = Vec::new();
                    if differences.iter().any(|d| d.field == "type") {
                        lines.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
                            q(table),
                            q(column),
                            to.type_name
                        ));
                    }
                    if differences.iter().any(|d| d.field == "nullable") {
                        lines.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} {} NOT NULL;",
                            q(table),
                            q(column),
                            if to.nullable { "DROP" } else { "SET" }
                        ));
                    }
                    if differences.iter().any(|d| d.field == "default") {
                        lines.push(match &to.default {
                            Some(value) => format!(
                                "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {value};",
                                q(table),
                                q(column)
                            ),
                            None => format!(
                                "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                                q(table),
                                q(column)
                            ),
                        });
                    }
                    if lines.is_empty() {
                        return None;
                    }
                    lines.join("\n")
                }
                AlterColumnStyle::MySql => format!(
                    "ALTER TABLE {} MODIFY COLUMN {};",
                    q(table),
                    column_definition(to, dialect)
                ),
                AlterColumnStyle::TSql => format!(
                    "ALTER TABLE {} ALTER COLUMN {};",
                    q(table),
                    column_definition(to, dialect)
                ),
            };

            Some(Statement {
                sql,
                // Narrowing a type truncates, and no catalog says whether this
                // one narrows — so it is flagged for a human rather than
                // guessed at.
                destructive: false,
                note: Some(format!(
                    "{note}. Check the existing values fit before running this."
                )),
            })
        }

        Change::IndexAdded { table, index } => plain(format!(
            "CREATE {}INDEX {} ON {} ({});",
            if index.unique { "UNIQUE " } else { "" },
            q(&index.name),
            q(table),
            index
                .columns
                .iter()
                .map(|c| q(c))
                .collect::<Vec<_>>()
                .join(", ")
        )),

        Change::IndexRemoved { table, index } => {
            // MySQL and SQL Server need the table; PostgreSQL and SQLite refuse
            // it. Same statement, two spellings.
            let sql = match dialect.alter_column {
                AlterColumnStyle::MySql => {
                    format!("DROP INDEX {} ON {};", q(index), q(table))
                }
                AlterColumnStyle::TSql => format!("DROP INDEX {} ON {};", q(index), q(table)),
                AlterColumnStyle::Postgres => format!("DROP INDEX {};", q(index)),
            };
            plain(sql)
        }

        Change::ForeignKeyAdded { table, key } => plain(format!(
            "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
            q(table),
            q(&key.name),
            key.columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", "),
            q(&key.referenced_table),
            key.referenced_columns
                .iter()
                .map(|c| q(c))
                .collect::<Vec<_>>()
                .join(", ")
        )),

        Change::ForeignKeyRemoved { table, key } => plain(match dialect.alter_column {
            AlterColumnStyle::MySql => format!("ALTER TABLE {} DROP FOREIGN KEY {};", q(table), q(key)),
            _ => format!("ALTER TABLE {} DROP CONSTRAINT {};", q(table), q(key)),
        }),

        Change::PrimaryKeyChanged { table, from, to } => {
            let mut lines = Vec::new();
            if !from.is_empty() {
                lines.push(match dialect.alter_column {
                    AlterColumnStyle::MySql => {
                        format!("ALTER TABLE {} DROP PRIMARY KEY;", q(table))
                    }
                    _ => format!(
                        "-- The existing primary key must be dropped by name:\n\
                         -- ALTER TABLE {} DROP CONSTRAINT <constraint name>;",
                        q(table)
                    ),
                });
            }
            if !to.is_empty() {
                lines.push(format!(
                    "ALTER TABLE {} ADD PRIMARY KEY ({});",
                    q(table),
                    to.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ")
                ));
            }
            Some(Statement {
                sql: lines.join("\n"),
                destructive: false,
                note: Some("Changing a primary key rewrites the table on most engines.".into()),
            })
        }
    }
}

/// A column as it appears inside CREATE TABLE or after ADD COLUMN.
fn column_definition(column: &ColumnDef, dialect: Dialect) -> String {
    let mut out = format!(
        "{} {}",
        quote_ident(&column.name, dialect.quote),
        column.type_name
    );
    if !column.nullable {
        out.push_str(" NOT NULL");
    }
    if let Some(default) = &column.default {
        out.push_str(&format!(" DEFAULT {default}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, type_name: &str) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            type_name: type_name.into(),
            nullable: true,
            default: None,
            auto_increment: false,
            ordinal: 1,
            comment: None,
        }
    }

    fn table(name: &str, columns: Vec<ColumnDef>) -> TableDetail {
        TableDetail {
            schema: None,
            name: name.into(),
            columns,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            primary_key: Vec::new(),
            estimated_rows: None,
            comment: None,
        }
    }

    fn snapshot(tables: Vec<TableDetail>) -> SchemaSnapshot {
        SchemaSnapshot {
            label: "test".into(),
            tables,
        }
    }

    const PG: Dialect = Dialect {
        quote: '"',
        alter_column: AlterColumnStyle::Postgres,
    };

    #[test]
    fn the_direction_is_from_to_to() {
        // Backwards, this generates a script that destroys the wrong side.
        let from = snapshot(vec![table("users", vec![column("id", "integer")])]);
        let to = snapshot(vec![
            table("users", vec![column("id", "integer")]),
            table("orders", vec![column("id", "integer")]),
        ]);

        let forward = diff(&from, &to);
        assert!(matches!(forward[0], Change::TableAdded { .. }));

        let backward = diff(&to, &from);
        assert!(matches!(backward[0], Change::TableRemoved { .. }));
    }

    #[test]
    fn identical_schemas_produce_nothing() {
        let a = snapshot(vec![table("users", vec![column("id", "integer")])]);
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn a_changed_column_says_which_field_changed() {
        // "orders.total differs" sends someone to read two catalogs.
        let mut before = column("total", "numeric(10,2)");
        before.nullable = true;
        let mut after = column("total", "numeric(12,2)");
        after.nullable = false;

        let changes = diff(
            &snapshot(vec![table("orders", vec![before])]),
            &snapshot(vec![table("orders", vec![after])]),
        );

        let Change::ColumnChanged { differences, .. } = &changes[0] else {
            panic!("expected a column change, got {:?}", changes[0]);
        };
        assert_eq!(differences.len(), 2);
        assert_eq!(differences[0].field, "type");
        assert_eq!(differences[0].to, "numeric(12,2)");
        assert_eq!(differences[1].field, "nullable");
    }

    #[test]
    fn a_postgres_default_written_with_its_type_is_the_same_default() {
        // PostgreSQL reads back what you wrote as 'active'::text.
        let mut plain = column("status", "text");
        plain.default = Some("'active'".into());
        let mut cast = column("status", "text");
        cast.default = Some("'active'::text".into());

        let changes = diff(
            &snapshot(vec![table("t", vec![plain])]),
            &snapshot(vec![table("t", vec![cast])]),
        );
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn column_order_alone_is_not_a_difference() {
        // Two schemas that reached the same state by different routes have
        // different ordinals, and no engine here can reorder without a rewrite.
        let mut a = column("b", "text");
        a.ordinal = 2;
        let mut b = column("b", "text");
        b.ordinal = 5;
        let changes = diff(
            &snapshot(vec![table("t", vec![a])]),
            &snapshot(vec![table("t", vec![b])]),
        );
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn foreign_keys_are_matched_on_what_they_do_not_what_they_are_called() {
        // Auto-generated constraint names differ between two databases holding
        // the identical constraint.
        let key = |name: &str| ForeignKeyDef {
            name: name.into(),
            columns: vec!["user_id".into()],
            referenced_schema: None,
            referenced_table: "users".into(),
            referenced_columns: vec!["id".into()],
            on_delete: None,
            on_update: None,
        };
        let mut before = table("orders", vec![column("user_id", "integer")]);
        before.foreign_keys = vec![key("fk_orders_1")];
        let mut after = table("orders", vec![column("user_id", "integer")]);
        after.foreign_keys = vec![key("orders_user_id_fkey")];

        assert!(diff(&snapshot(vec![before]), &snapshot(vec![after])).is_empty());
    }

    #[test]
    fn an_index_changed_in_place_becomes_a_drop_and_a_create() {
        // No engine here has an ALTER INDEX that changes its columns.
        let index = |columns: &[&str]| IndexDef {
            name: "ix_orders".into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            unique: false,
            primary: false,
            method: None,
        };
        let mut before = table("orders", Vec::new());
        before.indexes = vec![index(&["a"])];
        let mut after = table("orders", Vec::new());
        after.indexes = vec![index(&["a", "b"])];

        let changes = diff(&snapshot(vec![before]), &snapshot(vec![after]));
        assert_eq!(changes.len(), 2);
        assert!(matches!(changes[0], Change::IndexRemoved { .. }));
        assert!(matches!(changes[1], Change::IndexAdded { .. }));
    }

    #[test]
    fn the_primary_keys_own_index_is_not_reported_twice() {
        let mut before = table("t", vec![column("id", "integer")]);
        before.primary_key = vec!["id".into()];
        before.indexes = vec![IndexDef {
            name: "t_pkey".into(),
            columns: vec!["id".into()],
            unique: true,
            primary: true,
            method: None,
        }];
        let after = before.clone();
        assert!(diff(&snapshot(vec![before]), &snapshot(vec![after])).is_empty());
    }

    #[test]
    fn a_dropped_table_comes_after_the_keys_that_pointed_at_it() {
        // Otherwise the script stops on the first statement.
        let changes = vec![
            Change::TableRemoved {
                table: "users".into(),
            },
            Change::ForeignKeyRemoved {
                table: "orders".into(),
                key: "fk_orders_users".into(),
            },
        ];
        let script = migration(&changes, PG);
        assert!(script[0].sql.contains("DROP CONSTRAINT"), "{:?}", script[0]);
        assert!(script[1].sql.contains("DROP TABLE"));
    }

    #[test]
    fn a_new_table_is_created_before_a_key_that_points_at_it() {
        let changes = vec![
            Change::ForeignKeyAdded {
                table: "orders".into(),
                key: ForeignKeyDef {
                    name: "fk".into(),
                    columns: vec!["user_id".into()],
                    referenced_schema: None,
                    referenced_table: "users".into(),
                    referenced_columns: vec!["id".into()],
                    on_delete: None,
                    on_update: None,
                },
            },
            Change::TableAdded {
                table: "users".into(),
                columns: vec![column("id", "integer")],
                primary_key: vec!["id".into()],
            },
        ];
        let script = migration(&changes, PG);
        assert!(script[0].sql.starts_with("CREATE TABLE"), "{:?}", script[0]);
        assert!(script[1].sql.contains("ADD CONSTRAINT"));
    }

    #[test]
    fn drops_are_marked_destructive_and_say_what_is_lost() {
        let script = migration(
            &[Change::ColumnRemoved {
                table: "users".into(),
                column: "email".into(),
            }],
            PG,
        );
        assert!(script[0].destructive);
        assert!(script[0].note.as_ref().unwrap().contains("lost"));
    }

    #[test]
    fn adding_a_not_null_column_without_a_default_is_flagged() {
        // The engine refuses it on a table with rows, and finding out when the
        // script stops halfway is a worse way to learn it.
        let mut c = column("code", "text");
        c.nullable = false;
        let script = migration(
            &[Change::ColumnAdded {
                table: "users".into(),
                column: c,
            }],
            PG,
        );
        assert!(script[0].note.as_ref().unwrap().contains("NOT NULL"));
    }

    #[test]
    fn postgres_splits_a_column_change_into_the_parts_that_changed() {
        let mut to = column("total", "numeric(12,2)");
        to.nullable = false;
        let script = migration(
            &[Change::ColumnChanged {
                table: "orders".into(),
                column: "total".into(),
                to,
                differences: vec![
                    FieldChange {
                        field: "type".into(),
                        from: "numeric(10,2)".into(),
                        to: "numeric(12,2)".into(),
                    },
                    FieldChange {
                        field: "nullable".into(),
                        from: "true".into(),
                        to: "false".into(),
                    },
                ],
            }],
            PG,
        );
        assert!(script[0].sql.contains("TYPE numeric(12,2)"));
        assert!(script[0].sql.contains("SET NOT NULL"));
        // And nothing about the default, which did not change.
        assert!(!script[0].sql.contains("DEFAULT"));
    }

    #[test]
    fn mysql_restates_the_whole_column_because_that_is_its_syntax() {
        let dialect = Dialect::for_driver("mysql");
        let mut to = column("total", "decimal(12,2)");
        to.nullable = false;
        let script = migration(
            &[Change::ColumnChanged {
                table: "orders".into(),
                column: "total".into(),
                to,
                differences: vec![FieldChange {
                    field: "type".into(),
                    from: "decimal(10,2)".into(),
                    to: "decimal(12,2)".into(),
                }],
            }],
            dialect,
        );
        assert!(script[0].sql.contains("MODIFY COLUMN"));
        assert!(script[0].sql.contains("`total` decimal(12,2) NOT NULL"));
    }

    #[test]
    fn dropping_an_index_needs_the_table_on_mysql_and_not_on_postgres() {
        let change = Change::IndexRemoved {
            table: "orders".into(),
            index: "ix_orders".into(),
        };
        let mysql = migration(std::slice::from_ref(&change), Dialect::for_driver("mysql"));
        assert!(mysql[0].sql.contains("ON `orders`"));

        let postgres = migration(&[change], PG);
        assert_eq!(postgres[0].sql, r#"DROP INDEX "ix_orders";"#);
    }

    #[test]
    fn a_rename_is_reported_as_a_drop_and_an_add() {
        // Not guessed at: the catalog records no link between them, and a wrong
        // guess emits a RENAME that silently discards a real column's data.
        let changes = diff(
            &snapshot(vec![table("t", vec![column("email", "text")])]),
            &snapshot(vec![table("t", vec![column("email_address", "text")])]),
        );
        assert_eq!(changes.len(), 2);
        assert!(matches!(changes[0], Change::ColumnAdded { .. }));
        assert!(matches!(changes[1], Change::ColumnRemoved { .. }));
    }

    #[test]
    fn the_script_reads_the_same_every_time_it_is_generated() {
        let from = snapshot(vec![
            table("zebra", vec![column("id", "integer")]),
            table("apple", vec![column("id", "integer")]),
        ]);
        let to = snapshot(Vec::new());
        let names: Vec<String> = diff(&from, &to)
            .iter()
            .map(|c| c.table().to_string())
            .collect();
        assert_eq!(names, vec!["apple", "zebra"]);
    }
}
