/**
 * Placeholders in a statement, and how to fill them.
 *
 * A query pasted from application code arrives with the holes its driver
 * would fill: `?`, `:id`, `${id}`, `#{id}`, `@id`, `$1`. Running it as-is is a
 * syntax error on every engine; editing the holes out by hand is tedious and
 * has to be undone before the query goes back. So the holes are found here,
 * asked for, and filled in as literals before the statement runs.
 *
 * Filled in as text, on purpose. A bound parameter would be the engine's way,
 * but the statement that runs is then not the statement in the history, and
 * a value that was quoted wrong is invisible. A literal is what actually ran,
 * and it reads back.
 *
 * Which spellings count depends on the engine, because two of them are also
 * something else somewhere: `@name` is a session variable on MySQL and a
 * declared variable on SQL Server, and `$1` is only a placeholder on
 * PostgreSQL. Everything is matched outside strings, comments and quoted
 * names, so `'price: ?'` asks for nothing.
 */

export interface Parameter {
  /** How it was written, for the label: `?`, `:id`, `${id}`. */
  token: string;
  /**
   * What it is asked for under. Named placeholders share a value when they
   * share a name; each `?` is its own, numbered in order.
   */
  name: string;
}

export type ValueType = "text" | "number" | "boolean" | "null" | "raw";

export interface ParameterValue {
  type: ValueType;
  text: string;
}

/** One placeholder found in the statement, with where it sits. */
interface Found extends Parameter {
  start: number;
  end: number;
}

/** Whether `@name` is a placeholder on this engine or something of its own. */
function atIsPlaceholder(driver: string): boolean {
  return driver !== "mysql" && driver !== "mssql";
}

function isWordStart(c: string): boolean {
  return /[A-Za-z_]/.test(c);
}

function isWordChar(c: string): boolean {
  return /[A-Za-z0-9_]/.test(c);
}

/**
 * Walk the statement and report every placeholder outside a string, a
 * comment or a quoted name.
 */
function scan(sql: string, driver: string): Found[] {
  const out: Found[] = [];
  let positional = 0;
  let i = 0;
  const n = sql.length;
  const at = (j: number) => sql[j] ?? "";

  while (i < n) {
    const c = at(i);

    // Strings and quoted names, skipped whole. A doubled quote is an escape;
    // so is a backslash before a quote in a single-quoted string.
    if (c === "'" || c === '"' || c === "`") {
      i++;
      while (i < n) {
        if (at(i) === "\\" && c === "'" && i + 1 < n) {
          i += 2;
          continue;
        }
        if (at(i) === c) {
          if (at(i + 1) === c) {
            i += 2;
            continue;
          }
          i++;
          break;
        }
        i++;
      }
      continue;
    }
    if (c === "[") {
      const close = sql.indexOf("]", i + 1);
      i = close === -1 ? n : close + 1;
      continue;
    }
    if (c === "-" && at(i + 1) === "-") {
      const eol = sql.indexOf("\n", i);
      i = eol === -1 ? n : eol + 1;
      continue;
    }
    if (c === "/" && at(i + 1) === "*") {
      const close = sql.indexOf("*/", i + 2);
      i = close === -1 ? n : close + 2;
      continue;
    }

    if (c === "?") {
      // PostgreSQL's JSON operators `?`, `?|` and `?&` share the character.
      // Only the bare one, with an operand on each side, is a placeholder —
      // and even then a question mark between two JSON values is ambiguous,
      // so the two spelled forms are left alone and the bare one is asked for.
      if (driver === "postgres" && (at(i + 1) === "|" || at(i + 1) === "&")) {
        i++;
        continue;
      }
      positional++;
      out.push({ token: "?", name: `?${positional}`, start: i, end: i + 1 });
      i++;
      continue;
    }

    if (c === "$") {
      // `$1` on PostgreSQL; `${name}` everywhere.
      if (at(i + 1) === "{") {
        const close = sql.indexOf("}", i + 2);
        if (close !== -1) {
          const name = sql.slice(i + 2, close).trim();
          if (name) out.push({ token: `\${${name}}`, name, start: i, end: close + 1 });
          i = close + 1;
          continue;
        }
      }
      if (driver === "postgres" && /[0-9]/.test(at(i + 1)) && !isWordChar(at(i - 1))) {
        let j = i + 1;
        while (j < n && /[0-9]/.test(at(j))) j++;
        const token = sql.slice(i, j);
        out.push({ token, name: token, start: i, end: j });
        i = j;
        continue;
      }
      // A dollar-quoted body on PostgreSQL: `$$ … $$` or `$tag$ … $tag$`.
      if (driver === "postgres") {
        const tag = /^\$([A-Za-z_][A-Za-z0-9_]*)?\$/.exec(sql.slice(i));
        if (tag) {
          const close = sql.indexOf(tag[0], i + tag[0].length);
          i = close === -1 ? n : close + tag[0].length;
          continue;
        }
      }
      i++;
      continue;
    }

    if (c === "#" && at(i + 1) === "{") {
      const close = sql.indexOf("}", i + 2);
      if (close !== -1) {
        const name = sql.slice(i + 2, close).trim();
        if (name) out.push({ token: `#{${name}}`, name, start: i, end: close + 1 });
        i = close + 1;
        continue;
      }
    }

    if (c === ":" || (c === "@" && atIsPlaceholder(driver))) {
      // `::` is a cast, `:=` an assignment, and `t@dblink` is Oracle's own;
      // a placeholder is the sigil, then a name, with nothing stuck to it.
      const before = at(i - 1);
      const next = at(i + 1);
      const stuck = before === c || next === c || next === "=" || isWordChar(before);
      if (!stuck && isWordStart(next)) {
        let j = i + 1;
        while (j < n && isWordChar(at(j))) j++;
        const name = sql.slice(i + 1, j);
        out.push({ token: sql.slice(i, j), name, start: i, end: j });
        i = j;
        continue;
      }
      i++;
      continue;
    }

    i++;
  }
  return out;
}

