/**
 * What this application is promising about the result on screen.
 *
 * All of it is already true — exact numerics are carried as text end to end, an
 * edit is refused unless it matches exactly one row, editability is derived
 * from column provenance rather than assumed. None of it is visible, which
 * makes it worth nothing to anyone deciding whether to trust the tool.
 *
 * So this derives the specific claims for the specific result in front of you.
 * Specific matters: "we are careful with decimals" is marketing, and "total and
 * balance are carried exactly; rate went through a float because the server
 * typed it as one" is a fact someone can check.
 */

import type { Column, ResultSet, Value } from "./types";

/** How a column's numbers survived the trip. */
export type Precision =
  /** Carried as text, digit for digit. Nothing rounded it. */
  | "exact"
  /** Went through a 64-bit float, because the column is one. */
  | "approximate"
  /** A whole number, exact within its width. */
  | "integer"
  /** Not a number. */
  | "none";

export interface ColumnGuarantee {
  name: string;
  precision: Precision;
  /** Whether an edit would use this column to find its row. */
  isKey: boolean;
}

export interface Guarantees {
  columns: ColumnGuarantee[];
  /** Columns whose values cannot lose a digit. */
  exact: string[];
  /** Columns that passed through a float, and so may already have. */
  approximate: string[];
  keyColumns: string[];
  editable: boolean;
  /** Whether the rows shown are all the statement produced. */
  complete: boolean;
}

/**
 * Read a column's precision off the values that actually arrived.
 *
 * The declared type name would be easier and would be wrong often enough to
 * matter: SQLite's `DECIMAL(20,10)` has NUMERIC affinity and is stored as a
 * float, so a column that *says* decimal can still have been rounded before
 * this application ever saw it. What came back is the evidence.
 */
export function precisionOf(rows: Value[][], columnIndex: number): Precision {
  // Sampled rather than scanned: a column is one type in practice, and reading
  // a hundred thousand rows to label a header is not worth the frame.
  const sample = rows.slice(0, 200);

  for (const row of sample) {
    const cell = row[columnIndex];
    if (!cell || cell.kind === "null") continue;
    switch (cell.kind) {
      case "numeric":
        return "exact";
      case "float":
        return "approximate";
      case "int":
      case "u_int":
        return "integer";
      default:
        return "none";
    }
  }
  // Every sampled row was NULL, so there is no evidence either way — and
  // claiming exactness on no evidence is exactly the kind of thing this is
  // meant to stop.
  return "none";
}

export function guaranteesFor(result: ResultSet): Guarantees {
  const columns: ColumnGuarantee[] = result.columns.map((column: Column, index) => ({
    name: column.name,
    precision: precisionOf(result.rows, index),
    isKey: result.key_columns.includes(column.name),
  }));

  return {
    columns,
    exact: columns.filter((c) => c.precision === "exact").map((c) => c.name),
    approximate: columns.filter((c) => c.precision === "approximate").map((c) => c.name),
    keyColumns: result.key_columns,
    editable: result.editable,
    complete: !result.truncated,
  };
}

/**
 * Why this result cannot be edited, and what would change that.
 *
 * Three different situations that all render as "read-only" today, and the
 * remedy differs for each — so guessing which one applies is guessing what to
 * do about it.
 */
export function readOnlyExplanation(options: {
  connectionReadOnly: boolean;
  driverName: string;
  hasProvenance: boolean;
  keyColumns: string[];
}): { reason: string; remedy: string } {
  if (options.connectionReadOnly) {
    return {
      reason: "This connection is marked read-only.",
      remedy:
        "The flag is on the connection, not the database. Edit the connection to turn it off — deliberately, since it is usually on for a reason.",
    };
  }

  if (!options.hasProvenance) {
    return {
      reason: `${options.driverName} does not report which table each column came from.`,
      remedy:
        "Without that, an UPDATE cannot be aimed safely, so editing stays off for every result on this driver rather than being offered and getting it wrong.",
    };
  }

  if (options.keyColumns.length === 0) {
    return {
      reason: "This result has no column that identifies a row uniquely.",
      remedy:
        "Include the table's primary key, or a column covered by a unique index over non-nullable columns, and editing turns on.",
    };
  }

  return {
    reason: "This result draws from more than one table.",
    remedy: "A join gives no single target for an UPDATE. Query one table to edit its rows.",
  };
}
