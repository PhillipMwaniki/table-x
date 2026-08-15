import { describe, expect, it } from "vitest";
import { buildChart, formatTick, niceScale, numericOf } from "./chart";
import type { Column, ResultSet, Value } from "./types";

function result(names: string[], rows: Value[][]): ResultSet {
  return {
    type: "rows",
    columns: names.map((name): Column => ({ name, type_name: "unknown", nullable: true })),
    rows,
    truncated: false,
    editable: false,
    key_columns: [],
  } as ResultSet;
}

describe("numericOf", () => {
  it("reads every kind of number a driver can return", () => {
    expect(numericOf({ kind: "int", value: 42 })).toBe(42);
    expect(numericOf({ kind: "u_int", value: 7 })).toBe(7);
    expect(numericOf({ kind: "float", value: 1.5 })).toBe(1.5);
    expect(numericOf({ kind: "numeric", value: "2.25" })).toBe(2.25);
  });

  it("is null for anything that is not a number", () => {
    expect(numericOf({ kind: "text", value: "12" })).toBeNull();
    expect(numericOf({ kind: "null" })).toBeNull();
    expect(numericOf(undefined)).toBeNull();
  });

  it("counts booleans, because a chart of how many are true is a real chart", () => {
    expect(numericOf({ kind: "bool", value: true })).toBe(1);
    expect(numericOf({ kind: "bool", value: false })).toBe(0);
  });
});

describe("buildChart", () => {
  it("keeps the exact text beside the plotted number", () => {
    // The float is a position and nothing else. Nineteen significant digits do
    // not survive it, and the label a reader sees must not come from it.
    const wide = "123456789012345678.1234567890";
    const chart = buildChart(
      result(
        ["name", "balance"],
        [
          [
            { kind: "text", value: "a" },
            { kind: "numeric", value: wide },
          ],
        ],
      ),
    );

    const point = chart.series[0]?.points[0];
    expect(point?.exact).toBe(wide);
    // And the float genuinely did lose digits, which is why the text is kept.
    expect(String(point?.value)).not.toBe(wide);
  });

  it("labels from the first non-numeric column and plots the rest", () => {
    const chart = buildChart(
      result(
        ["name", "orders", "revenue"],
        [
          [
            { kind: "text", value: "alice" },
            { kind: "int", value: 3 },
            { kind: "numeric", value: "10.50" },
          ],
        ],
      ),
    );

    expect(chart.labels).toEqual(["alice"]);
    expect(chart.series.map((s) => s.name)).toEqual(["orders", "revenue"]);
  });

  it("falls back to row numbers when every column is numeric", () => {
    // The result still deserves an axis, and the row number is the honest one.
    const chart = buildChart(
      result(
        ["x", "y"],
        [
          [
            { kind: "int", value: 1 },
            { kind: "int", value: 10 },
          ],
          [
            { kind: "int", value: 2 },
            { kind: "int", value: 20 },
          ],
        ],
      ),
    );
    expect(chart.labels).toEqual(["1", "2"]);
    expect(chart.series).toHaveLength(2);
  });

  it("does not treat a column of nulls as numeric", () => {
    // It is not a numeric column; it is no column, and plotting a flat line of
    // nothing suggests data that is not there.
    const chart = buildChart(
      result(["name", "empty"], [[{ kind: "text", value: "a" }, { kind: "null" }]]),
    );
    expect(chart.numericColumns).toEqual([]);
    expect(chart.series).toEqual([]);
  });

  it("keeps a gap where a row has no number, rather than plotting zero", () => {
    // Zero is a value. Missing is not, and drawing one as the other invents a
    // reading that was never taken.
    const chart = buildChart(
      result(
        ["name", "n"],
        [
          [
            { kind: "text", value: "a" },
            { kind: "int", value: 5 },
          ],
          [{ kind: "text", value: "b" }, { kind: "null" }],
        ],
      ),
    );
    expect(chart.series[0]?.points[0]?.value).toBe(5);
    expect(chart.series[0]?.points[1]).toBeNull();
  });

  it("honours an explicit choice of columns", () => {
    const rows: Value[][] = [
      [
        { kind: "text", value: "a" },
        { kind: "int", value: 1 },
        { kind: "int", value: 2 },
      ],
    ];
    const chart = buildChart(result(["name", "x", "y"], rows), { valueColumns: [2] });
    expect(chart.series.map((s) => s.name)).toEqual(["y"]);
  });

  it("ignores a chosen column that cannot be plotted", () => {
    // Asking for a text column should give no series rather than a row of NaN.
    const chart = buildChart(
      result(
        ["name", "n"],
        [
          [
            { kind: "text", value: "a" },
            { kind: "int", value: 1 },
          ],
        ],
      ),
      { valueColumns: [0] },
    );
    expect(chart.series).toEqual([]);
  });
});

describe("niceScale", () => {
  it("rounds outward to values worth reading", () => {
    const scale = niceScale([3, 47], false);
    expect(scale.min).toBeLessThanOrEqual(3);
    expect(scale.max).toBeGreaterThanOrEqual(47);
    // Round numbers, not 3 and 47.
    expect(scale.ticks.every((t) => Number.isInteger(t))).toBe(true);
  });

  it("includes zero for bars, because a truncated axis exaggerates", () => {
    // A bar chart whose axis starts at 90 makes a 2% difference look like a
    // tenfold one. It is the most common way a chart misleads without lying.
    expect(niceScale([95, 100], true).min).toBe(0);
    // A line chart may legitimately zoom in, so it is not forced.
    expect(niceScale([95, 100], false).min).toBeGreaterThan(0);
  });

  it("gives a range to a set of identical values instead of dividing by zero", () => {
    const scale = niceScale([5, 5, 5], false);
    expect(scale.max).toBeGreaterThan(scale.min);
    expect(scale.ticks.length).toBeGreaterThan(1);
  });

  it("handles all zeroes and an empty set without producing NaN", () => {
    for (const scale of [niceScale([0, 0], false), niceScale([], false)]) {
      expect(Number.isFinite(scale.min)).toBe(true);
      expect(Number.isFinite(scale.max)).toBe(true);
      expect(scale.ticks.every(Number.isFinite)).toBe(true);
    }
  });

  it("spans negatives and positives together", () => {
    const scale = niceScale([-30, 40], false);
    expect(scale.min).toBeLessThanOrEqual(-30);
    expect(scale.max).toBeGreaterThanOrEqual(40);
    expect(scale.ticks).toContain(0);
  });

  it("produces ticks free of floating-point noise", () => {
    // Accumulating a step drifts: twenty additions of 0.1 is visibly not 2.
    const scale = niceScale([0, 1], false);
    for (const tick of scale.ticks) {
      expect(String(tick)).not.toMatch(/\d{10,}/);
    }
  });
});

describe("formatTick", () => {
  it("shortens the magnitudes that would otherwise not fit", () => {
    expect(formatTick(750)).toBe("750");
    expect(formatTick(12_000)).toBe("12K");
    expect(formatTick(3_400_000)).toBe("3.4M");
    expect(formatTick(-1_500)).toBe("-1.5K");
  });
});
