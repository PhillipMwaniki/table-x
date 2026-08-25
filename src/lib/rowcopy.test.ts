import { describe, expect, it } from "vitest";
import { quickFilter, rowsForMenu, sourceTable } from "./rowcopy";
import { matchesFilter, parseFilter } from "./filter";
import type { Column, Value } from "./types";

function column(name: string, table?: string): Column {
  return {
    name,
    type_name: "text",
    ...(table ? { source: { table, column: name } } : {}),
  };
}

describe("rowsForMenu", () => {
  it("acts on the row under the pointer when it is not in the selection", () => {
    // Right-clicking row 7 while rows 1 and 2 are picked is a question about
    // row 7. Answering it about 1 and 2 would be answering a question nobody
    // asked, in a menu whose next item down deletes things.
    expect(rowsForMenu(7, new Set([1, 2]))).toEqual([7]);
    expect(rowsForMenu(7, new Set())).toEqual([7]);
  });

  it("acts on the whole selection when the click lands inside it", () => {
    expect(rowsForMenu(4, new Set([9, 4, 6]))).toEqual([4, 6, 9]);
  });

  it("treats a selection of one as the row it is", () => {
    expect(rowsForMenu(3, new Set([3]))).toEqual([3]);
  });
});

describe("quickFilter", () => {
  /** What the grid does with the expression: parse it, then test a cell. */
  function keeps(expression: string, value: Value): boolean {
    return matchesFilter(value, parseFilter(expression));
  }

  const draft: Value = { kind: "text", value: "draft" };
  const published: Value = { kind: "text", value: "published" };
  const nothing: Value = { kind: "null" };

  it("keeps only the value that was clicked, or only the others", () => {
    // `!=draft` has to read as "negated, then equals", not as a comparison
    // against the text "=draft". The parser strips the ! first; this is the
    // test that says so.
    expect(keeps(quickFilter("equals", "draft"), draft)).toBe(true);
    expect(keeps(quickFilter("equals", "draft"), published)).toBe(false);
    expect(keeps(quickFilter("not", "draft"), draft)).toBe(false);
    expect(keeps(quickFilter("not", "draft"), published)).toBe(true);
  });

  it("matches part of a value for contains", () => {
    expect(keeps(quickFilter("contains", "raf"), draft)).toBe(true);
    expect(keeps(quickFilter("contains", "raf"), published)).toBe(false);
  });

  it("asks about NULL both ways round", () => {
    expect(keeps(quickFilter("null", ""), nothing)).toBe(true);
    expect(keeps(quickFilter("null", ""), draft)).toBe(false);
    expect(keeps(quickFilter("notNull", ""), nothing)).toBe(false);
    expect(keeps(quickFilter("notNull", ""), draft)).toBe(true);
  });

  it("does not read a value that looks like syntax as syntax", () => {
    // A cell holding ">100" is a value, not a comparison. Equals is the item
    // that has to survive it, since it is the one built from the cell.
    const literal: Value = { kind: "text", value: ">100" };
    expect(keeps(quickFilter("equals", ">100"), literal)).toBe(true);
  });
});

describe("sourceTable", () => {
  it("finds the table when every sourced column agrees", () => {
    expect(sourceTable([column("id", "users"), column("name", "users")])).toBe("users");
  });

  it("ignores columns that came from no table at all", () => {
    // `SELECT *, now() FROM users` is still about users.
    expect(sourceTable([column("id", "users"), column("now")])).toBe("users");
  });

  it("names nothing when two tables are involved", () => {
    // A join has no single table to insert into, and picking the first would
    // produce a statement that runs and puts the data in the wrong place.
    expect(sourceTable([column("id", "users"), column("total", "orders")])).toBeNull();
  });

  it("names nothing for a result with no sources", () => {
    expect(sourceTable([column("one"), column("two")])).toBeNull();
    expect(sourceTable([])).toBeNull();
  });
});
