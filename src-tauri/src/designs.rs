//! Schema designs: a diagram you own, rather than a picture of a database.
//!
//! The diagram view draws a live schema and can only ever draw what is there.
//! A design is the other way round — a document that starts from a database or
//! from nothing, is edited freely while no server is involved, and is turned
//! back into a database when it is ready. That is the whole reason it exists:
//! the point of designing is to hold a shape that is not true yet.
//!
//! # Why the tables are `TableDetail`
//!
//! The obvious shape for a design is its own model of a table. It is also the
//! wrong one, because everything worth doing to a design is a comparison with
//! something else: the script that creates it is the difference between nothing
//! and the design, and the script that updates a database is the difference
//! between that database and the design. `diff` already answers both questions,
//! and it answers them about `TableDetail`. Storing anything else would mean a
//! translation layer on both sides of every operation, and two definitions of
//! what a column is.
//!
//! So a design is a snapshot with positions attached, and the engine that
//! compares two databases compares a design and a database without being told
//! that either is unusual.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tablex_core::{
    diagram,
    diff::SchemaSnapshot,
    error::{Error, Result},
    schema::TableDetail,
};

const FILE_NAME: &str = "designs.json";

/// Where one table sits on the canvas.
///
/// Kept beside the tables rather than inside them: a position is a fact about
/// this drawing of the schema, not about the table, and a `TableDetail` that
/// carried coordinates could not be compared with one read from a database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub table: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Design {
    pub id: String,
    pub name: String,
    /// The engine this design is written for.
    ///
    /// A design is not portable and pretending otherwise would be the lie that
    /// matters most here: `AUTO_INCREMENT` and `SERIAL` are not the same thing,
    /// and the script has to pick one. Set when the design is created, from the
    /// connection it was reverse engineered from or from the user's choice.
    pub driver: String,
    /// The schema new tables belong to, where the engine has schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub tables: Vec<TableDetail>,
    /// Positions for the tables that have been moved. Absent means "wherever
    /// the layout puts it", which is what a table nobody has dragged should do.
    #[serde(default)]
    pub layout: Vec<Placement>,
    /// RFC 3339, UTC. Set once and preserved across edits.
    pub created_at: String,
    pub updated_at: String,
}

impl Design {
    /// The design as a side of a comparison.
    ///
    /// Named for the design, because that name is what the diff report shows
    /// above the script somebody is about to run.
    pub fn snapshot(&self) -> SchemaSnapshot {
        SchemaSnapshot {
            label: self.name.clone(),
            tables: self.tables.clone(),
        }
    }

    /// The design laid out, with anything the user has moved left where it was.
    ///
    /// The automatic layout runs first and every saved position overrides it,
    /// so a table that has never been dragged is placed sensibly, a table that
    /// has stays put, and a design that gains a table does not rearrange around
    /// it.
    pub fn diagram(&self) -> diagram::Diagram {
        let mut drawing = diagram::layout(&diagram::graph_of(&self.tables));

        for placed in &self.layout {
            if let Some(item) = drawing
                .boxes
                .iter_mut()
                .find(|b| b.table.eq_ignore_ascii_case(&placed.table))
            {
                item.x = placed.x;
                item.y = placed.y;
            }
        }

        // The canvas has to cover what was moved, or a table dragged to the
        // right of everything else sits outside the scrollable area.
        for item in &drawing.boxes {
            drawing.width = drawing.width.max(item.x + item.width + 24.0);
            drawing.height = drawing.height.max(item.y + item.height + 24.0);
        }

        drawing
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Document {
    version: u32,
    designs: Vec<Design>,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: 1,
            designs: Vec::new(),
        }
    }
}

pub struct DesignStore {
    path: PathBuf,
    designs: Vec<Design>,
}

