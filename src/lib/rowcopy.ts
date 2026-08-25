/**
 * What a right-click on a cell is pointing at.
 *
 * Two questions, both of which have a wrong answer that is easy to reach for:
 * which rows the menu should act on, and which table an `INSERT` built from
 * them should name.
 */

import type { Column } from "./types";

/**
 * The rows a menu opened on `clicked` should act on.
 *
 * A right-click inside a selection acts on the selection — that is what a
 * selection is for, and offering one row out of the twelve somebody just picked
 * would read as the menu having missed them. A right-click outside one acts on
 * the row under the pointer and leaves the selection alone: quietly redefining
 * what is selected because a menu was opened is how the wrong rows get copied,
 * or deleted.
 *
 * Returned in ascending order, which is the order they are in the result.
 */
export function rowsForMenu(clicked: number, selected: ReadonlySet<number>): number[] {
  if (!selected.has(clicked) || selected.size <= 1) return [clicked];
  return [...selected].sort((a, b) => a - b);
}

/**
 * The one table a result's rows came from, if they came from exactly one.
 *
 * Taken from the columns' sources rather than from the tab's title: a tab is
 * called "Query 1" as often as it is called "users", and `INSERT INTO "Query 1"`
 * is not a statement anybody can run. Columns that have no source at all —
 * expressions, counts, literals — are ignored rather than disqualifying, since
 * `SELECT *, now() FROM users` is still unambiguously about `users`.
 *
 * Null when two tables are involved, or none is: a join has no single table to
 * insert into, and saying so is better than naming whichever one came first.
 */
export function sourceTable(columns: Column[]): string | null {
  let found: string | null = null;
  for (const column of columns) {
    const table = column.source?.table;
    if (!table) continue;
    if (found === null) found = table;
    else if (found !== table) return null;
  }
  return found;
}
