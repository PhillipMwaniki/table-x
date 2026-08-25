import { describe, expect, it } from "vitest";
import { blankColumn, columnDifferences, discard, stageChange, withPending } from "./structure";
import type { Change, ColumnDef, TableDetail, TriggerDef } from "./types";

function trigger(name: string, extra: Partial<TriggerDef> = {}): TriggerDef {
  return {
    name,
    timing: "before",
    events: ["update"],
    for_each_row: true,
    body: "SET NEW.updated_at = NOW();",
    ...extra,
  };
}

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

  it("lists a staged trigger on a table the driver read none for", () => {
    // `triggers` is absent, not empty, when the driver does not read them --
    // and a table that has none reads the same way. Staging one has to produce
    // a list either way, or the new trigger is staged and never shown.
    const trigger = {
      name: "orders_touch",
      timing: "before" as const,
      events: ["update" as const],
      for_each_row: true,
      body: "SET NEW.updated_at = NOW();",
    };
    const { detail, state } = withPending(TABLE, [
      { kind: "trigger_added", table: "orders", trigger },
    ]);
    expect(detail.triggers?.map((t) => t.name)).toEqual(["orders_touch"]);
    expect(state.get("trigger:orders_touch")).toBe("added");
  });

  it("shows a dropped trigger in place, like a dropped column", () => {
    const trigger = {
      name: "orders_audit",
      timing: "after" as const,
      events: ["insert" as const],
      for_each_row: true,
      body: "INSERT INTO audit VALUES (NEW.id);",
    };
    const { detail, state } = withPending({ ...TABLE, triggers: [trigger] }, [
      { kind: "trigger_removed", table: "orders", trigger: "orders_audit" },
    ]);
    expect(detail.triggers?.map((t) => t.name)).toEqual(["orders_audit"]);
    expect(state.get("trigger:orders_audit")).toBe("removed");
  });

  it("shows an edited trigger as it will read, marked changed", () => {
    const { detail, state } = withPending({ ...TABLE, triggers: [trigger("orders_touch")] }, [
      {
        kind: "trigger_changed",
        table: "orders",
        trigger: trigger("orders_touch", { body: "SET NEW.updated_at = NOW(); -- and audit" }),
      },
    ]);
    expect(detail.triggers?.[0]?.body).toContain("and audit");
    expect(state.get("trigger:orders_touch")).toBe("changed");
  });

  it("leaves the original untouched", () => {
    // The staged view is derived every render; mutating the fetched detail would
    // make a discarded edit unrecoverable without refetching.
    const before = JSON.stringify(TABLE);
    withPending(TABLE, [{ kind: "column_removed", table: "orders", column: "total" }]);
    expect(JSON.stringify(TABLE)).toBe(before);
  });
});

describe("stageChange", () => {
  it("keeps only the last edit to a trigger", () => {
    // Two ALTERs of one trigger is the second one and a statement nobody asked
    // to run. The migration would apply both, in order, to the same object.
    const once = stageChange([], {
      kind: "trigger_changed",
      table: "orders",
      trigger: trigger("orders_touch", { body: "first" }),
    });
    const twice = stageChange(once, {
      kind: "trigger_changed",
      table: "orders",
      trigger: trigger("orders_touch", { body: "second" }),
    });
    expect(twice).toHaveLength(1);
    expect(twice[0]).toMatchObject({ trigger: { body: "second" } });
  });

  it("takes the edit out when the trigger is dropped instead", () => {
    // This one is not tidiness. The migration runs drops before redefinitions,
    // so an edit left beside a drop is an ALTER against something the script
    // has already removed -- and it fails partway through a migration.
    const edited = stageChange([], {
      kind: "trigger_changed",
      table: "orders",
      trigger: trigger("orders_touch"),
    });
    const dropped = stageChange(edited, {
      kind: "trigger_removed",
      table: "orders",
      trigger: "orders_touch",
    });
    expect(dropped).toEqual([
      { kind: "trigger_removed", table: "orders", trigger: "orders_touch" },
    ]);
  });

  it("edits a staged trigger in place rather than staging an alter", () => {
    // The server has never seen it, so there is nothing to alter: a CREATE and
    // then an ALTER of the same trigger is two statements doing one thing.
    const added = stageChange([], {
      kind: "trigger_added",
      table: "orders",
      trigger: trigger("orders_touch", { body: "first" }),
    });
    const edited = stageChange(added, {
      kind: "trigger_changed",
      table: "orders",
      trigger: trigger("orders_touch", { body: "second" }),
    });
    expect(edited).toHaveLength(1);
    expect(edited[0]).toMatchObject({ kind: "trigger_added", trigger: { body: "second" } });
  });

  it("leaves a different trigger alone", () => {
    const other: Change = {
      kind: "trigger_changed",
      table: "orders",
      trigger: trigger("orders_audit"),
    };
    const out = stageChange([other], {
      kind: "trigger_removed",
      table: "orders",
      trigger: "orders_touch",
    });
    expect(out).toHaveLength(2);
  });

  it("appends anything that is not a trigger", () => {
    const drop: Change = { kind: "column_removed", table: "orders", column: "total" };
    expect(stageChange([], drop)).toEqual([drop]);
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
