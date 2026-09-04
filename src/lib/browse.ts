/**
 * The clauses a table tab adds to its statement.
 *
 * A table tab opens with `SELECT * FROM t` and pages through it. The filter
 * above the rows narrows the page that came back, which is honest and capped
 * at the fetch limit; these two reach the whole table, because they go to the
 * server as part of the statement. They are typed in the engine's own dialect
 * and sent as written — the app does not know enough SQL to rewrite them and
 * would only get it wrong in one engine's spelling.
 */

export interface Clauses {
  where: string;
  orderBy: string;
}

/**
 * What a person typed into a clause box, made ready to append.
 *
 * Trimmed; a trailing semicolon dropped, since the box is not a statement;
 * and the keyword itself dropped if it was typed — `WHERE id = 1` in a box
 * labelled WHERE is a natural thing to type and would otherwise send
 * `WHERE WHERE id = 1`.
 */
export function clause(text: string, keyword: "WHERE" | "ORDER BY"): string {
  let out = text
    .trim()
    .replace(/;+\s*$/, "")
    .trim();
  const lead = new RegExp(`^${keyword.replace(" ", "\\s+")}\\s+`, "i");
  out = out.replace(lead, "").trim();
  return out;
}

/** The base statement with the clauses appended, or the base alone. */
export function browseStatement(base: string, clauses: Partial<Clauses>): string {
  const where = clause(clauses.where ?? "", "WHERE");
  const orderBy = clause(clauses.orderBy ?? "", "ORDER BY");
  let sql = base.trim().replace(/;+\s*$/, "");
  if (where) sql += ` WHERE ${where}`;
  if (orderBy) sql += ` ORDER BY ${orderBy}`;
  return sql;
}

/**
 * The statement a tab will actually run.
 *
 * A table tab's `sql` is the bare `SELECT`; the clauses live beside it so the
 * tab can show them as two boxes rather than as text to edit. Every other kind
 * of tab runs its `sql` as it is.
 */
export function tableStatement(tab: {
  kind: string;
  sql: string;
  where?: string | undefined;
  orderBy?: string | undefined;
}): string {
  if (tab.kind !== "table") return tab.sql;
  return browseStatement(tab.sql, { where: tab.where ?? "", orderBy: tab.orderBy ?? "" });
}
