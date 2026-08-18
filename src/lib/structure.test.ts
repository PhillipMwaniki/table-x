import { describe, expect, it } from "vitest";
import { blankColumn, columnDifferences, discard, withPending } from "./structure";
import type { Change, ColumnDef, TableDetail } from "./types";

function column(name: string, type_name: string, extra: Partial<ColumnDef> = {}): ColumnDef {
  return { ...blankColumn(1), name, type_name, ...extra };
}

const TABLE: TableDetail = {
  schema: "public",
  name: "orders",
  columns: [column("id", "integer", { nullable: false }), column("total", "numeric")],
  indexes: [{ name: "orders_pkey", columns: ["id"], unique: true, primary: true }],
  foreign_keys: [],
  primary_key: ["id"],
  estimated_rows: undefined,
  comment: undefined,
};

describe("columnDifferences", () => {
  it("names fields the way the backend's emitter expects", () => {
    // Not labels: PostgreSQL emits one ALTER line per changed field and selects
    // them by these exact strings, so a rename here silently drops the change.
    const diffs = columnDifferences(
      column("total", "numeric", { nullable: true, default: undefined }),
      column("total", "text", { nullable: false, default: "''" }),
    );
    expect(diffs.map((d) => d.field)).toEqual(["type", "nullable", "default"]);
  });

  it("treats a type that differs only in case as unchanged", () => {
    // Matching the backend, which compares case-insensitively. Reporting this
    // would generate a table rewrite that changes nothing.
    expect(columnDifferences(column("a", "TEXT"), column("a", "text"))).toEqual([]);
  });

  it("reports a cleared default rather than ignoring it", () => {
    // null and "" both mean "no default" to the form, but going from a default
    // to none is a real change and has to survive the round trip.
    const diffs = columnDifferences(
      column("a", "text", { default: "'x'" }),
      column("a", "text", { default: undefined }),
    );
    expect(diffs).toEqual([{ field: "default", from: "'x'", to: "none" }]);
  });
});

describe("withPending", () => {
  it("shows a dropped column in place rather than removing it", () => {
    // A row that disappears on click leaves nothing to undo from and no way to
    // see what is about to go.
    const pending: Change[] = [{ kind: "column_removed", table: "orders", column: "total" }];
    const { detail, state } = withPending(TABLE, pending);
    expect(detail.columns.map((c) => c.name)).toEqual(["id", "total"]);
    expect(state.get("column:total")).toBe("removed");
  });

  it("shows an added column alongside the real ones", () => {
    const added = column("note", "text");
    const { detail, state } = withPending(TABLE, [
      { kind: "column_added", table: "orders", column: added },
    ]);
    expect(detail.columns.map((c) => c.name)).toEqual(["id", "total", "note"]);
    expect(state.get("column:note")).toBe("added");
  });

  it("keeps an edited new column marked as added", () => {
    // It does not exist yet, so "changed" would be a claim about a column the
    // database has never seen.
    const added = column("note", "text");
    const { state } = withPending(TABLE, [
      { kind: "column_added", table: "orders", column: added },
      {
        kind: "column_changed",
        table: "orders",
        column: "note",
        to: column("note", "varchar(64)"),
        differences: [{ field: "type", from: "text", to: "varchar(64)" }],
      },
    ]);
    expect(state.get("column:note")).toBe("added");
  });

  it("leaves the original untouched", () => {
    // The staged view is derived every render; mutating the fetched detail would
    // make a discarded edit unrecoverable without refetching.
    const before = JSON.stringify(TABLE);
    withPending(TABLE, [{ kind: "column_removed", table: "orders", column: "total" }]);
    expect(JSON.stringify(TABLE)).toBe(before);
  });
});

describe("discard", () => {
  it("takes an edit to a new column with the column", () => {
    // Otherwise the backend gets an ALTER COLUMN for a column that will not
    // exist, and the apply fails on a statement the user never asked for.
    const pending: Change[] = [
      { kind: "column_added", table: "orders", column: column("note", "text") },
      {
        kind: "column_changed",
        table: "orders",
        column: "note",
        to: column("note", "varchar(64)"),
        differences: [{ field: "type", from: "text", to: "varchar(64)" }],
      },
    ];
    expect(discard(pending, 0)).toEqual([]);
  });

  it("leaves an edit to an existing column alone", () => {
    const pending: Change[] = [
      { kind: "column_added", table: "orders", column: column("note", "text") },
      {
        kind: "column_changed",
        table: "orders",
        column: "total",
        to: column("total", "text"),
        differences: [{ field: "type", from: "numeric", to: "text" }],
      },
    ];
    expect(discard(pending, 0)).toHaveLength(1);
  });

  it("is a no-op for an index that is not there", () => {
    const pending: Change[] = [{ kind: "index_removed", table: "orders", index: "i" }];
    expect(discard(pending, 7)).toEqual(pending);
  });
});
