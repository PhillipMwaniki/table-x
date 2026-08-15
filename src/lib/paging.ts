/**
 * Whether a statement is safe to page through.
 *
 * `OFFSET` without `ORDER BY` is the trap. No engine here promises a stable row
 * order for an unordered query — PostgreSQL will happily return the same rows in
 * a different sequence between two executions, and a parallel or index scan makes
 * it likely rather than theoretical. Page two of an unordered query can therefore
 * repeat rows from page one and skip others entirely, and nothing about the
 * result says so.
 *
 * So the grid says so instead. It does not refuse to page, because paging an
 * unordered query is still useful for a look around, and refusing would be worse
 * than warning.
 */

/**
 * Whether the statement ends with an `ORDER BY` that governs its rows.
 *
 * A heuristic, deliberately. Answering this exactly means parsing the statement,
 * and the cost of being wrong here is a warning that is shown when it need not
 * be — not a wrong result. The bias is toward warning: an `ORDER BY` inside a
 * subquery or a window function does not order the outer result, so those do not
 * count.
 */
export function hasOrderBy(sql: string): boolean {
  const stripped = stripLiteralsAndComments(sql);

  // Scanned from the end at nesting depth zero: the last top-level ORDER BY is
  // the one that orders what comes back. One inside parentheses belongs to a
  // subquery or an OVER clause and orders something else.
  let depth = 0;
  const upper = stripped.toUpperCase();

  for (let i = upper.length - 1; i >= 0; i--) {
    const ch = upper[i];
    if (ch === ")") depth++;
    else if (ch === "(") depth--;
    else if (depth === 0 && ch === "Y" && upper.startsWith("ORDER BY", i - 7)) {
      // Guard the left edge so "REORDER BY" does not match.
      const before = upper[i - 8];
      if (before === undefined || !/[A-Z0-9_$]/.test(before)) return true;
    }
  }
  return false;
}

/**
 * Blank out string literals and comments, keeping the length.
 *
 * Length is preserved so offsets into the result still line up with the
 * original, and blanking rather than deleting means `'order by'` inside a
 * literal cannot be mistaken for the clause.
 */
function stripLiteralsAndComments(sql: string): string {
  const out = sql.split("");
  let i = 0;

  while (i < sql.length) {
    const ch = sql[i];
    const next = sql[i + 1];

    if (ch === "'" || ch === '"' || ch === "`") {
      const quote = ch;
      out[i] = " ";
      i++;
      while (i < sql.length) {
        if (sql[i] === quote) {
          // A doubled quote is an escaped one and does not close the literal.
          if (sql[i + 1] === quote) {
            out[i] = " ";
            out[i + 1] = " ";
            i += 2;
            continue;
          }
          out[i] = " ";
          i++;
          break;
        }
        out[i] = " ";
        i++;
      }
      continue;
    }

    if (ch === "-" && next === "-") {
      while (i < sql.length && sql[i] !== "\n") {
        out[i] = " ";
        i++;
      }
      continue;
    }

    if (ch === "/" && next === "*") {
      while (i < sql.length && !(sql[i] === "*" && sql[i + 1] === "/")) {
        out[i] = " ";
        i++;
      }
      // The closing delimiter itself.
      if (i < sql.length) {
        out[i] = " ";
        out[i + 1] = " ";
        i += 2;
      }
      continue;
    }

    i++;
  }

  return out.join("");
}
