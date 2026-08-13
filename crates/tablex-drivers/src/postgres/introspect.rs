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
    schema::{ColumnDef, ForeignKeyDef, IndexDef, NodeKind, SchemaNode, TableDetail},
};
use tokio_postgres::Client;

/// Schemas that exist on every server and are noise for the user.
const HIDDEN_SCHEMAS: &str = "'pg_catalog', 'information_schema', 'pg_toast'";

/// One level of the object tree.
///
/// `parent` is `None` for the roots (schemas), a schema name for its tables, or
/// `schema.table` for that table's columns.
pub async fn browse(client: &Client, parent: Option<&str>) -> Result<Vec<SchemaNode>> {
    match parent {
        None => browse_schemas(client).await,
        Some(path) => match path.split_once('.') {
            Some((schema, table)) => browse_columns(client, schema, table).await,
            None => browse_tables(client, path).await,
        },
    }
}

async fn browse_schemas(client: &Client) -> Result<Vec<SchemaNode>> {
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
            SchemaNode {
                id: name.clone(),
                name,
                kind: NodeKind::Schema,
                expandable: true,
                children: None,
                detail: None,
            }
        })
        .collect())
}

async fn browse_tables(client: &Client, schema: &str) -> Result<Vec<SchemaNode>> {
    // relkind: r ordinary table, p partitioned table, v view, m materialized view,
    // f foreign table. Partitions themselves (relispartition) are hidden because
    // they clutter the tree and are reachable through their parent.
    let rows = client
        .query(
            "SELECT c.relname, c.relkind, \
                    CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint END \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relkind IN ('r','p','v','m','f') \
               AND NOT c.relispartition \
             ORDER BY c.relname",
            &[&schema],
        )
        .await
        .map_err(map_err)?;

    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let relkind: i8 = r.get(1);
            let estimated: Option<i64> = r.get(2);
            SchemaNode {
                id: format!("{schema}.{name}"),
                name,
                kind: match relkind as u8 as char {
                    'v' => NodeKind::View,
                    'm' => NodeKind::MaterializedView,
                    _ => NodeKind::Table,
                },
                expandable: true,
                children: None,
                // Explicitly an estimate from the planner statistics. An exact
                // COUNT(*) per table would make expanding a schema unusable.
                detail: estimated.map(|n| format!("~{n} rows")),
            }
        })
        .collect())
}

async fn browse_columns(client: &Client, schema: &str, table: &str) -> Result<Vec<SchemaNode>> {
    let columns = columns(client, schema, table).await?;
    Ok(columns
        .into_iter()
        .map(|c| SchemaNode {
            id: format!("{schema}.{table}.{}", c.name),
            name: c.name.clone(),
            kind: NodeKind::Column,
            expandable: false,
            children: None,
            detail: Some(if c.nullable {
                c.type_name
            } else {
                format!("{} NOT NULL", c.type_name)
            }),
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
