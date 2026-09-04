import { describe, expect, it } from "vitest";
import { findParameters, literal, substitute } from "./params";

const names = (sql: string, driver = "postgres") => findParameters(sql, driver).map((p) => p.name);

describe("findParameters", () => {
  it("finds the spellings application drivers use", () => {
    expect(names("SELECT * FROM t WHERE a = ? AND b = ?")).toEqual(["?1", "?2"]);
    expect(names("SELECT * FROM t WHERE id = :id AND org = :org")).toEqual(["id", "org"]);
    expect(names("SELECT * FROM t WHERE id = ${id}")).toEqual(["id"]);
    expect(names("SELECT * FROM t WHERE id = #{id}")).toEqual(["id"]);
    expect(names("SELECT * FROM t WHERE id = $1 OR id = $2")).toEqual(["$1", "$2"]);
    expect(names("SELECT * FROM t WHERE id = @id", "sqlite")).toEqual(["id"]);
  });

  it("asks for a repeated name once", () => {
    expect(names("SELECT :a, :b, :a")).toEqual(["a", "b"]);
  });

  it("leaves a cast, an assignment and a database link alone", () => {
    expect(names("SELECT id::text FROM t")).toEqual([]);
    expect(names("SELECT a := 1")).toEqual([]);
    expect(names("SELECT * FROM t@remote", "oracle")).toEqual([]);
  });

  it("knows where @ is a variable rather than a hole", () => {
    // MySQL's session variables and SQL Server's declared ones are the
    // statement's own; asking for them would be asking to break it.
    expect(names("SELECT @total := @total + 1", "mysql")).toEqual([]);
    expect(names("SELECT @count", "mssql")).toEqual([]);
    expect(names("SELECT @count", "postgres")).toEqual(["count"]);
  });

  it("does not read $1 as a placeholder off PostgreSQL", () => {
    expect(names("SELECT $1", "mysql")).toEqual([]);
  });

  it("skips strings, comments and quoted names", () => {
    expect(names("SELECT 'price: ?' FROM t -- :note\n WHERE \"a:b\" = 1")).toEqual([]);
    expect(names("SELECT /* ? */ 1")).toEqual([]);
    expect(names("SELECT $$:not$$ FROM t")).toEqual([]);
    expect(names("SELECT 'it''s ?'")).toEqual([]);
  });

  it("does not take PostgreSQL's JSON operators for placeholders", () => {
    expect(names("SELECT * FROM t WHERE data ?| array['a'] OR data ?& array['b']")).toEqual([]);
  });
});

describe("literal", () => {
  it("quotes text and doubles the quotes inside it", () => {
    expect(literal({ type: "text", text: "O'Brien" }, "postgres")).toEqual({
      ok: true,
      sql: "'O''Brien'",
    });
  });

  it("writes a number as typed, so it stays exact", () => {
    expect(literal({ type: "number", text: "12345678901234567890.123" }, "postgres")).toEqual({
      ok: true,
      sql: "12345678901234567890.123",
    });
    expect(literal({ type: "number", text: "twelve" }, "postgres").ok).toBe(false);
  });

  it("spells a boolean the way the engine can read it", () => {
    expect(literal({ type: "boolean", text: "true" }, "postgres")).toEqual({
      ok: true,
      sql: "TRUE",
    });
    expect(literal({ type: "boolean", text: "true" }, "mssql")).toEqual({ ok: true, sql: "1" });
    expect(literal({ type: "boolean", text: "false" }, "oracle")).toEqual({ ok: true, sql: "0" });
  });

  it("passes raw SQL through and refuses an empty one", () => {
    expect(literal({ type: "raw", text: " now() " }, "postgres")).toEqual({
      ok: true,
      sql: "now()",
    });
    expect(literal({ type: "raw", text: "" }, "postgres").ok).toBe(false);
    expect(literal({ type: "null", text: "" }, "postgres")).toEqual({ ok: true, sql: "NULL" });
  });
});

describe("substitute", () => {
  it("fills every hole, in place, and leaves the rest of the text as it was", () => {
    const out = substitute(
      "SELECT * FROM t WHERE a = ? AND b = :b -- ?\n AND c = ?",
      {
        "?1": { type: "number", text: "1" },
        b: { type: "text", text: "x" },
        "?2": { type: "null", text: "" },
      },
      "postgres",
    );
    expect(out).toEqual({
      ok: true,
      sql: "SELECT * FROM t WHERE a = 1 AND b = 'x' -- ?\n AND c = NULL",
    });
  });

  it("refuses to run with a hole left open", () => {
    const out = substitute("SELECT :a, :b", { a: { type: "text", text: "x" } }, "postgres");
    expect(out).toEqual({ ok: false, error: "No value for :b" });
  });
});