impl DesignStore {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE_NAME);
        let designs = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Document>(&bytes) {
                Ok(doc) => doc.designs,
                Err(e) => {
                    tracing::error!("{} is not valid JSON ({e}); starting empty", path.display());
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::error!("could not read {}: {e}", path.display());
                Vec::new()
            }
        };

        DesignStore { path, designs }
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Design> {
        let mut out = self.designs.clone();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn get(&self, id: &str) -> Option<Design> {
        self.designs.iter().find(|d| d.id == id).cloned()
    }

    pub fn save(&mut self, mut design: Design) -> Result<Design> {
        if design.name.trim().is_empty() {
            return Err(Error::Config("a design needs a name".into()));
        }
        design.name = design.name.trim().to_string();

        // Positions for tables that are no longer in the design are dropped
        // rather than kept against the day a table of that name comes back: a
        // new table sharing a dropped one's name is a different table, and
        // inheriting its place on the canvas would be a small mystery.
        let names: Vec<&str> = design.tables.iter().map(|t| t.name.as_str()).collect();
        design
            .layout
            .retain(|p| names.iter().any(|n| n.eq_ignore_ascii_case(&p.table)));

        let now = chrono::Utc::now().to_rfc3339();
        design.updated_at = now.clone();

        match self.designs.iter_mut().find(|d| d.id == design.id) {
            Some(existing) => {
                // When the user first drew it, not when they last touched it.
                design.created_at = existing.created_at.clone();
                *existing = design.clone();
            }
            None => {
                if design.created_at.is_empty() {
                    design.created_at = now;
                }
                self.designs.push(design.clone());
            }
        }

        self.write()?;
        Ok(design)
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.designs.retain(|d| d.id != id);
        self.write()
    }

    fn write(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
        }

        let doc = Document {
            version: 1,
            designs: self.designs.clone(),
        };
        let json = serde_json::to_vec_pretty(&doc)?;

        // Temp then rename, like the other stores: this is content somebody
        // drew and cannot reconstruct.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tablex_core::schema::{ColumnDef, ForeignKeyDef};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tablex-designs-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn column(name: &str) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            type_name: "int".into(),
            nullable: false,
            default: None,
            auto_increment: false,
            ordinal: 0,
            comment: None,
        }
    }

    fn table(name: &str, references: Option<&str>) -> TableDetail {
        TableDetail {
            schema: None,
            name: name.into(),
            columns: vec![column("id")],
            indexes: vec![],
            foreign_keys: references
                .map(|target| ForeignKeyDef {
                    name: format!("fk_{name}"),
                    columns: vec!["id".into()],
                    referenced_schema: None,
                    referenced_table: target.into(),
                    referenced_columns: vec!["id".into()],
                    on_delete: None,
                    on_update: None,
                })
                .into_iter()
                .collect(),
            primary_key: vec!["id".into()],
            estimated_rows: None,
            comment: None,
        }
    }

    fn design(id: &str, tables: Vec<TableDetail>) -> Design {
        Design {
            id: id.into(),
            name: "Shop".into(),
            driver: "mysql".into(),
            schema: None,
            tables,
            layout: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_design_compares_as_a_schema_does() {
        // The whole reason a design holds `TableDetail`: the migration engine
        // takes it as one side of a comparison without translation.
        let d = design("a", vec![table("users", None)]);
        let script = tablex_core::diff::migration(
            &tablex_core::diff::diff(&SchemaSnapshot::default(), &d.snapshot()),
            tablex_core::diff::Dialect::for_driver("mysql"),
        );
        assert!(
            script.iter().any(|s| s.sql.contains("CREATE TABLE")),
            "{script:?}"
        );
    }

    #[test]
    fn a_moved_table_stays_where_it_was_put() {
        let mut d = design(
            "a",
            vec![table("users", None), table("orders", Some("users"))],
        );
        let automatic = d.diagram();
        let before = automatic
            .boxes
            .iter()
            .find(|b| b.table == "orders")
            .expect("orders");

        d.layout = vec![Placement {
            table: "orders".into(),
            x: 900.0,
            y: 40.0,
        }];
        let moved = d.diagram();
        let after = moved
            .boxes
            .iter()
            .find(|b| b.table == "orders")
            .expect("orders");

        assert_ne!(before.x, 900.0, "the fixture must not sit there already");
        assert_eq!(after.x, 900.0);
        assert_eq!(after.y, 40.0);
        // And the canvas grew to hold it, or it would be dragged out of reach.
        assert!(moved.width >= 900.0 + after.width, "{}", moved.width);
    }

    #[test]
    fn positions_do_not_outlive_the_tables_they_were_for() {
        let dir = scratch("stale-layout");
        let mut store = DesignStore::load(&dir);

        let mut d = design("a", vec![table("users", None)]);
        d.layout = vec![
            Placement {
                table: "users".into(),
                x: 10.0,
                y: 10.0,
            },
            Placement {
                table: "gone".into(),
                x: 20.0,
                y: 20.0,
            },
        ];

        let saved = store.save(d).expect("save");
        assert_eq!(saved.layout.len(), 1);
        assert_eq!(saved.layout[0].table, "users");
    }

    #[test]
    fn saving_keeps_the_date_it_was_drawn() {
        let dir = scratch("dates");
        let mut store = DesignStore::load(&dir);

        let first = store.save(design("a", vec![])).expect("first");
        assert!(!first.created_at.is_empty());

        let again = store
            .save(Design {
                name: "Renamed".into(),
                ..first.clone()
            })
            .expect("second");
        assert_eq!(again.created_at, first.created_at);
        assert_eq!(again.name, "Renamed");
    }

    #[test]
    fn a_design_needs_a_name() {
        let dir = scratch("unnamed");
        let mut store = DesignStore::load(&dir);
        let mut d = design("a", vec![]);
        d.name = "   ".into();
        assert!(store.save(d).is_err());
    }

    #[test]
    fn designs_survive_a_restart() {
        let dir = scratch("reload");
        {
            let mut store = DesignStore::load(&dir);
            store
                .save(design("a", vec![table("users", None)]))
                .expect("save");
        }
        let store = DesignStore::load(&dir);
        let back = store.get("a").expect("still there");
        assert_eq!(back.tables.len(), 1);
        assert_eq!(back.driver, "mysql");
    }
}
