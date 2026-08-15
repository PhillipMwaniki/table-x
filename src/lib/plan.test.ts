import { describe, expect, it } from "vitest";
import { BAD_ESTIMATE, estimateError, formatRows, maxSelfCost, selfCost } from "./plan";
import type { PlanNode } from "./types";

function node(overrides: Partial<PlanNode> = {}): PlanNode {
  return {
    label: "Seq Scan",
    detail: null,
    rows: null,
    actual_rows: null,
    cost: null,
    actual_ms: null,
    children: [],
    ...overrides,
  };
}

describe("selfCost", () => {
  it("is the total minus what is below it", () => {
    // The root of a cumulative plan always has the highest total and is never
    // the answer; the step that added the most is.
    const root = node({
      label: "Hash Join",
      cost: 120.5,
      children: [node({ cost: 30 }), node({ cost: 20 })],
    });
    expect(selfCost(root)).toBe(70.5);
    expect(selfCost(root.children[0]!)).toBe(30);
    expect(maxSelfCost(root)).toBe(70.5);
  });

  it("never goes negative when a child reports more than its parent", () => {
    // Estimated and measured costs can disagree; a negative bar would just
    // render as nothing and hide the step.
    const root = node({ cost: 10, children: [node({ cost: 40 })] });
    expect(selfCost(root)).toBe(0);
  });

  it("is absent where the engine reports no cost at all", () => {
    // SQLite reports none, and inventing one would be inventing the finding.
    expect(selfCost(node())).toBeNull();
  });
});

describe("estimateError", () => {
  it("is symmetric — missing high and low are the same size of mistake", () => {
    expect(estimateError(node({ rows: 10, actual_rows: 400_000 }))).toBe(40_000);
    expect(estimateError(node({ rows: 400, actual_rows: 4 }))).toBe(100);
  });

  it("is absent without both numbers, which means without ANALYZE", () => {
    expect(estimateError(node({ rows: 10 }))).toBeNull();
    expect(estimateError(node({ actual_rows: 10 }))).toBeNull();
  });

  it("treats zero as no information rather than an infinite error", () => {
    expect(estimateError(node({ rows: 10, actual_rows: 0 }))).toBeNull();
    expect(estimateError(node({ rows: 0, actual_rows: 10 }))).toBeNull();
  });

  it("does not flag the ordinary imprecision every estimate has", () => {
    // Planners are routinely out by a factor of two and it means nothing.
    expect(estimateError(node({ rows: 100, actual_rows: 180 }))!).toBeLessThan(BAD_ESTIMATE);
  });
});

describe("formatRows", () => {
  it("keeps small counts exact and large ones readable", () => {
    expect(formatRows(1)).toBe("1");
    expect(formatRows(999)).toBe("999");
    expect(formatRows(12_400)).toBe("12.4K");
    expect(formatRows(3_500_000)).toBe("3.5M");
    expect(formatRows(2_000_000_000)).toBe("2.0B");
  });
});
