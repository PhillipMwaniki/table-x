import { describe, expect, it } from "vitest";
import { drop, insertInto, PREVIEW_LIMIT, selectFrom, truncate } from "./statements";

describe("selectFrom", () => {
  it("limits rows the way the engine does", () => {
    expect(selectFrom("`app`.`users`", "mysql")).toBe(
      `SELECT * FROM \`app\`.\`users\` LIMIT ${PREVIEW_LIMIT};`,
    );
    // SQL Server had no LIMIT until 2012 and still prefers TOP; OFFSET/FETCH
    // needs an ORDER BY, and there is no sensible default for one.
    expect(selectFrom("[db].[dbo].[users]", "mssql")).toBe(
      `SELECT TOP ${PREVIEW_LIMIT} * FROM [db].[dbo].[users];`,
    );
  });

  it("takes the qualified name as given", () => {
    // The driver quoted it for its own engine; re-quoting here would double it.
    expect(selectFrom('"public"."users"', "postgres")).toContain('"public"."users"');
  });
});

describe("truncate", () => {
  it("uses DELETE on SQLite, which has no TRUNCATE", () => {
    expect(truncate('"users"', "sqlite")).toBe('DELETE FROM "users";');
  });

  it("uses TRUNCATE everywhere else", () => {
    expect(truncate("`users`", "mysql")).toBe("TRUNCATE TABLE `users`;");
    expect(truncate('"users"', "postgres")).toBe('TRUNCATE TABLE "users";');
  });
});

describe("drop", () => {
  it("names the object type", () => {
    expect(drop('"v"', "postgres", "view")).toBe('DROP VIEW "v";');
    expect(drop('"t"', "postgres", "table")).toBe('DROP TABLE "t";');
  });

  it("makes a ClickHouse drop synchronous", () => {
    // ClickHouse drops are asynchronous by default, so without SYNC the
    // statement returns before the table is actually gone.
    expect(drop("`t`", "clickhouse", "table")).toBe("DROP TABLE `t` SYNC;");
  });
});

describe("insertInto", () => {
  it("names every column and pairs it with a placeholder", () => {
    // Named placeholders rather than a row of question marks: past three
    // columns, positional markers are unreadable.
    expect(insertInto("`t`", ["id", "email"], "`")).toBe(
      "INSERT INTO `t` (`id`, `email`)\nVALUES (:id, :email);",
    );
  });

  it("escapes a quote inside a column name", () => {
    expect(insertInto('"t"', ['we"ird'], '"')).toContain('"we""ird"');
  });

  it("still produces a runnable shape for a table with no columns listed", () => {
    expect(insertInto('"t"', [], '"')).toBe('INSERT INTO "t" VALUES ();');
  });
});