/** The placeholders to ask for, each name once, in order of first appearance. */
export function findParameters(sql: string, driver: string): Parameter[] {
  const seen = new Set<string>();
  const out: Parameter[] = [];
  for (const found of scan(sql, driver)) {
    if (seen.has(found.name)) continue;
    seen.add(found.name);
    out.push({ token: found.token, name: found.name });
  }
  return out;
}

/** What goes in the statement's text for a value, or why it cannot. */
export function literal(
  value: ParameterValue,
  driver: string,
): { ok: true; sql: string } | { ok: false; error: string } {
  switch (value.type) {
    case "null":
      return { ok: true, sql: "NULL" };
    case "text":
      return { ok: true, sql: `'${value.text.replaceAll("'", "''")}'` };
    case "number": {
      const t = value.text.trim();
      // A number is written as typed, so an exact decimal stays exact; the
      // check is only that it is one.
      if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t)) {
        return { ok: false, error: "Not a number" };
      }
      return { ok: true, sql: t };
    }
    case "boolean": {
      const t = value.text.trim().toLowerCase();
      if (t !== "true" && t !== "false") return { ok: false, error: "true or false" };
      // Two engines have no boolean literal in SQL; one and zero are what
      // their BIT and NUMBER(1) columns hold.
      if (driver === "mssql" || driver === "oracle")
        return { ok: true, sql: t === "true" ? "1" : "0" };
      return { ok: true, sql: t === "true" ? "TRUE" : "FALSE" };
    }
    case "raw":
      if (!value.text.trim()) return { ok: false, error: "Empty" };
      return { ok: true, sql: value.text.trim() };
  }
}

/**
 * The statement with every placeholder replaced by its value's literal.
 *
 * Every placeholder must have a value; a hole left open would run as the
 * syntax error the prompt exists to prevent.
 */
export function substitute(
  sql: string,
  values: Record<string, ParameterValue>,
  driver: string,
): { ok: true; sql: string } | { ok: false; error: string } {
  let out = "";
  let from = 0;
  for (const found of scan(sql, driver)) {
    const value = values[found.name];
    if (!value) return { ok: false, error: `No value for ${found.token}` };
    const lit = literal(value, driver);
    if (!lit.ok) return { ok: false, error: `${found.token}: ${lit.error}` };
    out += sql.slice(from, found.start) + lit.sql;
    from = found.end;
  }
  out += sql.slice(from);
  return { ok: true, sql: out };
}
