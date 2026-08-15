/**
 * Turning a result set into something that can be drawn.
 *
 * The one thing worth stating up front: a chart is pixels, and pixels are
 * approximate. Plotting an exact decimal means putting it through a float,
 * which is the conversion the rest of this application exists to avoid. That is
 * fine for deciding *where a bar ends* and not fine for telling someone what a
 * value is — so every point keeps the exact text alongside the number, and the
 * label a reader sees comes from the text, never from the float.
 *
 * Charts are drawn from the rows that have been fetched, not from the whole
 * result. That is the same limitation the grid's own sorting and filtering
 * have, and the same answer: say so rather than imply otherwise.
 */

import type { ResultSet, Value } from "./types";
import { formatValue } from "./value";

export type ChartKind = "bar" | "line" | "area" | "scatter";

export interface Point {
  /** Where to draw it. Lossy for wide decimals, and only ever a position. */
  value: number;
  /** What it actually is, digit for digit. What a reader is shown. */
  exact: string;
}

export interface Series {
  name: string;
  columnIndex: number;
  /** One per row, `null` where the row had no number there. */
  points: (Point | null)[];
}

export interface ChartData {
  /** One per row, taken from the label column. */
  labels: string[];
  series: Series[];
  /** Columns that could be plotted, whether or not they were chosen. */
  numericColumns: number[];
  /** Columns that could label the x axis. */
  labelColumns: number[];
}

/**
 * A cell as a number to plot, or `null` if it is not one.
 *
 * Exact decimals go through `Number`, which is lossy past about seventeen
 * digits — deliberately, and only here. The caller keeps the text.
 */
export function numericOf(cell: Value | undefined): number | null {
  if (!cell) return null;
  switch (cell.kind) {
    case "int":
    case "u_int":
    case "float":
      return Number.isFinite(cell.value) ? cell.value : null;
    case "numeric": {
      const parsed = Number(cell.value);
      return Number.isFinite(parsed) ? parsed : null;
    }
    case "bool":
      // A count of trues is a real thing to chart, and the alternative is
      // refusing to plot a column of them.
      return cell.value ? 1 : 0;
    default:
      return null;
  }
}

/** Whether a column holds anything plottable, judged from the rows present. */
function isNumericColumn(rows: Value[][], index: number): boolean {
  let seen = 0;
  for (const row of rows.slice(0, 200)) {
    const cell = row[index];
    if (!cell || cell.kind === "null") continue;
    if (numericOf(cell) === null) return false;
    seen += 1;
  }
  // A column of nothing but NULLs is not a numeric column; it is no column.
  return seen > 0;
}

/**
 * Build the series for a chart.
 *
 * `labelColumn` and `valueColumns` may be omitted, in which case the first
 * non-numeric column labels and every numeric column is plotted — which is the
 * shape of most `SELECT name, count(*) …` results and saves the common case a
 * round of configuration.
 */
export function buildChart(
  result: ResultSet,
  options: { labelColumn?: number; valueColumns?: number[] } = {},
): ChartData {
  const numericColumns = result.columns
    .map((_, index) => index)
    .filter((index) => isNumericColumn(result.rows, index));

  const labelColumns = result.columns
    .map((_, index) => index)
    .filter((index) => !numericColumns.includes(index));

  // A result that is entirely numeric still needs an axis; the row number is
  // the honest one, and is what `labels` falls back to below.
  const labelColumn =
    options.labelColumn ?? (labelColumns.length > 0 ? labelColumns[0] : undefined);

  const chosen = (options.valueColumns ?? numericColumns).filter((index) =>
    numericColumns.includes(index),
  );

  const labels = result.rows.map((row, i) => {
    if (labelColumn === undefined) return String(i + 1);
    const cell = row[labelColumn];
    return cell ? formatValue(cell) : String(i + 1);
  });

  const series: Series[] = chosen.map((columnIndex) => ({
    name: result.columns[columnIndex]?.name ?? `Column ${columnIndex + 1}`,
    columnIndex,
    points: result.rows.map((row) => {
      const cell = row[columnIndex];
      const value = numericOf(cell);
      if (value === null || !cell) return null;
      return { value, exact: formatValue(cell) };
    }),
  }));

  return { labels, series, numericColumns, labelColumns };
}

export interface Scale {
  min: number;
  max: number;
  ticks: number[];
}

/**
 * Axis bounds a person would have chosen.
 *
 * Rounded outward to a round number so the gridlines land on values worth
 * reading — 0, 25, 50 rather than 0, 23.7, 47.4. `includeZero` is on for bars
 * because a bar chart whose axis starts at 90 exaggerates every difference on
 * it, which is the most common way a chart misleads without lying.
 */
export function niceScale(values: number[], includeZero: boolean, tickCount = 5): Scale {
  const finite = values.filter((v) => Number.isFinite(v));
  if (finite.length === 0) return { min: 0, max: 1, ticks: [0, 1] };

  let min = Math.min(...finite);
  let max = Math.max(...finite);
  if (includeZero) {
    min = Math.min(min, 0);
    max = Math.max(max, 0);
  }

  // Every value identical: a zero-height range has no scale, so give it one
  // rather than dividing by zero and drawing nothing.
  if (min === max) {
    if (min === 0) return { min: 0, max: 1, ticks: [0, 0.5, 1] };
    const pad = Math.abs(min) * 0.1;
    min -= pad;
    max += pad;
  }

  const step = niceStep((max - min) / Math.max(1, tickCount));
  const niceMin = Math.floor(min / step) * step;
  const niceMax = Math.ceil(max / step) * step;

  const ticks: number[] = [];
  // Accumulated by index rather than by repeated addition, which drifts: after
  // twenty steps of 0.1 a running total is visibly not a round number.
  const count = Math.round((niceMax - niceMin) / step);
  for (let i = 0; i <= count; i++) {
    ticks.push(round(niceMin + i * step));
  }

  return { min: niceMin, max: niceMax, ticks };
}

/** The nearest 1, 2, 5 or 10 times a power of ten. */
function niceStep(rough: number): number {
  if (rough <= 0) return 1;
  const magnitude = Math.pow(10, Math.floor(Math.log10(rough)));
  const normalized = rough / magnitude;
  const factor = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return factor * magnitude;
}

/** Trim the float noise that arithmetic on a round step leaves behind. */
function round(value: number): number {
  return Number(value.toPrecision(12));
}

/** Axis labels, at the magnitude a reader can hold. */
export function formatTick(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 1_000_000_000) return `${round(value / 1_000_000_000)}B`;
  if (abs >= 1_000_000) return `${round(value / 1_000_000)}M`;
  if (abs >= 1_000) return `${round(value / 1_000)}K`;
  return String(round(value));
}
