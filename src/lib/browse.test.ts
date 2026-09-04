import { describe, expect, it } from "vitest";
import { browseStatement, clause, tableStatement } from "./browse";

describe("clause", () => {
  it("trims and drops a trailing semicolon", () => {
    expect(clause("  id > 5;  ", "WHERE")).toBe("id > 5");
  });

  it("drops the keyword when it was typed into its own box", () => {
    // `WHERE id = 1` in a box labelled WHERE is a natural thing to type.
    expect(clause("WHERE id = 1", "WHERE")).toBe("id = 1");
    expect(clause("where id = 1", "WHERE")).toBe("id = 1");
    expect(clause("order  by  joined DESC", "ORDER BY")).toBe("joined DESC");
  });

  it("leaves a column that happens to start with the keyword alone", () => {
    // `where_from` is a column, not the keyword followed by nothing.
    expect(clause("where_from = 'x'", "WHERE")).toBe("where_from = 'x'");
    expect(clause("orderby_col", "ORDER BY")).toBe("orderby_col");
  });

  it("is empty for an empty box", () => {
    expect(clause("   ", "WHERE")).toBe("");
    expect(clause(";", "ORDER BY")).toBe("");
  });
});

describe("browseStatement", () => {
  it("is the base statement alone with nothing typed", () => {
    expect(browseStatement("SELECT * FROM t", {})).toBe("SELECT * FROM t");
    expect(browseStatement("SELECT * FROM t;", { where: " " })).toBe("SELECT * FROM t");
  });

  it("appends each clause in the order the engine wants them", () => {
    expect(browseStatement("SELECT * FROM t", { where: "a = 1", orderBy: "b DESC" })).toBe(
      "SELECT * FROM t WHERE a = 1 ORDER BY b DESC",
    );
    expect(browseStatement("SELECT * FROM t", { orderBy: "b" })).toBe("SELECT * FROM t ORDER BY b");
  });
});

describe("tableStatement", () => {
  it("composes a table tab and leaves every other tab as written", () => {
    expect(
      tableStatement({ kind: "table", sql: "SELECT * FROM t", where: "x", orderBy: "y" }),
    ).toBe("SELECT * FROM t WHERE x ORDER BY y");
    // A query tab may have typed its own WHERE; nothing is appended to it.
    expect(tableStatement({ kind: "query", sql: "SELECT 1", where: "x" })).toBe("SELECT 1");
  });
});
