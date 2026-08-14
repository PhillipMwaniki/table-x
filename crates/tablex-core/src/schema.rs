//! Database schema description.
//!
//! Deliberately hierarchical and lazy: a production database can hold tens of
//! thousands of objects, so the tree is fetched one level at a time rather than
//! introspected wholesale on connect.

use serde::{Deserialize, Serialize};

/// One level of the object tree. Databases disagree about how many levels there
/// are — PostgreSQL has database → schema → table, MySQL has database → table,
/// SQLite has just tables — so the UI renders whatever depth the driver reports
/// instead of assuming a fixed shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Database,
    Schema,
    /// A grouping level that exists only in the tree — "Tables", "Views",
    /// "Functions". Databases have no such object; it is how a schema with
    /// three hundred tables and four triggers stays navigable.
    Folder,
    Table,
    View,
    MaterializedView,
    Column,
    Index,
    ForeignKey,
    Function,
    Procedure,
    Trigger,
    Sequence,
    Enum,
    /// Non-relational containers (Redis keyspaces, Mongo collections).
    Collection,
}

/// A node in the lazily-expanded object tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaNode {
    /// Stable identifier, and the argument for requesting this node's children.
    ///
    /// Built with [`encode_path`] rather than joined with dots: object names may
    /// contain the separator, and a tree that mis-parses `my.table` addresses
    /// the wrong object.
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    /// `true` when the node can be expanded. Distinct from "has zero children" —
    /// we do not know the child count until we ask.
    pub expandable: bool,
    /// Populated only once the node has been expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<SchemaNode>>,
    /// Extra display detail: a column's type, a table's estimated row count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The object's name as SQL should refer to it, already quoted and qualified
    /// by the driver — `"public"."users"`, `` `app`.`orders` ``.
    ///
    /// The driver builds it because only the driver knows how many levels its
    /// engine qualifies by and which quote character it uses. The alternative,
    /// reassembling it in the frontend from a path, means teaching the UI five
    /// dialects' quoting rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualified: Option<String>,
}

impl SchemaNode {
    /// A leaf node whose id is the encoding of `path`.
    ///
    /// Builders rather than struct literals: a driver builds these in a dozen
    /// places, and six fields of which four are usually defaults buries the two
    /// that matter.
    pub fn new(path: &[&str], name: impl Into<String>, kind: NodeKind) -> Self {
        SchemaNode {
            id: encode_path(path),
            name: name.into(),
            kind,
            expandable: false,
            children: None,
            detail: None,
            qualified: None,
        }
    }

    pub fn expandable(mut self) -> Self {
        self.expandable = true;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attach the name SQL should use for this object, already quoted.
    pub fn qualified(mut self, qualified: impl Into<String>) -> Self {
        self.qualified = Some(qualified.into());
        self
    }
}

/// Join path segments into a [`SchemaNode::id`].
///
/// `/` separates, `\` escapes both itself and the separator. Object names really
/// do contain slashes and dots — PostgreSQL and MySQL permit any character in a
/// quoted identifier — so the encoding has to survive them rather than assume
/// they are rare.
pub fn encode_path(segments: &[&str]) -> String {
    segments
        .iter()
        .map(|s| s.replace('\\', "\\\\").replace('/', "\\/"))
        .collect::<Vec<_>>()
        .join("/")
}

/// Split a [`SchemaNode::id`] back into its segments.
pub fn decode_path(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in id.chars() {
        match (escaped, ch) {
            (true, c) => {
                current.push(c);
                escaped = false;
            }
            (false, '\\') => escaped = true,
            (false, '/') => out.push(std::mem::take(&mut current)),
            (false, c) => current.push(c),
        }
    }
    out.push(current);
    out
}

/// Full detail for a single table, loaded when the user opens its structure tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDetail {
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
    /// Primary key column names, in key order.
    pub primary_key: Vec<String>,
    /// Estimated row count. Explicitly an estimate — an exact `COUNT(*)` on a
    /// large table is far too expensive to run just to populate a sidebar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_rows: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Identity / auto-increment / serial.
    pub auto_increment: bool,
    /// Position in the table, 1-based.
    pub ordinal: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyDef {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_schema: Option<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
}

impl TableDetail {
    /// Columns usable as an edit key: the primary key if there is one, otherwise
    /// the first unique index over non-nullable columns. Returns empty when no
    /// safe key exists, which forces the grid into read-only mode.
    pub fn edit_key(&self) -> Vec<String> {
        if !self.primary_key.is_empty() {
            return self.primary_key.clone();
        }
        self.indexes
            .iter()
            .find(|ix| {
                ix.unique
                    && !ix.columns.is_empty()
                    && ix.columns.iter().all(|c| {
                        self.columns
                            .iter()
                            .any(|col| col.name == *c && !col.nullable)
                    })
            })
            .map(|ix| ix.columns.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, nullable: bool) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            type_name: "text".into(),
            nullable,
            default: None,
            auto_increment: false,
            ordinal: 1,
            comment: None,
        }
    }

