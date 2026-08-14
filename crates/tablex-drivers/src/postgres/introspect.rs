//! PostgreSQL catalog introspection.
//!
//! Every query here reads `pg_catalog` rather than `information_schema`. The
//! standard views are portable but markedly slower on large catalogs, and they
//! omit PostgreSQL-specific detail the UI wants — index method, partitioning, and
//! the relkind distinction between tables, views, and materialized views.

use super::{map_err, OidMap};
use std::collections::HashMap;
use tablex_core::{
    driver::CompletionScope,
    error::{Error, Result},
    schema::{decode_path, ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
    sql::quote_ident,
};
use tokio_postgres::Client;

/// Schemas that exist on every server and are noise for the user.
const HIDDEN_SCHEMAS: &str = "'pg_catalog', 'information_schema', 'pg_toast'";

/// One level of the object tree.
///
/// The path shape is `[]`, `[database]`, `[database, schema]`,
/// `[database, schema, folder]`, then `[database, schema, folder, object]` for a
/// table's columns.
///
/// The database level is real but only one of its entries can be expanded: a
/// PostgreSQL connection is bound to one database for its lifetime, so the other
/// databases are listed and marked, and opening one reconnects. `current` is the
/// database this connection is actually attached to.
pub async fn browse(
    client: &Client,
    current: &str,
    parent: Option<&str>,
) -> Result<Vec<SchemaNode>> {
    let path = parent.map(decode_path).unwrap_or_default();
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();

    match segments.as_slice() {
        [] => browse_databases(client, current).await,
        [db] => browse_schemas(client, db).await,
        [db, schema] => Ok(FOLDERS
            .iter()
            .map(|f| SchemaNode::new(&[db, schema, f.id], f.label, NodeKind::Folder).expandable())
            .collect()),
        [db, schema, folder] => match FOLDERS.iter().find(|f| f.id == *folder) {
            Some(spec) => browse_folder(client, db, schema, spec).await,
            None => Ok(Vec::new()),
        },
        [db, schema, folder, object] => browse_columns(client, db, schema, folder, object).await,
        _ => Ok(Vec::new()),
    }
}

/// Databases on this server, with the connected one marked.
async fn browse_databases(client: &Client, current: &str) -> Result<Vec<SchemaNode>> {
    // datallowconn excludes template0, which by design refuses connections.
    let rows = client
        .query(
            "SELECT datname FROM pg_catalog.pg_database \
             WHERE datallowconn AND NOT datistemplate ORDER BY datname",
            &[],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let is_current = name == current;
            let node = SchemaNode::new(&[&name], &name, NodeKind::Database)
                .expandable()
                .qualified(quote_ident(&name, '"'));
            // Only the attached database can be expanded without reconnecting.
            // Saying so is the difference between "click to open" and a node
            // that mysteriously fails.
            if is_current {
                node.detail("connected")
            } else {
                node
            }
        })
        .collect())
}

async fn browse_schemas(client: &Client, db: &str) -> Result<Vec<SchemaNode>> {
    let sql = format!(
        "SELECT nspname FROM pg_catalog.pg_namespace \
         WHERE nspname NOT IN ({HIDDEN_SCHEMAS}) AND nspname NOT LIKE 'pg_temp%' \
           AND nspname NOT LIKE 'pg_toast_temp%' \
         ORDER BY nspname"
    );
    let rows = client.query(sql.as_str(), &[]).await.map_err(map_err)?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            SchemaNode::new(&[db, &name], &name, NodeKind::Schema)
                .expandable()
                .qualified(quote_ident(&name, '"'))
        })
        .collect())
}

/// One folder of objects, and the catalogue query that fills it.
struct Folder {
    id: &'static str,
    label: &'static str,
    kind: NodeKind,
}

const FOLDERS: &[Folder] = &[
    Folder {
        id: "tables",
        label: "Tables",
        kind: NodeKind::Table,
    },
    Folder {
        id: "views",
        label: "Views",
        kind: NodeKind::View,
    },
    Folder {
        id: "matviews",
        label: "Materialized views",
        kind: NodeKind::MaterializedView,
    },
    Folder {
        id: "functions",
        label: "Functions",
        kind: NodeKind::Function,
    },
    Folder {
        id: "procedures",
        label: "Procedures",
        kind: NodeKind::Procedure,
    },
    Folder {
        id: "triggers",
        label: "Triggers",
        kind: NodeKind::Trigger,
    },
    Folder {
        id: "sequences",
        label: "Sequences",
        kind: NodeKind::Sequence,
    },
];

