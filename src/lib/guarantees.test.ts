import { describe, expect, it } from "vitest";
import { guaranteesFor, precisionOf, readOnlyExplanation } from "./guarantees";
import type { Column, ResultSet, Value } from "./types";

function column(name: string): Column {
  return { name, type_name: "unknown", nullable: true };
}

function result(columns: string[], rows: Value[][], overrides: Partial<ResultSet> = {}): ResultSet {
  return {
    type: "rows",
    columns: columns.map(column),
    rows,
    truncated: false,
    editable: false,
    key_columns: [],
    ...overrides,
  } as ResultSet;
}

describe("precisionOf", () => {
  it("calls a column exact only when its values arrived as text", () => {
    const rows: Value[][] = [[{ kind: "numeric", value: "123456789.123456789" }]];
    expect(precisionOf(rows, 0)).toBe("exact");
  });

  it("calls a float column approximate, because it already may have rounded", () => {
    expect(precisionOf([[{ kind: "float", value: 0.1 }]], 0)).toBe("approximate");
  });

  it("separates integers from both", () => {
    expect(precisionOf([[{ kind: "int", value: 42 }]], 0)).toBe("integer");
    expect(precisionOf([[{ kind: "u_int", value: 42 }]], 0)).toBe("integer");
  });

  it("skips nulls to find the evidence", () => {
    const rows: Value[][] = [
      [{ kind: "null" }],
      [{ kind: "null" }],
      [{ kind: "numeric", value: "1.5" }],
    ];
    expect(precisionOf(rows, 0)).toBe("exact");
  });

  it("claims nothing when every sampled value was null", () => {
    // Claiming exactness on no evidence is the exact failure this exists to
    // prevent, so an all-null column gets no badge rather than a flattering one.
    expect(precisionOf([[{ kind: "null" }]], 0)).toBe("none");
    expect(precisionOf([], 0)).toBe("none");
  });

  it("reads the values rather than the declared type", () => {
    // SQLite's DECIMAL(20,10) has NUMERIC affinity and is stored as a float, so
    // a column that says decimal can already have been rounded before this
    // application saw it. What came back is the evidence.
    const rows: Value[][] = [[{ kind: "float", value: 1.23 }]];
    expect(precisionOf(rows, 0)).toBe("approximate");
  });
});

describe("guaranteesFor", () => {
  it("separates the exact columns from the approximate ones", () => {
    const rs = result(
      ["id", "total", "rate"],
      [
        [
          { kind: "int", value: 1 },
          { kind: "numeric", value: "10.00" },
          { kind: "float", value: 0.07 },
        ],
      ],
      { key_columns: ["id"], editable: true },
    );

    const g = guaranteesFor(rs);
    expect(g.exact).toEqual(["total"]);
    expect(g.approximate).toEqual(["rate"]);
    expect(g.keyColumns).toEqual(["id"]);
    expect(g.columns[0]?.isKey).toBe(true);
    expect(g.columns[1]?.isKey).toBe(false);
  });

  it("reports a capped result as incomplete", () => {
    const rs = result(["a"], [[{ kind: "int", value: 1 }]], { truncated: true });
    expect(guaranteesFor(rs).complete).toBe(false);
  });
});

describe("readOnlyExplanation", () => {
  const base = {
    connectionReadOnly: false,
    driverName: "SQL Server",
    hasProvenance: true,
    keyColumns: ["id"],
  };

  it("names the connection flag before anything else", () => {
    // It overrides every other consideration, so diagnosing further would send
    // someone to fix a thing that is not the cause.
    const e = readOnlyExplanation({ ...base, connectionReadOnly: true, hasProvenance: false });
    expect(e.reason).toContain("read-only");
    expect(e.remedy).toContain("connection");
  });

  it("blames the driver where the driver is what cannot answer", () => {
    const e = readOnlyExplanation({ ...base, hasProvenance: false });
    expect(e.reason).toContain("SQL Server");
  });

  it("distinguishes no key from too many tables", () => {
    // Different remedies: one is "select the primary key", the other is
    // "stop joining". Rendering both as "read-only" hides which applies.
    const noKey = readOnlyExplanation({ ...base, keyColumns: [] });
    expect(noKey.reason).toContain("identifies a row uniquely");
    expect(noKey.remedy).toContain("primary key");

    const joined = readOnlyExplanation(base);
    expect(joined.reason).toContain("more than one table");
  });
});
