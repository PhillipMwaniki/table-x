import { describe, expect, it } from "vitest";
import { changesCatalog } from "./statements";

describe("changesCatalog", () => {
  it("catches the statements that add or remove objects", () => {
    for (const sql of [
      "CREATE DATABASE app",
      "drop database app;",
      "CREATE SCHEMA reporting",
      "CREATE TABLE orders (id int)",
      "DROP TABLE orders",
      "ALTER TABLE orders ADD COLUMN note text",
      "CREATE UNIQUE INDEX orders_note_idx ON orders (note)",
      "CREATE OR REPLACE VIEW recent AS SELECT 1",
      "CREATE TEMPORARY TABLE scratch (a int)",
      "DROP TABLE IF EXISTS orders",
      "RENAME TABLE a TO b",
    ]) {
      expect(changesCatalog(sql), sql).toBe(true);
    }
  });

  it("leaves ordinary work alone", () => {
    // A refresh after every UPDATE would refetch the catalog constantly for a
    // tree that cannot have changed.
    for (const sql of [
      "SELECT * FROM users",
      "UPDATE users SET email = 'a@b.c' WHERE id = 1",
      "INSERT INTO users VALUES (1)",
      "DELETE FROM users WHERE id = 1",
      "BEGIN",
      "EXPLAIN SELECT * FROM orders",
    ]) {
      expect(changesCatalog(sql), sql).toBe(false);
    }
  });

  it("does not fire on a statement that is only commented out", () => {
    // Watching the tree refresh because of a line you deliberately disabled
    // would be a small mystery every time.
    expect(changesCatalog("-- DROP TABLE orders\nSELECT 1")).toBe(false);
    expect(changesCatalog("/* CREATE TABLE t (a int) */ SELECT 1")).toBe(false);
  });

  it("does not join a verb in one statement to a noun in another", () => {
    // The bounded gap is what stops this. Without it, any script containing a
    // DROP anywhere and the word "table" later would refresh on every run.
    const sql = "DELETE FROM audit WHERE dropped;\n" + "SELECT 1;\n".repeat(6) + "SELECT * FROM t";
    expect(changesCatalog(sql)).toBe(false);
  });
});