async fn browse_folder(
    client: &Client,
    db: &str,
    schema: &str,
    spec: &Folder,
) -> Result<Vec<SchemaNode>> {
    match spec.id {
        "functions" | "procedures" => browse_routines(client, db, schema, spec).await,
        "triggers" => browse_triggers(client, db, schema, spec).await,
        _ => browse_relations(client, db, schema, spec).await,
    }
}

/// Tables, views, materialized views, and sequences all live in `pg_class`,
/// separated by `relkind`.
async fn browse_relations(
    client: &Client,
    db: &str,
    schema: &str,
    spec: &Folder,
) -> Result<Vec<SchemaNode>> {
    // Partitions are hidden: they clutter the list and are reachable through
    // the partitioned table that owns them.
    let relkinds: &[&str] = match spec.id {
        "tables" => &["r", "p", "f"],
        "views" => &["v"],
        "matviews" => &["m"],
        "sequences" => &["S"],
        _ => &[],
    };
    let list = relkinds
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "SELECT c.relname, \
                CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END \
         FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relkind IN ({list}) AND NOT c.relispartition \
         ORDER BY c.relname"
    );
    let rows = client
        .query(sql.as_str(), &[&schema])
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let estimated: Option<i64> = r.get(1);
            let node = SchemaNode::new(&[db, schema, spec.id, &name], &name, spec.kind.clone())
                .qualified(qualify(schema, &name));
            // A sequence has no columns to expand into.
            let node = if spec.id == "sequences" {
                node
            } else {
                node.expandable()
            };
            // Explicitly an estimate from the planner statistics. An exact
            // COUNT(*) per table would make expanding a schema unusable.
            match estimated {
                Some(n) if spec.id != "sequences" => node.detail(format!("~{n} rows")),
                _ => node,
            }
        })
        .collect())
}

async fn browse_routines(
    client: &Client,
    db: &str,
    schema: &str,
    spec: &Folder,
) -> Result<Vec<SchemaNode>> {
    // prokind: f function, p procedure, a aggregate, w window. Aggregates and
    // window functions are listed with the functions - they are things you call.
    let kinds = if spec.id == "procedures" {
        "'p'"
    } else {
        "'f','a','w'"
    };
    let sql = format!(
        "SELECT p.proname, pg_catalog.pg_get_function_identity_arguments(p.oid) \
         FROM pg_catalog.pg_proc p \
         JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = $1 AND p.prokind IN ({kinds}) \
         ORDER BY p.proname"
    );
    let rows = client
        .query(sql.as_str(), &[&schema])
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let args: String = r.get(1);
            // Overloads share a name, so the argument list is what tells them
            // apart — and it is what you need to call one.
            SchemaNode::new(
                &[db, schema, spec.id, &format!("{name}({args})")],
                &name,
                spec.kind.clone(),
            )
            .detail(format!("({args})"))
            .qualified(qualify(schema, &name))
        })
        .collect())
}

async fn browse_triggers(
    client: &Client,
    db: &str,
    schema: &str,
    spec: &Folder,
) -> Result<Vec<SchemaNode>> {
    // tgisinternal excludes the triggers PostgreSQL creates to enforce foreign
    // keys, which are an implementation detail of the constraint.
    let rows = client
        .query(
            "SELECT t.tgname, c.relname FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND NOT t.tgisinternal \
             ORDER BY c.relname, t.tgname",
            &[&schema],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let table: String = r.get(1);
            SchemaNode::new(
                &[db, schema, spec.id, &format!("{table}.{name}")],
                &name,
                spec.kind.clone(),
            )
            .detail(format!("on {table}"))
        })
        .collect())
}

/// `"schema"."name"`, quoted so it can be pasted straight into a statement.
fn qualify(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(schema, '"'), quote_ident(name, '"'))
}

async fn browse_columns(
    client: &Client,
    db: &str,
    schema: &str,
    folder: &str,
    table: &str,
) -> Result<Vec<SchemaNode>> {
    let columns = columns(client, schema, table).await?;
    Ok(columns
        .into_iter()
        .map(|c| {
            SchemaNode::new(
                &[db, schema, folder, table, &c.name],
                &c.name,
                NodeKind::Column,
            )
            .detail(if c.nullable {
                c.type_name
            } else {
                format!("{} NOT NULL", c.type_name)
            })
        })
        .collect())
}

