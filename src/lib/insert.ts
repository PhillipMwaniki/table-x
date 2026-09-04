/**
 * What a filled-in insert form actually sends.
 *
 * Three states per field, and the whole form turns on keeping them apart: a
 * value, an explicit NULL, and *nothing at all*. The third is what lets the
 * server apply a default or generate a key, and it is the one that is easy to
 * lose — an empty box and an omitted column look identical on screen.
 */

import type { ColumnDef, Value } from "./types";

/**
 * Fill a form in from an existing row, for duplicating it.
 *
 * Generated keys are left out. A duplicate that carries the original's
 * auto-increment value is a duplicate key error every time, so copying it only
 * gives the user something to clear before the form will submit — and clearing
 * it is exactly the step that used to send an empty string to an integer
 * column.
 *
 * Columns the row has no value for are skipped rather than blanked: a query
 * that selected three columns out of twenty says nothing about the other
 * seventeen, and the server's defaults are a better answer than empty.
 */
export function seedFields(
  columns: ColumnDef[],
  initial: Record<string, string | null> | undefined,
): { entered: Record<string, string>; nulled: string[] } {
  const entered: Record<string, string> = {};
  const nulled: string[] = [];
  if (!initial) return { entered, nulled };

  for (const column of columns) {
    if (column.auto_increment) continue;
    if (!(column.name in initial)) continue;
    const value = initial[column.name];
    // NULL is carried across as an explicit NULL rather than as a blank. A
    // duplicate of a row whose `deleted_at` is NULL should say NULL, not let a
    // default decide.
    if (value === null) nulled.push(column.name);
    else if (value !== undefined) entered[column.name] = value;
  }
  return { entered, nulled };
}

/**
 * The columns and values to insert.
 *
 * A blank field is omitted, which is what the form promises and what makes
 * defaults and generated keys work. Sending `""` instead is not a subtle
 * difference: on an integer column the server rejects the whole statement, and
 * on a text column it quietly writes an empty string where a default was meant
 * to go.
 *
 * Blank means blank to the eye — whitespace included — which is the same
 * definition the form uses when it says which required fields are still
 * missing. Two meanings of "blank" in one dialog is one too many.
 */
export function insertValues(
  columns: ColumnDef[],
  entered: Record<string, string>,
  nulled: ReadonlySet<string>,
): [string, Value][] {
  const values: [string, Value][] = [];
  for (const column of columns) {
    if (nulled.has(column.name)) {
      values.push([column.name, { kind: "null" }]);
      continue;
    }
    const text = entered[column.name];
    if (text === undefined || text.trim() === "") continue;
    // Sent as text and cast by the server, the same way an edited cell is — so
    // an exact decimal reaches the column as its digits.
    values.push([column.name, { kind: "text", value: text }]);
  }
  return values;
}

/**
 * Which part of a clock a column stores, from its type name, or `null` for a
 * column that does not store one.
 *
 * Read off the name rather than a driver-supplied kind because the insert form
 * only has the catalog's type string to go on, and every supported database
 * spells the three families recognisably: `datetime`, `timestamp`,
 * `timestamptz`, `datetime2`, `smalldatetime`, `timestamp without time zone`;
 * `date`; `time`, `timetz`, `time with time zone`. Checked in that order
 * because `timestamp` also starts with `time`.
 */
export type TemporalKind = "datetime" | "date" | "time";

export function temporalKind(typeName: string): TemporalKind | null {
  const name = typeName.trim().toLowerCase();
  if (name.includes("timestamp") || name.includes("datetime")) return "datetime";
  if (name.startsWith("date")) return "date";
  if (name.startsWith("time")) return "time";
  return null;
}

/**
 * The current moment, written the way the column expects it.
 *
 * Local wall-clock time rather than UTC, because that is what a person reading
 * the field expects to see, and sent as text so the server casts it exactly as
 * it would a typed value. Seconds but no fraction: a fraction on a `datetime`
 * that has no room for it is a rounding the server does silently, and a
 * timestamp somebody chose to stamp "now" is not one they need to the
 * millisecond.
 */
export function nowText(kind: TemporalKind, now: Date = new Date()): string {
  const two = (n: number) => String(n).padStart(2, "0");
  const date = `${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}`;
  const time = `${two(now.getHours())}:${two(now.getMinutes())}:${two(now.getSeconds())}`;
  switch (kind) {
    case "date":
      return date;
    case "time":
      return time;
    case "datetime":
      return `${date} ${time}`;
  }
}
