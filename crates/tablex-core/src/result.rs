//! Query results and column metadata.

use crate::value::Value;
use serde::{Deserialize, Serialize};

/// Metadata for one column of a result set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    /// Label as returned by the database (respects `AS` aliases).
    pub name: String,
    /// The database's own type name, shown verbatim in the UI (`int4`, `VARCHAR(255)`).
    pub type_name: String,
    /// Whether the database reports this column as nullable, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    /// Provenance of the column, when the database tells us. Required for
    /// inline editing: without knowing the source table and column we cannot
    /// build a safe `UPDATE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ColumnSource>,
}

/// Where a result column actually came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSource {
    pub schema: Option<String>,
    pub table: String,
    pub column: String,
}

/// The outcome of executing one statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatementResult {
    /// A statement that returned rows (`SELECT`, `RETURNING`, `SHOW`).
    Rows(ResultSet),
    /// A statement that reported an affected-row count (`INSERT`, `UPDATE`, `DDL`).
    Affected {
        rows_affected: u64,
        /// Some databases report a generated key.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_insert_id: Option<i64>,
    },
}

/// A materialized page of rows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    /// True when the driver stopped at the requested row cap and more rows exist.
    /// The UI shows a "load more" affordance rather than implying completeness.
    pub truncated: bool,
    /// Whether these rows can be edited in place. False for joins, aggregates,
    /// and any result whose columns lack a single unambiguous source table.
    pub editable: bool,
    /// Columns that uniquely identify a row, used to build `UPDATE ... WHERE`.
    /// Empty when the result has no usable key, which forces read-only mode.
    pub key_columns: Vec<String>,
}

/// Everything one execution produced. A single editor submission may contain
/// several statements separated by semicolons, so this is a list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryOutcome {
    pub statements: Vec<StatementResult>,
    /// Wall-clock execution time in milliseconds.
    pub elapsed_ms: u64,
    /// Non-fatal messages the server emitted (PostgreSQL `NOTICE`, MySQL warnings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

impl ResultSet {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Index of a column by name, for building edit statements.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// A result is only editable when it has both a usable key and known provenance.
    /// Callers should prefer this over setting `editable` by hand.
    pub fn recompute_editable(&mut self) {
        let single_table = {
            let mut tables = self
                .columns
                .iter()
                .filter_map(|c| c.source.as_ref())
                .map(|s| (s.schema.clone(), s.table.clone()));
            match tables.next() {
                None => false,
                Some(first) => tables.all(|t| t == first),
            }
        };
        self.editable = single_table && !self.key_columns.is_empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, table: Option<&str>) -> Column {
        Column {
            name: name.to_string(),
            type_name: "text".into(),
            nullable: Some(true),
            source: table.map(|t| ColumnSource {
                schema: Some("public".into()),
                table: t.to_string(),
                column: name.to_string(),
            }),
        }
    }

    #[test]
    fn single_table_result_with_a_key_is_editable() {
        let mut rs = ResultSet {
            columns: vec![col("id", Some("users")), col("email", Some("users"))],
            key_columns: vec!["id".into()],
            ..Default::default()
        };
        rs.recompute_editable();
        assert!(rs.editable);
    }

    #[test]
    fn joins_are_not_editable() {
        // Columns from two tables give no single target for an UPDATE.
        let mut rs = ResultSet {
            columns: vec![col("id", Some("users")), col("total", Some("orders"))],
            key_columns: vec!["id".into()],
            ..Default::default()
        };
        rs.recompute_editable();
        assert!(!rs.editable, "a join must never be silently editable");
    }

    #[test]
    fn results_without_a_key_are_read_only() {
        // Without a unique key an UPDATE could hit more rows than the user edited.
        let mut rs = ResultSet {
            columns: vec![col("email", Some("users"))],
            key_columns: vec![],
            ..Default::default()
        };
        rs.recompute_editable();
        assert!(!rs.editable);
    }

    #[test]
    fn computed_columns_without_provenance_are_read_only() {
        // e.g. SELECT count(*) — no source table at all.
        let mut rs = ResultSet {
            columns: vec![col("count", None)],
            key_columns: vec!["count".into()],
            ..Default::default()
        };
        rs.recompute_editable();
        assert!(!rs.editable);
    }
}