pub async fn columns(client: &Client, schema: &str, table: &str) -> Result<Vec<ColumnDef>> {
    let rows = client
        .query(
            "SELECT a.attname, \
                    pg_catalog.format_type(a.atttypid, a.atttypmod), \
                    NOT a.attnotnull, \
                    pg_catalog.pg_get_expr(d.adbin, d.adrelid), \
                    a.attidentity <> '' OR pg_catalog.pg_get_expr(d.adbin, d.adrelid) \
                        LIKE 'nextval(%', \
                    a.attnum, \
                    col_description(a.attrelid, a.attnum) \
             FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE n.nspname = $1 AND c.relname = $2 \
               AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| ColumnDef {
            name: r.get(0),
            type_name: r.get(1),
            nullable: r.get(2),
            default: r.get(3),
            auto_increment: r.get::<_, Option<bool>>(4).unwrap_or(false),
            ordinal: r.get::<_, i16>(5) as i32,
            comment: r.get(6),
        })
        .collect())
}

/// Column name → base type name, used to build casts for inline edits.
pub async fn column_types(
    client: &Client,
    schema: &str,
    table: &str,
) -> Result<HashMap<String, String>> {
    let rows = client
        .query(
            "SELECT a.attname, t.typname \
             FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
             WHERE n.nspname = $1 AND c.relname = $2 \
               AND a.attnum > 0 AND NOT a.attisdropped",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?;

    if rows.is_empty() {
        return Err(Error::Unsupported(format!(
            "table {schema}.{table} not found or not visible"
        )));
    }
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

pub async fn primary_key(client: &Client, schema: &str, table: &str) -> Result<Vec<String>> {
    let rows = client
        .query(
            "SELECT a.attname \
             FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class c ON c.oid = i.indrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_catalog.pg_attribute a \
                  ON a.attrelid = c.oid AND a.attnum = ANY(i.indkey) \
             WHERE n.nspname = $1 AND c.relname = $2 AND i.indisprimary \
             ORDER BY array_position(i.indkey, a.attnum)",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

pub async fn indexes(client: &Client, schema: &str, table: &str) -> Result<Vec<IndexDef>> {
    let rows = client
        .query(
            "SELECT ic.relname, i.indisunique, i.indisprimary, am.amname, \
                    ARRAY( \
                      SELECT a.attname FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord) \
                      JOIN pg_catalog.pg_attribute a \
                        ON a.attrelid = c.oid AND a.attnum = k.attnum \
                      ORDER BY k.ord \
                    ) \
             FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class c  ON c.oid  = i.indrelid \
             JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_catalog.pg_am am ON am.oid = ic.relam \
             WHERE n.nspname = $1 AND c.relname = $2 \
             ORDER BY ic.relname",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| IndexDef {
            name: r.get(0),
            unique: r.get(1),
            primary: r.get(2),
            method: r.get(3),
            columns: r.get(4),
        })
        .collect())
}

pub async fn foreign_keys(
    client: &Client,
    schema: &str,
    table: &str,
) -> Result<Vec<ForeignKeyDef>> {
    let rows = client
        .query(
            "SELECT con.conname, \
                    ARRAY( \
                      SELECT a.attname FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                      JOIN pg_catalog.pg_attribute a \
                        ON a.attrelid = con.conrelid AND a.attnum = k.attnum \
                      ORDER BY k.ord), \
                    fn.nspname, fc.relname, \
                    ARRAY( \
                      SELECT a.attname FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                      JOIN pg_catalog.pg_attribute a \
                        ON a.attrelid = con.confrelid AND a.attnum = k.attnum \
                      ORDER BY k.ord), \
                    con.confdeltype, con.confupdtype \
             FROM pg_catalog.pg_constraint con \
             JOIN pg_catalog.pg_class c  ON c.oid  = con.conrelid \
             JOIN pg_catalog.pg_namespace n  ON n.oid  = c.relnamespace \
             JOIN pg_catalog.pg_class fc ON fc.oid = con.confrelid \
             JOIN pg_catalog.pg_namespace fn ON fn.oid = fc.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 AND con.contype = 'f' \
             ORDER BY con.conname",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| ForeignKeyDef {
            name: r.get(0),
            columns: r.get(1),
            referenced_schema: r.get(2),
            referenced_table: r.get(3),
            referenced_columns: r.get(4),
            on_delete: Some(referential_action(r.get::<_, i8>(5))),
            on_update: Some(referential_action(r.get::<_, i8>(6))),
        })
        .collect())
}

/// Expand the single-character action code stored in `pg_constraint`.
fn referential_action(code: i8) -> String {
    match code as u8 as char {
        'a' => "NO ACTION",
        'r' => "RESTRICT",
        'c' => "CASCADE",
        'n' => "SET NULL",
        'd' => "SET DEFAULT",
        _ => "UNKNOWN",
    }
    .to_string()
}

pub async fn table_detail(client: &Client, schema: &str, table: &str) -> Result<TableDetail> {
    let columns = columns(client, schema, table).await?;
    if columns.is_empty() {
        return Err(Error::Unsupported(format!(
            "table {schema}.{table} not found or not visible"
        )));
    }

    let estimated_rows = client
        .query_opt(
            "SELECT CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?
        .and_then(|r| r.get(0));

    let comment = client
        .query_opt(
            "SELECT obj_description(c.oid) \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2",
            &[&schema, &table],
        )
        .await
        .map_err(map_err)?
        .and_then(|r| r.get(0));

    Ok(TableDetail {
        schema: Some(schema.to_string()),
        name: table.to_string(),
        columns,
        indexes: indexes(client, schema, table).await?,
        foreign_keys: foreign_keys(client, schema, table).await?,
        primary_key: primary_key(client, schema, table).await?,
        estimated_rows,
        comment,
    })
}

/// Map table OIDs to `(schema, table)` for result-column provenance.
pub async fn resolve_table_oids(client: &Client, oids: &[u32]) -> Result<OidMap> {
    if oids.is_empty() {
        return Ok(OidMap::new());
    }
    // OIDs arrive as u32 but the catalog compares against `oid`; casting the
    // parameter array keeps this to a single round trip regardless of width.
    let as_i64: Vec<i64> = oids.iter().map(|o| *o as i64).collect();
    let rows = client
        .query(
            "SELECT c.oid::int8, n.nspname, c.relname \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.oid = ANY($1::int8[]::oid[])",
            &[&as_i64],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let oid: i64 = r.get(0);
            (oid as u32, (r.get(1), r.get(2)))
        })
        .collect())
}

pub async fn completion_scope(client: &Client) -> Result<CompletionScope> {
    let schemas: Vec<String> = client
        .query(
            &format!(
                "SELECT nspname FROM pg_catalog.pg_namespace \
                 WHERE nspname NOT IN ({HIDDEN_SCHEMAS}) ORDER BY nspname"
            ),
            &[],
        )
        .await
        .map_err(map_err)?
        .iter()
        .map(|r| r.get(0))
        .collect();

    // One query for every table's columns rather than one per table: on a schema
    // with a few thousand tables the round trips dominate everything else.
    let rows = client
        .query(
            &format!(
                "SELECT n.nspname || '.' || c.relname, \
                        ARRAY_AGG(a.attname ORDER BY a.attnum) \
                 FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid \
                 WHERE c.relkind IN ('r','p','v','m','f') \
                   AND n.nspname NOT IN ({HIDDEN_SCHEMAS}) \
                   AND a.attnum > 0 AND NOT a.attisdropped \
                 GROUP BY 1 ORDER BY 1"
            ),
            &[],
        )
        .await
        .map_err(map_err)?;

    let tables = rows.iter().map(|r| (r.get(0), r.get(1))).collect();

    let functions: Vec<String> = client
        .query(
            &format!(
                "SELECT DISTINCT p.proname FROM pg_catalog.pg_proc p \
                 JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
                 WHERE n.nspname NOT IN ({HIDDEN_SCHEMAS}) OR n.nspname = 'pg_catalog' \
                 ORDER BY p.proname LIMIT 2000"
            ),
            &[],
        )
        .await
        .map_err(map_err)?
        .iter()
        .map(|r| r.get(0))
        .collect();

    Ok(CompletionScope {
        schemas,
        tables,
        functions,
        keywords: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referential_actions_expand_to_sql_keywords() {
        assert_eq!(referential_action(b'c' as i8), "CASCADE");
        assert_eq!(referential_action(b'n' as i8), "SET NULL");
        assert_eq!(referential_action(b'a' as i8), "NO ACTION");
        assert_eq!(referential_action(b'r' as i8), "RESTRICT");
        assert_eq!(referential_action(b'd' as i8), "SET DEFAULT");
        // An unrecognized code must not panic or silently claim NO ACTION.
        assert_eq!(referential_action(b'?' as i8), "UNKNOWN");
    }
}
