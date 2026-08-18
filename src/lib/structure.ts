/**
 * Turning structure edits into `Change`s, and showing what the table will look
 * like once they run.
 *
 * The editor stages `Change` values rather than a mutated `TableDetail`, because
 * `Change` is what the backend already consumes: the same union the schema diff
 * produces, the same `migration()` that renders it per dialect. An editor that
 * built its own edit model would need its own DDL writer to go with it, and two
 * writers is one too many.
 *
 * Everything here is pure, so the parts worth getting right — which field counts
 * as changed, what the table looks like with edits pending — can be tested
 * without a database or a browser.
 */

import type { Change, ColumnDef, FieldChange, TableDetail } from "./types";

/**
 * Which fields of a column differ, in the shape the backend expects.
 *
 * The field names are load-bearing rather than labels: PostgreSQL emits one
 * `ALTER TABLE` line per changed field and picks them by these exact strings, so
 * they mirror `compare_column` in `diff.rs`. A typo here produces a migration
 * that quietly skips the change it was asked for.
 */
export function columnDifferences(from: ColumnDef, to: ColumnDef): FieldChange[] {
  const out: FieldChange[] = [];

  // Case-insensitive, matching the backend: `TEXT` and `text` are the same type,
  // and reporting them as a change would generate a pointless rewrite.
  if (from.type_name.toLowerCase() !== to.type_name.toLowerCase()) {
    out.push({ field: "type", from: from.type_name, to: to.type_name });
  }
  if (from.nullable !== to.nullable) {
    out.push({ field: "nullable", from: String(from.nullable), to: String(to.nullable) });
  }
  if ((from.default ?? "") !== (to.default ?? "")) {
    out.push({ field: "default", from: from.default ?? "none", to: to.default ?? "none" });
  }
  return out;
}

/** How a staged change reads in the pending list. */
export function describeChange(change: Change): string {
  switch (change.kind) {
    case "column_added":
      return `add column ${change.column.name}`;
    case "column_removed":
      return `drop column ${change.column}`;
    case "column_changed":
      return `alter ${change.column} (${change.differences.map((d) => d.field).join(", ")})`;
    case "index_added":
      return `add index ${change.index.name}`;
    case "index_removed":
      return `drop index ${change.index}`;
    case "foreign_key_added":
      return `add foreign key ${change.key.name}`;
    case "foreign_key_removed":
      return `drop foreign key ${change.key}`;
    default:
      // The remaining variants are refused by the backend before they can be
      // staged, so this is a label for something that should not arrive.
      return change.kind.replace(/_/g, " ");
  }
}

/** What a name is affected by, so a row can show its own state. */
export type RowState = "added" | "removed" | "changed" | undefined;

/**
 * The table as it will be once the pending changes run.
 *
 * Rendered from this rather than from the edits directly, so a staged change is
 * visible in place — a dropped column struck through where it was, a new one in
 * the list with the others — instead of only as a line in a summary somewhere
 * else on screen.
 */
export function withPending(
  detail: TableDetail,
  pending: Change[],
): { detail: TableDetail; state: Map<string, RowState> } {
  const state = new Map<string, RowState>();
  let columns = [...detail.columns];
  let indexes = [...detail.indexes];
  let foreignKeys = [...detail.foreign_keys];

  for (const change of pending) {
    switch (change.kind) {
      case "column_added":
        columns = [...columns, change.column];
        state.set(`column:${change.column.name}`, "added");
        break;
      case "column_removed":
        // Kept in the list and marked, not filtered out. A row that vanishes on
        // click gives nothing to undo from and no way to see what is going.
        state.set(`column:${change.column}`, "removed");
        break;
      case "column_changed":
        columns = columns.map((c) => (c.name === change.column ? change.to : c));
        // An added column edited again stays "added": it does not exist yet, so
        // there is nothing for "changed" to mean.
        if (state.get(`column:${change.column}`) !== "added") {
          state.set(`column:${change.column}`, "changed");
        }
        break;
      case "index_added":
        indexes = [...indexes, change.index];
        state.set(`index:${change.index.name}`, "added");
        break;
      case "index_removed":
        state.set(`index:${change.index}`, "removed");
        break;
      case "foreign_key_added":
        foreignKeys = [...foreignKeys, change.key];
        state.set(`fk:${change.key.name}`, "added");
        break;
      case "foreign_key_removed":
        state.set(`fk:${change.key}`, "removed");
        break;
      default:
        break;
    }
  }

  return {
    detail: { ...detail, columns, indexes, foreign_keys: foreignKeys },
    state,
  };
}

/**
 * Drop a staged change, and anything that only made sense alongside it.
 *
 * Removing the "add column" for a column that was then edited has to take the
 * edit with it, or the backend is handed an `ALTER COLUMN` for a column that
 * will not exist.
 */
export function discard(pending: Change[], index: number): Change[] {
  const target = pending[index];
  if (!target) return pending;

  const orphaned =
    target.kind === "column_added"
      ? (name: Change) => name.kind === "column_changed" && name.column === target.column.name
      : () => false;

  return pending.filter((change, i) => i !== index && !orphaned(change));
}

/** A blank column, as the add form starts. */
export function blankColumn(ordinal: number): ColumnDef {
  return {
    name: "",
    type_name: "",
    nullable: true,
    default: undefined,
    auto_increment: false,
    ordinal,
    comment: undefined,
  };
}