    fn table(columns: Vec<ColumnDef>, indexes: Vec<IndexDef>, pk: Vec<String>) -> TableDetail {
        TableDetail {
            schema: Some("public".into()),
            name: "t".into(),
            columns,
            indexes,
            foreign_keys: vec![],
            primary_key: pk,
            estimated_rows: None,
            comment: None,
        }
    }

    #[test]
    fn primary_key_is_preferred() {
        let t = table(vec![column("id", false)], vec![], vec!["id".into()]);
        assert_eq!(t.edit_key(), vec!["id".to_string()]);
    }

    #[test]
    fn falls_back_to_a_non_nullable_unique_index() {
        let t = table(
            vec![column("email", false)],
            vec![IndexDef {
                name: "uq_email".into(),
                columns: vec!["email".into()],
                unique: true,
                primary: false,
                method: None,
            }],
            vec![],
        );
        assert_eq!(t.edit_key(), vec!["email".to_string()]);
    }

    #[test]
    fn nullable_unique_index_is_rejected() {
        // NULLs are not equal to each other, so a nullable unique column cannot
        // safely address exactly one row.
        let t = table(
            vec![column("email", true)],
            vec![IndexDef {
                name: "uq_email".into(),
                columns: vec!["email".into()],
                unique: true,
                primary: false,
                method: None,
            }],
            vec![],
        );
        assert!(t.edit_key().is_empty());
    }

    #[test]
    fn table_with_no_key_is_not_editable() {
        let t = table(vec![column("note", true)], vec![], vec![]);
        assert!(t.edit_key().is_empty());
    }

    #[test]
    fn a_path_round_trips() {
        let path = ["app", "public", "tables", "users"];
        assert_eq!(decode_path(&encode_path(&path)), path);
    }

    #[test]
    fn separators_inside_names_survive() {
        // Quoted identifiers accept any character, so a table really can be
        // called `a/b`. Splitting on a raw separator would address `a` instead,
        // and the user would be shown the wrong object's columns.
        let path = ["my/db", "schema\\weird", "tables", "a/b"];
        assert_eq!(decode_path(&encode_path(&path)), path);
    }

    #[test]
    fn a_single_segment_needs_no_separator() {
        assert_eq!(encode_path(&["users"]), "users");
        assert_eq!(decode_path("users"), vec!["users".to_string()]);
    }

    #[test]
    fn an_empty_segment_is_preserved() {
        // PostgreSQL's default schema arrives as an empty string in some paths;
        // dropping it would shift every later segment one position left.
        assert_eq!(decode_path(&encode_path(&["", "users"])), vec!["", "users"]);
    }

    #[test]
    fn builders_default_to_a_plain_leaf() {
        let node = SchemaNode::new(&["app", "users"], "users", NodeKind::Table);
        assert!(!node.expandable);
        assert!(node.qualified.is_none());

        let node = node.expandable().qualified("\"app\".\"users\"").detail("120 rows");
        assert!(node.expandable);
        assert_eq!(node.qualified.as_deref(), Some("\"app\".\"users\""));
        assert_eq!(node.detail.as_deref(), Some("120 rows"));
    }
}
