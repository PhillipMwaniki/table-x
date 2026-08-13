/**
 * Tests for the cell value layer.
 *
 * This is where the backend's precision guarantees can quietly be undone: one
 * `Number(...)` in the display or edit path throws away digits the Rust side
 * went to real trouble to preserve.
 */

import { describe, expect, it } from "vitest";
import { cellClass, editText, formatValue, isNumeric, kindOf, parseEdit, previewValue } from "./value";
import type { Value } from "./types";

describe("formatValue", () => {
  it("renders NULL distinctly from an empty string", () => {
    expect(formatValue({ kind: "null" })).toBe("NULL");
    expect(formatValue({ kind: "text", value: "" })).toBe("");
    // And they must be visually distinguishable, not just textually.
    expect(cellClass({ kind: "null" })).toContain("italic");
    expect(cellClass({ kind: "text", value: "" })).not.toContain("italic");
  });

  it("passes exact numerics through untouched", () => {
    // 40 significant digits: any trip through a JS number loses most of them.
    const exact = "12345678901234567890.12345678901234567890";
    expect(formatValue({ kind: "numeric", value: exact })).toBe(exact);
  });

  it("summarizes binary instead of stringifying it", () => {
    const value: Value = { kind: "bytes", value: new Array(4_000_000).fill(0) };
    // Expanding a 4 MB BLOB into a grid cell would freeze rendering.
    expect(formatValue(value)).toBe("[4000000 bytes]");
  });

  it("renders timestamps with a space rather than an ISO T", () => {
    expect(formatValue({ kind: "date_time", value: "2026-08-13T11:30:00" })).toBe(
      "2026-08-13 11:30:00",
    );
  });

  it("shows unsupported types as their raw value", () => {
    expect(
      formatValue({ kind: "unsupported", value: { type_name: "point", raw: "(1,2)" } }),
    ).toBe("(1,2)");
  });

  it("renders arrays elementwise including nulls", () => {
    const value: Value = {
      kind: "array",
      value: [{ kind: "int", value: 1 }, { kind: "null" }, { kind: "int", value: 3 }],
    };
    expect(formatValue(value)).toBe("{1, NULL, 3}");
  });
});

describe("previewValue", () => {
  it("clips very long text and flags it", () => {
    const long = "x".repeat(5000);
    const { text, clipped } = previewValue({ kind: "text", value: long });
    expect(clipped).toBe(true);
    expect(text.length).toBeLessThan(long.length);
  });

  it("leaves short text alone", () => {
    const { text, clipped } = previewValue({ kind: "text", value: "hello" });
    expect(clipped).toBe(false);
    expect(text).toBe("hello");
  });
});

describe("parseEdit", () => {
  it("keeps typed digits exact rather than parsing to a number", () => {
    const original: Value = { kind: "numeric", value: "1" };
    const exact = "99999999999999999999.99999999999999999999";
    const result = parseEdit(exact, original);
    // Must stay a string; `{ kind: "float", value: 1e20 }` would silently round.
    expect(result).toEqual({ kind: "numeric", value: exact });
  });

  it("keeps integers exact past Number.MAX_SAFE_INTEGER", () => {
    const original: Value = { kind: "int", value: 1 };
    const big = "9007199254740993"; // MAX_SAFE_INTEGER + 2
    expect(parseEdit(big, original)).toEqual({ kind: "numeric", value: big });
  });

  it("distinguishes clearing a text cell from clearing any other cell", () => {
    // An emptied text cell is plausibly an empty string; an emptied number is
    // not, so it becomes NULL. Guessing wrong writes the wrong value.
    expect(parseEdit("", { kind: "text", value: "a" })).toEqual({ kind: "text", value: "" });
    expect(parseEdit("", { kind: "int", value: 5 })).toEqual({ kind: "null" });
    expect(parseEdit("", { kind: "date", value: "2026-01-01" })).toEqual({ kind: "null" });
  });

  it("accepts the usual spellings of booleans", () => {
    const original: Value = { kind: "bool", value: false };
    for (const t of ["true", "TRUE", "t", "1", "yes"]) {
      expect(parseEdit(t, original)).toEqual({ kind: "bool", value: true });
    }
    for (const f of ["false", "F", "0", "no"]) {
      expect(parseEdit(f, original)).toEqual({ kind: "bool", value: false });
    }
  });

  it("sends invalid JSON on as text so the database reports the real error", () => {
    const original: Value = { kind: "json", value: {} };
    // Silently rejecting in the UI would hide which part the server objects to.
    expect(parseEdit("{not json", original)).toEqual({ kind: "text", value: "{not json" });
    expect(parseEdit('{"a":1}', original)).toEqual({ kind: "json", value: { a: 1 } });
  });
});

describe("editText", () => {
  it("starts a NULL cell empty rather than with the word NULL", () => {
    // Otherwise the user has to clear "NULL" before typing, and leaving it
    // would write the literal string.
    expect(editText({ kind: "null" })).toBe("");
    expect(editText({ kind: "text", value: "hi" })).toBe("hi");
  });
});

describe("kind classification", () => {
  it("right-aligns numbers but not numeric-looking text", () => {
    expect(isNumeric({ kind: "int", value: 1 })).toBe(true);
    expect(isNumeric({ kind: "numeric", value: "1.5" })).toBe(true);
    expect(isNumeric({ kind: "text", value: "1" })).toBe(false);
  });

  it("maps every variant to a kind", () => {
    const samples: Value[] = [
      { kind: "null" },
      { kind: "bool", value: true },
      { kind: "int", value: 1 },
      { kind: "u_int", value: 1 },
      { kind: "float", value: 1 },
      { kind: "numeric", value: "1" },
      { kind: "text", value: "a" },
      { kind: "bytes", value: [] },
      { kind: "uuid", value: "x" },
      { kind: "date", value: "2026-01-01" },
      { kind: "time", value: "00:00" },
      { kind: "date_time", value: "2026-01-01T00:00" },
      { kind: "timestamp_tz", value: "2026-01-01T00:00Z" },
      { kind: "interval", value: { months: 0, days: 0, micros: 0 } },
      { kind: "json", value: null },
      { kind: "array", value: [] },
      { kind: "unsupported", value: { type_name: "t", raw: "r" } },
    ];
    // A missing case would return undefined and break alignment silently.
    for (const s of samples) {
      expect(kindOf(s), `no kind for ${s.kind}`).toBeTruthy();
    }
  });
});
