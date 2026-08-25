/**
 * Editing a design.
 *
 * Every operation here takes a design and returns a new one. Nothing touches a
 * server: a design is a document, and the point of it is that it can be wrong,
 * half-finished, or about a database that does not exist yet.
 *
 * The functions live here rather than in the canvas so that "what does renaming
 * a table do to the foreign keys pointing at it" is a question with a test
 * rather than a question about a React component.
 */

import type { ColumnDef, Design, ForeignKeyDef, TableDetail } from "./types";

/**
 * The type a new table's key gets, per engine.
 *
 * The engine is known — a design is written for one — so the starter table is
 * spelled the way that engine spells things rather than in a lowest common
 * denominator nobody uses. What makes it *generate* values is the
 * `auto_increment` flag; this is only the type beside it.
 */
const KEY_TYPE: Record<string, string> = {
  mysql: "int",
  mariadb: "int",
  postgres: "integer",
  sqlite: "INTEGER",
  mssql: "int",
  clickhouse: "Int32",
};

/** A column with nothing decided about it yet. */
export function blankColumn(name: string, ordinal: number): ColumnDef {
  return {
    name,
    type_name: "text",
    nullable: true,
    auto_increment: false,
    ordinal,
  };
}

/**
 * A name no table in the design is using.
 *
 * Numbered rather than "Untitled": the number is what makes the second one
 * distinguishable from the first at a glance on a canvas.
 */
export function freeTableName(design: Design, stem = "table"): string {
  const taken = new Set(design.tables.map((t) => t.name.toLowerCase()));
  if (!taken.has(stem)) return stem;
  for (let n = 1; ; n++) {
    const candidate = `${stem}_${n}`;
    if (!taken.has(candidate.toLowerCase())) return candidate;
  }
}

/**
 * Add a table, with the key most tables turn out to want.
 *
 * A generated integer primary key rather than an empty box: it is what the vast
 * majority of tables have, it is tedious to type, and deleting a column is
 * quicker than adding one.
 */
export function addTable(design: Design, name = freeTableName(design)): Design {
  const table: TableDetail = {
    schema: design.schema,
    name,
    columns: [
      {
        name: "id",
        type_name: KEY_TYPE[design.driver] ?? "int",
        nullable: false,
        auto_increment: true,
        ordinal: 0,
      },
    ],
    indexes: [],
    foreign_keys: [],
    primary_key: ["id"],
  };
  return { ...design, tables: [...design.tables, table] };
}

/**
 * Rename a table, and every reference to it.
 *
 * A foreign key naming a table that no longer exists is a relation the diagram
 * cannot draw and the script cannot create, and the moment it happens is a
 * rename — so the rename is where it is dealt with.
 */
export function renameTable(design: Design, from: string, to: string): Design {
  const name = to.trim();
  if (!name || name === from) return design;

  return {
    ...design,
    tables: design.tables.map((table) => ({
      ...(table.name === from ? { ...table, name } : table),
      foreign_keys: table.foreign_keys.map((key) =>
        key.referenced_table === from ? { ...key, referenced_table: name } : key,
      ),
    })),
    // The position belongs to the table, whatever it is called now.
    layout: design.layout.map((p) => (p.table === from ? { ...p, table: name } : p)),
  };
}

/**
 * Remove a table, and every foreign key that pointed at it.
 *
 * The keys go because they cannot survive: a reference to a table that is not
 * in the design is not a relation, and leaving them would produce a script that
 * fails on a statement nobody can explain.
 */
export function removeTable(design: Design, name: string): Design {
  return {
    ...design,
    tables: design.tables
      .filter((t) => t.name !== name)
      .map((table) => ({
        ...table,
        foreign_keys: table.foreign_keys.filter((key) => key.referenced_table !== name),
      })),
    layout: design.layout.filter((p) => p.table !== name),
  };
}

/** Replace one table, by name. */
export function withTable(design: Design, name: string, next: TableDetail): Design {
  return { ...design, tables: design.tables.map((t) => (t.name === name ? next : t)) };
}

/** Add a column to a table. */
export function addColumn(table: TableDetail, name?: string): TableDetail {
  const taken = new Set(table.columns.map((c) => c.name.toLowerCase()));
  let candidate = name ?? "column";
  for (let n = 1; taken.has(candidate.toLowerCase()); n++) candidate = `column_${n}`;
  return { ...table, columns: [...table.columns, blankColumn(candidate, table.columns.length)] };
}

/**
 * Change one column, keeping everything that names it in step.
 *
 * Renaming a column has to follow through to the primary key and to any foreign
 * key that uses it, for the same reason renaming a table does: a key naming a
 * column that no longer exists cannot be created.
 */
export function updateColumn(table: TableDetail, at: number, next: ColumnDef): TableDetail {
  const before = table.columns[at];
  if (!before) return table;

  const columns = table.columns.map((c, i) => (i === at ? next : c));
  const renamed = before.name !== next.name;

  return {
    ...table,
    columns,
    primary_key: renamed
      ? table.primary_key.map((c) => (c === before.name ? next.name : c))
      : table.primary_key,
    foreign_keys: renamed
      ? table.foreign_keys.map((key) => ({
          ...key,
          columns: key.columns.map((c) => (c === before.name ? next.name : c)),
        }))
      : table.foreign_keys,
  };
}

/** Remove a column, and anything that depended on it. */
export function removeColumn(table: TableDetail, at: number): TableDetail {
  const gone = table.columns[at];
  if (!gone) return table;

  return {
    ...table,
    columns: table.columns.filter((_, i) => i !== at).map((c, i) => ({ ...c, ordinal: i })),
    primary_key: table.primary_key.filter((c) => c !== gone.name),
    // A key over a column that is no longer there is not a key over anything.
    foreign_keys: table.foreign_keys.filter((key) => !key.columns.includes(gone.name)),
  };
}

/** Put a column in the primary key, or take it out. */
export function togglePrimaryKey(table: TableDetail, name: string): TableDetail {
  const inKey = table.primary_key.includes(name);
  return {
    ...table,
    primary_key: inKey
      ? table.primary_key.filter((c) => c !== name)
      : // Appended rather than sorted: the order of a composite key is part of
        // it, and it is the order they were chosen in.
        [...table.primary_key, name],
  };
}

/**
 * Point a column at another table.
 *
 * Named after both ends, so two relations between the same pair of tables do
 * not collide — which happens the moment a table has both `author_id` and
 * `editor_id` against the same `users`.
 */
export function addForeignKey(
  table: TableDetail,
  column: string,
  target: TableDetail,
): TableDetail {
  const to = target.primary_key[0] ?? target.columns[0]?.name;
  if (!to) return table;

  const key: ForeignKeyDef = {
    name: `fk_${table.name}_${column}_${target.name}`,
    columns: [column],
    referenced_table: target.name,
    referenced_columns: [to],
  };
  return {
    ...table,
    // One relation per column: pointing a column somewhere new replaces where
    // it pointed before rather than adding a second, contradictory key.
    foreign_keys: [...table.foreign_keys.filter((k) => !k.columns.includes(column)), key],
  };
}

/** Stop a column pointing anywhere. */
export function removeForeignKey(table: TableDetail, column: string): TableDetail {
  return {
    ...table,
    foreign_keys: table.foreign_keys.filter((k) => !k.columns.includes(column)),
  };
}
