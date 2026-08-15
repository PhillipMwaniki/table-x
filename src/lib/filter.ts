/**
 * Column filters for the result grid.
 *
 * A filter box that only does substring matching is useless on the columns
 * people most want to filter — you cannot ask for "over 100" or "not null" with
 * it. This parses a small expression language instead, chosen so the common
 * cases need no syntax at all: typing `smith` still means "contains smith".
 *
 * Filtering happens over the rows already fetched, which is why the grid says
 * so when a result is truncated. Pushing predicates into the query is the next
 * step and needs per-dialect literal escaping to be safe.
 */

import { compareDecimalText, formatValue } from "./value";
import type { Value } from "./types";

export type Comparison = ">" | ">=" | "<" | "<=" | "=";

export type Predicate =
  /** Matches everything — an empty filter box. */
  | { kind: "any" }
  | { kind: "contains"; text: string; negated: boolean }
  | { kind: "equals"; text: string; negated: boolean }
  | { kind: "compare"; op: Comparison; text: string }
  | { kind: "null"; negated: boolean };

/**
 * Read a filter expression.
 *
 * - `smith` — contains, case-insensitive
 * - `!smith` — does not contain
 * - `=42` — exactly equal
 * - `>100`, `>=100`, `<0`, `<=0` — compared as numbers when both sides are
 *   numbers, as text otherwise
 * - `null` / `!null` — is, or is not, NULL
 *
 * Anything unparseable falls back to `contains`, because a filter that silently
 * matches nothing is worse than one that matches too much.
 */
export function parseFilter(input: string): Predicate {
  const text = input.trim();
  if (text === "") return { kind: "any" };

  const negated = text.startsWith("!");
  const body = negated ? text.slice(1).trim() : text;
  if (body === "") return { kind: "any" };

  if (body.toLowerCase() === "null") return { kind: "null", negated };

  // Two-character operators first, or `>=` would read as `>` followed by `=`.
  for (const op of [">=", "<=", ">", "<"] as const) {
    if (body.startsWith(op)) {
      const operand = body.slice(op.length).trim();
      if (operand !== "") return { kind: "compare", op, text: operand };
    }
  }

  if (body.startsWith("=")) {
    const operand = body.slice(1).trim();
    if (operand !== "") return { kind: "equals", text: operand, negated };
  }

  return { kind: "contains", text: body, negated };
}

/** Whether one cell satisfies a predicate. */
export function matchesFilter(value: Value, predicate: Predicate): boolean {
  switch (predicate.kind) {
    case "any":
      return true;

    case "null":
      return (value.kind === "null") !== predicate.negated;

    case "contains": {
      // NULL contains nothing, so a positive filter excludes it and a negative
      // one keeps it — the same way an absent value behaves in a spreadsheet.
      if (value.kind === "null") return predicate.negated;
      const hit = formatValue(value).toLowerCase().includes(predicate.text.toLowerCase());
      return hit !== predicate.negated;
    }

    case "equals": {
      if (value.kind === "null") return predicate.negated;
      const hit = formatValue(value).toLowerCase() === predicate.text.toLowerCase();
      return hit !== predicate.negated;
    }

    case "compare": {
      // NULL is never greater or less than anything, which is what SQL says
      // too: a comparison against NULL is unknown, and unknown is not a match.
      if (value.kind === "null") return false;

      const left = formatValue(value);
      const right = predicate.text;
      // Exact first: a 19-digit value and its neighbour are the same double,
      // and `>` must not drop a row because of it.
      const cmp =
        compareDecimalText(left, right) ??
        left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });

      switch (predicate.op) {
        case ">":
          return cmp > 0;
        case ">=":
          return cmp >= 0;
        case "<":
          return cmp < 0;
        case "<=":
          return cmp <= 0;
        case "=":
          return cmp === 0;
      }
    }
  }
}

/** A one-line reminder of the syntax, for the filter row's tooltip. */
export const FILTER_HINT =
  "Contains by default. Use =exact, >100, <=0, null, or ! to negate (!null, !draft).";
