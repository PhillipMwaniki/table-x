import { describe, expect, it } from "vitest";
import { matchesFilter, parseFilter } from "./filter";
import type { Value } from "./types";

const text = (v: string): Value => ({ kind: "text", value: v });
const int = (v: number): Value => ({ kind: "int", value: v });
const numeric = (v: string): Value => ({ kind: "numeric", value: v });
const nul: Value = { kind: "null" };

/** Read a filter and apply it, which is how the grid uses both together. */
function keeps(filter: string, value: Value): boolean {
  return matchesFilter(value, parseFilter(filter));
}

describe("parseFilter", () => {
  it("treats an empty box as no filter at all", () => {
    expect(parseFilter("   ")).toEqual({ kind: "any" });
    // A lone `!` is someone mid-keystroke, not a request for nothing.
    expect(parseFilter("!")).toEqual({ kind: "any" });
  });

  it("reads two-character operators before one-character ones", () => {
    // Naive parsing reads `>=` as `>` followed by an operand of `=100`.
    expect(parseFilter(">=100")).toEqual({ kind: "compare", op: ">=", text: "100" });
    expect(parseFilter("<=0")).toEqual({ kind: "compare", op: "<=", text: "0" });
  });

  it("falls back to contains for anything it cannot parse", () => {
    // A filter that silently matches nothing is worse than one that matches
    // too much: the user can see and correct the second.
    expect(parseFilter(">")).toEqual({ kind: "contains", text: ">", negated: false });
  });
});

describe("matchesFilter", () => {
  it("matches substrings without regard to case", () => {
    expect(keeps("smith", text("Jo Smith"))).toBe(true);
    expect(keeps("SMITH", text("jo smith"))).toBe(true);
    expect(keeps("jones", text("Jo Smith"))).toBe(false);
  });

  it("negates with a leading bang", () => {
    expect(keeps("!draft", text("published"))).toBe(true);
    expect(keeps("!draft", text("draft"))).toBe(false);
  });

  it("compares numbers as numbers", () => {
    // The bug this prevents: "9" > "100" is true lexically and false in every
    // sense the user means.
    expect(keeps(">100", int(9))).toBe(false);
    expect(keeps(">100", int(9000))).toBe(true);
    expect(keeps("<=100", int(100))).toBe(true);
  });

  it("compares exact numerics without going through a float", () => {
    // These arrive as strings precisely because they exceed a double.
    expect(keeps(">123456789012345678", numeric("123456789012345679"))).toBe(true);
  });

  it("compares text as text", () => {
    expect(keeps(">m", text("zebra"))).toBe(true);
    expect(keeps(">m", text("apple"))).toBe(false);
  });

  it("finds NULLs and excludes them", () => {
    expect(keeps("null", nul)).toBe(true);
    expect(keeps("null", text("something"))).toBe(false);
    expect(keeps("!null", text("something"))).toBe(true);
    expect(keeps("!null", nul)).toBe(false);
  });

  it("never matches a NULL against a comparison", () => {
    // SQL says a comparison against NULL is unknown, and unknown is not a
    // match; a grid that answered differently would disagree with the database
    // the user is looking at.
    expect(keeps(">0", nul)).toBe(false);
    expect(keeps("<0", nul)).toBe(false);
  });

  it("excludes NULL from a positive text filter and keeps it from a negative one", () => {
    expect(keeps("draft", nul)).toBe(false);
    expect(keeps("!draft", nul)).toBe(true);
  });

  it("matches exactly when asked to", () => {
    expect(keeps("=42", int(42))).toBe(true);
    expect(keeps("=4", int(42))).toBe(false);
    // Where the plain form would have matched.
    expect(keeps("4", int(42))).toBe(true);
  });

  it("keeps every row when the box is empty", () => {
    expect(keeps("", text("anything"))).toBe(true);
    expect(keeps("", nul)).toBe(true);
  });
});
