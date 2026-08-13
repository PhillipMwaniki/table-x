/**
 * Rendering and editing helpers for cell values.
 *
 * The grid never calls `String(value)` directly. Several kinds need deliberate
 * treatment — NULL must be visually distinct from an empty string, binary must
 * never be stringified into a cell, and exact numerics must never round-trip
 * through a JavaScript number.
 */

import type { Value, ValueKind } from "./types";

/** Beyond this, a cell shows a truncated preview; the full text is in the inspector. */
const MAX_CELL_CHARS = 500;

export function kindOf(value: Value): ValueKind {
  switch (value.kind) {
    case "null":
      return "null";
    case "bool":
      return "bool";
    case "int":
    case "u_int":
      return "integer";
    case "float":
    case "numeric":
      return "number";
    case "text":
      return "text";
    case "bytes":
      return "binary";
    case "uuid":
      return "uuid";
    case "date":
    case "time":
    case "date_time":
    case "timestamp_tz":
      return "temporal";
    case "interval":
      return "interval";
    case "json":
      return "json";
    case "array":
      return "array";
    case "unsupported":
      return "unknown";
  }
}

/** Numerics are right-aligned so digits line up column-wise. */
export function isNumeric(value: Value): boolean {
  const k = kindOf(value);
  return k === "integer" || k === "number";
}

/**
 * Full display text for a value.
 *
 * Exact numerics are already strings and are passed through untouched — parsing
 * them into a JS number here would undo the precision the backend went out of
 * its way to preserve.
 */
export function formatValue(value: Value): string {
  switch (value.kind) {
    case "null":
      return "NULL";
    case "bool":
      return value.value ? "true" : "false";
    case "int":
    case "u_int":
    case "float":
      return String(value.value);
    case "numeric":
      return value.value;
    case "text":
    case "uuid":
    case "date":
    case "time":
      return value.value;
    case "date_time":
      // Rust emits ISO with a `T`; a space reads better in a dense grid.
      return value.value.replace("T", " ");
    case "timestamp_tz":
      return value.value.replace("T", " ");
    case "bytes":
      // Never expand binary into the cell: a multi-megabyte BLOB would freeze
      // rendering and tell the user nothing useful.
      return `[${value.value.length} bytes]`;
    case "interval": {
      const { months, days, micros } = value.value;
      const parts: string[] = [];
      if (months) parts.push(`${months} mon`);
      if (days) parts.push(`${days} d`);
      if (micros) parts.push(`${micros / 1_000_000} s`);
      return parts.length ? parts.join(" ") : "0";
    }
    case "json":
      return JSON.stringify(value.value);
    case "array":
      return `{${value.value.map(formatValue).join(", ")}}`;
    case "unsupported":
      return value.value.raw;
  }
}

/** Display text clipped to a length the grid can render cheaply. */
export function previewValue(value: Value): { text: string; clipped: boolean } {
  const full = formatValue(value);
  if (full.length <= MAX_CELL_CHARS) return { text: full, clipped: false };
  return { text: `${full.slice(0, MAX_CELL_CHARS)}…`, clipped: true };
}

/** Whether the cell can be edited by typing. Binary needs a dedicated editor. */
export function isInlineEditable(value: Value): boolean {
  return kindOf(value) !== "binary";
}

/** Text to seed an editing input with. NULL starts empty rather than as "NULL". */
export function editText(value: Value): string {
  return value.kind === "null" ? "" : formatValue(value);
}

/**
 * Turn edited text back into a `Value`, preserving the original column's kind so
 * a numeric column stays numeric.
 *
 * Numbers become `numeric` (a string) rather than `float`, so typed digits reach
 * the database exactly as entered. The server does the final conversion.
 */
export function parseEdit(text: string, original: Value): Value {
  if (text === "") {
    // An emptied cell means NULL for everything except text, where the user may
    // genuinely want an empty string. That distinction matters and cannot be
    // guessed from the text alone.
    return original.kind === "text" ? { kind: "text", value: "" } : { kind: "null" };
  }

  switch (original.kind) {
    case "bool": {
      const t = text.trim().toLowerCase();
      if (["true", "t", "1", "yes"].includes(t)) return { kind: "bool", value: true };
      if (["false", "f", "0", "no"].includes(t)) return { kind: "bool", value: false };
      return { kind: "text", value: text };
    }
    case "int":
    case "u_int":
    case "float":
    case "numeric":
      // Kept as text: the backend sends it to the server as digits, so a value
      // too large for a JS number still survives.
      return { kind: "numeric", value: text.trim() };
    case "json":
      try {
        return { kind: "json", value: JSON.parse(text) };
      } catch {
        // Invalid JSON is sent as text so the database reports the real error
        // rather than the UI silently rejecting it.
        return { kind: "text", value: text };
      }
    default:
      return { kind: "text", value: text };
  }
}

/** Tailwind classes tinting a cell by kind. NULL is muted and italic. */
export function cellClass(value: Value): string {
  switch (kindOf(value)) {
    case "null":
      return "text-null italic";
    case "integer":
    case "number":
      return "text-text tabular-nums";
    case "bool":
      return "text-ok";
    case "binary":
      return "text-text-muted italic";
    case "unknown":
      return "text-warn";
    case "json":
    case "array":
      return "text-text-muted";
    default:
      return "text-text";
  }
}
