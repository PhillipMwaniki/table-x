/**
 * Choosing how a cell is edited, and the helpers each editor needs.
 *
 * A one-line text input is the right control for a name and the wrong one for
 * a JSON document, a boolean, or four kilobytes of binary. This decides which
 * control a value gets, and the pure parts of what those controls do.
 */

import type { Value } from "./types";

export type EditorKind =
  /** A single-line input: numbers, short text, dates. */
  | "inline"
  /** A resizable text area: anything long enough that one line hides it. */
  | "text"
  /** A text area that validates and can reformat. */
  | "json"
  /** True / False / NULL, with no way to type something that is none of them. */
  | "bool"
  /** Read-only hex and ASCII. */
  | "binary";

/** Values longer than this get room to breathe rather than a one-line input. */
export const LONG_VALUE = 120;

/** Which control a value should be edited with. */
export function editorFor(value: Value): EditorKind {
  switch (value.kind) {
    case "bool":
      return "bool";
    case "json":
      return "json";
    case "bytes":
      return "binary";
    case "text":
      // A JSON document stored in a text column is still a JSON document; the
      // column type is what the schema says, not what the value is.
      if (looksLikeJson(value.value)) return "json";
      return value.value.length > LONG_VALUE || value.value.includes("\n") ? "text" : "inline";
    default:
      return "inline";
  }
}

/** Whether a string is plausibly a JSON object or array — cheap, not a parse. */
export function looksLikeJson(text: string): boolean {
  const trimmed = text.trim();
  if (trimmed.length < 2) return false;
  const first = trimmed[0];
  const last = trimmed[trimmed.length - 1];
  return (first === "{" && last === "}") || (first === "[" && last === "]");
}

export interface JsonCheck {
  valid: boolean;
  /** Message with the position, when the text does not parse. */
  error?: string;
}

/**
 * Check JSON before it is written back.
 *
 * Saving unparseable JSON into a JSON column fails at the database with a
 * message about the column; failing here says which character is wrong, while
 * the text is still on screen to fix.
 */
export function checkJson(text: string): JsonCheck {
  if (text.trim() === "") return { valid: true };
  try {
    JSON.parse(text);
    return { valid: true };
  } catch (e) {
    return { valid: false, error: (e as Error).message };
  }
}

/** Re-indent JSON, leaving it untouched when it does not parse. */
export function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** The three states a nullable boolean can hold. */
export type BoolChoice = "true" | "false" | "null";

export function boolChoiceOf(value: Value): BoolChoice {
  if (value.kind === "null") return "null";
  if (value.kind === "bool") return value.value ? "true" : "false";
  // A boolean column can arrive as 0/1 from engines without a real bool type.
  return /^(1|t|true|y|yes)$/i.test(String((value as { value?: unknown }).value ?? ""))
    ? "true"
    : "false";
}

/**
 * A hex dump, 16 bytes to the line.
 *
 * Binary is shown rather than edited: a byte array typed into a text box is a
 * way to corrupt a file, and no engine here reports enough about the column to
 * validate one.
 */
export function hexDump(bytes: number[], perLine = 16): string {
  const lines: string[] = [];
  for (let offset = 0; offset < bytes.length; offset += perLine) {
    const slice = bytes.slice(offset, offset + perLine);
    const hex = slice
      .map((b) => b.toString(16).padStart(2, "0"))
      .join(" ")
      // Pad so the ASCII column lines up on a short final row.
      .padEnd(perLine * 3 - 1, " ");
    // Anything outside printable ASCII shows as a dot: a control character
    // would move the cursor and break the alignment the dump exists for.
    const ascii = slice.map((b) => (b >= 0x20 && b <= 0x7e ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${offset.toString(16).padStart(8, "0")}  ${hex}  ${ascii}`);
  }
  return lines.join("\n");
}

/** "1.4 KB" — for the header of a binary viewer. */
export function byteSize(length: number): string {
  if (length < 1024) return `${length} bytes`;
  if (length < 1024 * 1024) return `${(length / 1024).toFixed(1)} KB`;
  return `${(length / (1024 * 1024)).toFixed(1)} MB`;
}
