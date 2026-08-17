import { describe, expect, it } from "vitest";
import { parseSaved, shouldAutoRun, toSaved } from "./session";
import type { Tab } from "@/store/workspace";

function tab(overrides: Partial<Tab> = {}): Tab {
  return {
    id: "tab-1",
    kind: "query",
    title: "Query 1",
    database: "app",
    sql: "SELECT 1",
    outcome: null,
    error: null,
    running: false,
    activeStatement: 0,
    offset: 0,
    limit: 1000,
    undo: [],
    redo: [],
    ...overrides,
  } as Tab;
}

describe("toSaved", () => {
  it("keeps the SQL, which is the thing worth not losing", () => {
    const saved = toSaved([tab({ sql: "SELECT * FROM orders WHERE id = 5" })], "tab-1");
    expect(saved.tabs[0]?.sql).toBe("SELECT * FROM orders WHERE id = 5");
    expect(saved.active).toBe(0);
  });

  it("does not keep results", () => {
    // A stored result is a claim about a database as it was whenever the app
    // last closed, shown on reopening as though it were current.
    const saved = toSaved(
      [tab({ outcome: { statements: [], elapsed_ms: 5, notices: [] } })],
      "tab-1",
    );
    expect(JSON.stringify(saved)).not.toContain("elapsed_ms");
  });

  it("does not keep undo history", () => {
    // Offering "undo" for a change made before a restart, against a database
    // that has moved on, is offering something the word does not mean.
    const saved = toSaved(
      [
        tab({
          undo: [
            {
              rowIndex: 0,
              columnIndex: 1,
              before: { kind: "null" },
              after: { kind: "int", value: 2 },
              inverse: { schema: undefined, table: "t", changes: [], key: [] },
            },
          ],
        }),
      ],
      "tab-1",
    );
    expect(JSON.stringify(saved)).not.toContain("inverse");
  });

  it("drops a comparison tab entirely", () => {
    // Its content is a one-shot report about two schemas at one moment, and
    // restoring it would show a diff of something that may no longer be true.
    const saved = toSaved([tab({ kind: "diff", title: "a ⇄ b" }), tab({ id: "tab-2" })], "tab-1");
    expect(saved.tabs).toHaveLength(1);
    expect(saved.tabs[0]?.kind).toBe("query");
  });

  it("keeps the live panels, which cost only a refetch", () => {
    const saved = toSaved(
      [tab({ kind: "activity", title: "Server activity" }), tab({ id: "tab-2", kind: "diagram" })],
      "tab-1",
    );
    expect(saved.tabs.map((t) => t.kind)).toEqual(["activity", "diagram"]);
  });

  it("keeps which side of a table tab was showing", () => {
    const saved = toSaved([tab({ kind: "table", title: "users", view: "structure" })], "tab-1");
    expect(saved.tabs[0]?.view).toBe("structure");
  });

  it("records the active tab by position, since ids are reassigned", () => {
    const saved = toSaved([tab(), tab({ id: "tab-2", title: "Query 2" })], "tab-2");
    expect(saved.active).toBe(1);
  });

  it("falls back to the first tab when the active one was not restorable", () => {
    const saved = toSaved([tab({ id: "tab-9", kind: "diff" }), tab()], "tab-9");
    expect(saved.active).toBe(0);
  });

  it("keeps a notebook's cells", () => {
    const cells = [{ id: "c1", kind: "sql" as const, source: "SELECT 1" }];
    const saved = toSaved([tab({ kind: "notebook", cells })], "tab-1");
    expect(saved.tabs[0]?.cells).toEqual(cells);
  });
});

describe("shouldAutoRun", () => {
  it("never runs a query tab", () => {
    // It holds whatever somebody typed, which may be a DELETE they never ran.
    // Running the editor's contents on launch would execute it without the
    // confirmation every other path insists on.
    expect(
      shouldAutoRun({ kind: "query", title: "Q", database: null, sql: "DELETE FROM users" }),
    ).toBe(false);
  });

  it("runs a table tab, whose statement this application wrote", () => {
    expect(
      shouldAutoRun({ kind: "table", title: "users", database: null, sql: "SELECT * FROM users" }),
    ).toBe(true);
  });

  it("does not run a table tab with no statement", () => {
    expect(shouldAutoRun({ kind: "table", title: "users", database: null, sql: "" })).toBe(false);
  });
});

describe("parseSaved", () => {
  it("reads back what it wrote", () => {
    const saved = toSaved([tab(), tab({ id: "tab-2", kind: "table", title: "users" })], "tab-2");
    const parsed = parseSaved(JSON.parse(JSON.stringify(saved)));
    expect(parsed?.tabs).toHaveLength(2);
    expect(parsed?.active).toBe(1);
  });

  it("refuses anything it does not recognise rather than restoring a broken tab", () => {
    // The file can be hand-edited, left half-written by a crash, or written by
    // an older version.
    expect(parseSaved(null)).toBeNull();
    expect(parseSaved("nonsense")).toBeNull();
    expect(parseSaved({})).toBeNull();
    expect(parseSaved({ tabs: "no" })).toBeNull();
    expect(parseSaved({ tabs: [] })).toBeNull();
  });

  it("drops the malformed tabs and keeps the rest", () => {
    const parsed = parseSaved({
      tabs: [
        { kind: "query", title: "ok", database: null, sql: "SELECT 1" },
        { kind: "query", title: "no sql", database: null },
        { kind: "unknown_kind", title: "x", database: null, sql: "" },
      ],
      active: 0,
    });
    expect(parsed?.tabs).toHaveLength(1);
    expect(parsed?.tabs[0]?.title).toBe("ok");
  });

  it("clamps an out-of-range active index", () => {
    const parsed = parseSaved({
      tabs: [{ kind: "query", title: "ok", database: null, sql: "" }],
      active: 7,
    });
    expect(parsed?.active).toBe(0);
  });
});
