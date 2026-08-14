/**
 * Statement templates for the object menu.
 *
 * These are dialect-specific by nature — row limits and table emptying are two
 * of the places SQL engines disagree most — so they take the driver id, the
 * same way the editor picks its parser. Everything here is a template the user
 * is shown before it runs; nothing in this file executes anything.
 */

/** Rows a menu-generated SELECT asks for. Enough to see the shape of a table. */
export const PREVIEW_LIMIT = 100;

/**
 * `SELECT * FROM x` with a row limit in the engine's own syntax.
 *
 * SQL Server had no `LIMIT` until 2012 and still prefers `TOP`; `OFFSET …
 * FETCH` exists but requires an `ORDER BY`, which there is no sensible default
 * for.
 */
export function selectFrom(qualified: string, driver: string, limit = PREVIEW_LIMIT): string {
  if (driver === "mssql") return `SELECT TOP ${limit} * FROM ${qualified};`;
  return `SELECT * FROM ${qualified} LIMIT ${limit};`;
}

/**
 * The statement that empties a table.
 *
 * SQLite has no `TRUNCATE` at all — the documented equivalent is an unqualified
 * `DELETE`, which its optimiser turns into the same bulk operation.
 */
export function truncate(qualified: string, driver: string): string {
  if (driver === "sqlite") return `DELETE FROM ${qualified};`;
  return `TRUNCATE TABLE ${qualified};`;
}

/** The statement that removes an object. */
export function drop(qualified: string, driver: string, kind: "table" | "view"): string {
  const what = kind === "view" ? "VIEW" : "TABLE";
  // ClickHouse drops are asynchronous by default; SYNC makes the statement mean
  // what it appears to mean — gone when it returns.
  if (driver === "clickhouse") return `DROP ${what} ${qualified} SYNC;`;
  return `DROP ${what} ${qualified};`;
}

/** `INSERT INTO x (a, b) VALUES (…)`, ready to fill in. */
export function insertInto(qualified: string, columns: string[], quote: string): string {
  if (columns.length === 0) return `INSERT INTO ${qualified} VALUES ();`;
  const q = (name: string) => `${quote}${name.replaceAll(quote, quote + quote)}${quote}`;
  const names = columns.map(q).join(", ");
  // Placeholders name the column they belong to: a row of bare question marks
  // is unreadable the moment a table has more than three of them.
  const values = columns.map((c) => `:${c}`).join(", ");
  return `INSERT INTO ${qualified} (${names})\nVALUES (${values});`;
}
