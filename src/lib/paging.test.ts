import { describe, expect, it } from "vitest";
import { hasOrderBy } from "./paging";

describe("hasOrderBy", () => {
  it("finds an ordinary trailing clause", () => {
    expect(hasOrderBy("SELECT * FROM users ORDER BY id")).toBe(true);
    expect(hasOrderBy("select * from users order by id desc")).toBe(true);
  });

  it("is false for the query that actually needs the warning", () => {
    // This is the shape a table tab opens with, and the one whose page two can
    // repeat rows from page one.
    expect(hasOrderBy("SELECT * FROM users")).toBe(false);
    expect(hasOrderBy("SELECT * FROM users WHERE active")).toBe(false);
  });

  it("does not count an ORDER BY that orders something else", () => {
    // A subquery's ordering says nothing about the order rows come back in.
    expect(hasOrderBy("SELECT * FROM (SELECT * FROM t ORDER BY id) x")).toBe(false);
    // Neither does a window function's.
    expect(
      hasOrderBy("SELECT row_number() OVER (ORDER BY id) FROM t"),
    ).toBe(false);
  });

  it("counts the outer clause when a subquery has one too", () => {
    expect(hasOrderBy("SELECT * FROM (SELECT * FROM t ORDER BY a) x ORDER BY b")).toBe(true);
  });

  it("ignores the words inside a string literal", () => {
    expect(hasOrderBy("SELECT 'order by' FROM t")).toBe(false);
    expect(hasOrderBy("SELECT * FROM t WHERE note = 'please order by date'")).toBe(false);
    // A doubled quote is an escape, not the end of the literal.
    expect(hasOrderBy("SELECT 'it''s order by' FROM t")).toBe(false);
  });

  it("ignores a commented-out clause", () => {
    expect(hasOrderBy("SELECT * FROM t -- ORDER BY id")).toBe(false);
    expect(hasOrderBy("SELECT * FROM t /* ORDER BY id */")).toBe(false);
    // And still sees a real one after a comment that mentions it.
    expect(hasOrderBy("SELECT * FROM t /* no ORDER BY here */ ORDER BY id")).toBe(true);
  });

  it("does not match a word that merely ends in the clause", () => {
    expect(hasOrderBy("SELECT * FROM t REORDER BY id")).toBe(false);
  });

  it("handles a quoted identifier containing the words", () => {
    expect(hasOrderBy('SELECT * FROM "order by"')).toBe(false);
  });

  it("is false for nothing at all rather than throwing", () => {
    expect(hasOrderBy("")).toBe(false);
    expect(hasOrderBy("   ")).toBe(false);
  });
});
