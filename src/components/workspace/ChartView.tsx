/**
 * A chart of the result on screen.
 *
 * Drawn by hand in SVG rather than with a charting library. This application's
 * whole position is that it is small and quiet — a few hundred kilobytes of
 * dependency to draw four chart types would be an odd thing to spend that on,
 * and drawing it here means the colours come from the same theme tokens as
 * everything else rather than from a library's palette.
 *
 * Values are shown from the exact text, never from the plotted float. The float
 * decides where a bar ends; it is not what the number is.
 */

import { useMemo, useState } from "react";
import { Banner, Button, Select, cx } from "../ui/primitives";
import { buildChart, formatTick, niceScale } from "@/lib/chart";
import type { ChartKind } from "@/lib/chart";
import type { ResultSet } from "@/lib/types";

const KINDS: { kind: ChartKind; label: string }[] = [
  { kind: "bar", label: "Bar" },
  { kind: "line", label: "Line" },
  { kind: "area", label: "Area" },
  { kind: "scatter", label: "Scatter" },
];

/** Series colours, in the order they are handed out. */
const PALETTE = [
  "var(--color-accent)",
  "var(--color-ok)",
  "var(--color-warn)",
  "var(--color-danger)",
  "#8b5cf6",
  "#06b6d4",
];

const PADDING = { top: 16, right: 16, bottom: 44, left: 56 };
const HEIGHT = 320;

export function ChartView({ result, onClose }: { result: ResultSet; onClose: () => void }) {
  const [kind, setKind] = useState<ChartKind>("bar");
  const [labelColumn, setLabelColumn] = useState<number | undefined>(undefined);
  const [valueColumn, setValueColumn] = useState<number | "all">("all");
  const [hover, setHover] = useState<{ series: number; point: number } | null>(null);

  const chart = useMemo(
    () =>
      buildChart(result, {
        ...(labelColumn !== undefined ? { labelColumn } : {}),
        ...(valueColumn !== "all" ? { valueColumns: [valueColumn] } : {}),
      }),
    [result, labelColumn, valueColumn],
  );

  const scale = useMemo(() => {
    const values = chart.series.flatMap((s) =>
      s.points.filter((p) => p !== null).map((p) => p!.value),
    );
    // Bars are measured from a baseline, so their axis has to include it.
    return niceScale(values, kind === "bar" || kind === "area");
  }, [chart, kind]);

  if (chart.numericColumns.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <Toolbar kind={kind} onKind={setKind} onClose={onClose} chart={chart} result={result}
          labelColumn={labelColumn} onLabelColumn={setLabelColumn}
          valueColumn={valueColumn} onValueColumn={setValueColumn} />
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          <p className="text-[12px] text-text-muted">
            Nothing here can be plotted — a chart needs a column of numbers, and this
            result has none.
          </p>
        </div>
      </div>
    );
  }

  const width = 900;
  const plotWidth = width - PADDING.left - PADDING.right;
  const plotHeight = HEIGHT - PADDING.top - PADDING.bottom;
  const span = scale.max - scale.min || 1;

  /** Where a value sits vertically. */
  const y = (value: number) => PADDING.top + plotHeight - ((value - scale.min) / span) * plotHeight;

  const count = chart.labels.length;
  /** The centre of a category's slot. */
  const x = (index: number) =>
    count <= 1
      ? PADDING.left + plotWidth / 2
      : PADDING.left + (index / (count - 1)) * plotWidth;

  /** Bars share a slot between the series, so each gets a share of it. */
  const slot = count > 0 ? plotWidth / count : plotWidth;
  const barWidth = Math.max(1, (slot * 0.7) / Math.max(1, chart.series.length));

  const hovered =
    hover && chart.series[hover.series]?.points[hover.point]
      ? {
          label: chart.labels[hover.point] ?? "",
          name: chart.series[hover.series]!.name,
          exact: chart.series[hover.series]!.points[hover.point]!.exact,
        }
      : null;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Toolbar kind={kind} onKind={setKind} onClose={onClose} chart={chart} result={result}
        labelColumn={labelColumn} onLabelColumn={setLabelColumn}
        valueColumn={valueColumn} onValueColumn={setValueColumn} />

      {result.truncated && (
        <div className="shrink-0 px-2 pt-2">
          {/* The same honesty the grid's own sorting needs: this is a chart of
              what was fetched, and a chart of part of a result looks exactly
              like a chart of all of it. */}
          <Banner tone="info">
            This charts the rows loaded so far, not the whole result. Raise the page size
            or add a GROUP BY to chart everything.
          </Banner>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto p-2">
        <svg
          viewBox={`0 0 ${width} ${HEIGHT}`}
          className="w-full"
          style={{ minWidth: Math.max(width, count * 24) }}
          role="img"
          aria-label={`${kind} chart of ${chart.series.map((s) => s.name).join(", ")}`}
        >
          {/* Gridlines and the value axis. */}
          {scale.ticks.map((tick) => (
            <g key={tick}>
              <line
                x1={PADDING.left}
                x2={width - PADDING.right}
                y1={y(tick)}
                y2={y(tick)}
                stroke="var(--color-border)"
                strokeWidth={tick === 0 ? 1.5 : 1}
                opacity={tick === 0 ? 0.9 : 0.4}
              />
              <text
                x={PADDING.left - 8}
                y={y(tick) + 3}
                textAnchor="end"
                className="fill-[var(--color-text-muted)] text-[10px]"
              >
                {formatTick(tick)}
              </text>
            </g>
          ))}

          {chart.series.map((series, s) => {
            const colour = PALETTE[s % PALETTE.length];

            if (kind === "bar") {
              return (
                <g key={series.columnIndex}>
                  {series.points.map((point, i) =>
                    point === null ? null : (
                      <rect
                        key={i}
                        x={
                          PADDING.left +
                          i * slot +
                          slot * 0.15 +
                          s * barWidth
                        }
                        y={Math.min(y(point.value), y(0))}
                        width={barWidth}
                        height={Math.max(1, Math.abs(y(point.value) - y(0)))}
                        fill={colour}
                        opacity={hover && hover.point !== i ? 0.45 : 0.9}
                        onMouseEnter={() => setHover({ series: s, point: i })}
                        onMouseLeave={() => setHover(null)}
                      />
                    ),
                  )}
                </g>
              );
            }

            if (kind === "scatter") {
              return (
                <g key={series.columnIndex}>
                  {series.points.map((point, i) =>
                    point === null ? null : (
                      <circle
                        key={i}
                        cx={x(i)}
                        cy={y(point.value)}
                        r={3.5}
                        fill={colour}
                        opacity={hover && hover.point !== i ? 0.4 : 0.9}
                        onMouseEnter={() => setHover({ series: s, point: i })}
                        onMouseLeave={() => setHover(null)}
                      />
                    ),
                  )}
                </g>
              );
            }

            // Line and area share their path; the area adds a floor.
            const segments = pathOf(series.points, x, y);
            return (
              <g key={series.columnIndex}>
                {kind === "area" &&
                  segments.map((segment, i) => (
                    <path
                      key={`fill-${i}`}
                      d={`${segment.d} L ${segment.lastX} ${y(scale.min)} L ${segment.firstX} ${y(scale.min)} Z`}
                      fill={colour}
                      opacity={0.18}
                    />
                  ))}
                {segments.map((segment, i) => (
                  <path
                    key={i}
                    d={segment.d}
                    fill="none"
                    stroke={colour}
                    strokeWidth={1.75}
                    strokeLinejoin="round"
                  />
                ))}
                {series.points.map((point, i) =>
                  point === null ? null : (
                    <circle
                      key={`dot-${i}`}
                      cx={x(i)}
                      cy={y(point.value)}
                      r={hover?.point === i ? 4 : 2.5}
                      fill={colour}
                      onMouseEnter={() => setHover({ series: s, point: i })}
                      onMouseLeave={() => setHover(null)}
                    />
                  ),
                )}
              </g>
            );
          })}

          {/* Category labels, thinned so they never overlap into a smear. */}
          {chart.labels.map((label, i) => {
            const every = Math.ceil(count / Math.max(1, Math.floor(plotWidth / 60)));
            if (i % every !== 0) return null;
            const cx = kind === "bar" ? PADDING.left + i * slot + slot / 2 : x(i);
            return (
              <text
                key={i}
                x={cx}
                y={HEIGHT - PADDING.bottom + 16}
                textAnchor="middle"
                className="fill-[var(--color-text-muted)] text-[10px]"
              >
                {label.length > 12 ? `${label.slice(0, 11)}…` : label}
              </text>
            );
          })}
        </svg>
      </div>

      <div className="flex h-7 shrink-0 items-center gap-3 border-t border-border bg-surface-1 px-2 text-[11px]">
        {chart.series.map((series, s) => (
          <span key={series.columnIndex} className="flex items-center gap-1.5">
            <span
              className="inline-block size-2 rounded-sm"
              style={{ background: PALETTE[s % PALETTE.length] }}
            />
            <span className="text-text-muted">{series.name}</span>
          </span>
        ))}

        <div className="flex-1" />

        {/* The exact text, never the plotted float — the float is a position. */}
        {hovered && (
          <span className="font-mono text-text">
            {hovered.label} · {hovered.name} ={" "}
            <span className="font-medium">{hovered.exact}</span>
          </span>
        )}
      </div>
    </div>
  );
}

/**
 * Split a series into the runs that actually have values.
 *
 * A missing point breaks the line rather than being bridged across: joining two
 * readings over a gap draws a trend through data that was never collected.
 */
function pathOf(
  points: ({ value: number } | null)[],
  x: (i: number) => number,
  y: (v: number) => number,
): { d: string; firstX: number; lastX: number }[] {
  const segments: { d: string; firstX: number; lastX: number }[] = [];
  let current: string[] = [];
  let firstX = 0;
  let lastX = 0;

  const flush = () => {
    if (current.length > 0) segments.push({ d: current.join(" "), firstX, lastX });
    current = [];
  };

  points.forEach((point, i) => {
    if (point === null) {
      flush();
      return;
    }
    const px = x(i);
    if (current.length === 0) {
      firstX = px;
      current.push(`M ${px} ${y(point.value)}`);
    } else {
      current.push(`L ${px} ${y(point.value)}`);
    }
    lastX = px;
  });
  flush();

  return segments;
}

function Toolbar({
  kind,
  onKind,
  onClose,
  chart,
  result,
  labelColumn,
  onLabelColumn,
  valueColumn,
  onValueColumn,
}: {
  kind: ChartKind;
  onKind: (kind: ChartKind) => void;
  onClose: () => void;
  chart: ReturnType<typeof buildChart>;
  result: ResultSet;
  labelColumn: number | undefined;
  onLabelColumn: (index: number | undefined) => void;
  valueColumn: number | "all";
  onValueColumn: (index: number | "all") => void;
}) {
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 text-[11px]">
      <span className="flex gap-0.5">
        {KINDS.map((option) => (
          <button
            key={option.kind}
            onClick={() => onKind(option.kind)}
            className={cx(
              "rounded px-1.5 py-0.5",
              kind === option.kind
                ? "bg-surface-3 text-text"
                : "text-text-muted hover:bg-surface-2 hover:text-text",
            )}
          >
            {option.label}
          </button>
        ))}
      </span>

      {chart.labelColumns.length > 0 && (
        <label className="flex items-center gap-1 text-text-muted">
          Labels
          <Select
            className="h-5 w-auto"
            value={labelColumn ?? chart.labelColumns[0] ?? 0}
            onChange={(e) => onLabelColumn(Number(e.target.value))}
          >
            {chart.labelColumns.map((index) => (
              <option key={index} value={index}>
                {result.columns[index]?.name}
              </option>
            ))}
          </Select>
        </label>
      )}

      {chart.numericColumns.length > 1 && (
        <label className="flex items-center gap-1 text-text-muted">
          Values
          <Select
            className="h-5 w-auto"
            value={valueColumn}
            onChange={(e) =>
              onValueColumn(e.target.value === "all" ? "all" : Number(e.target.value))
            }
          >
            <option value="all">All numeric columns</option>
            {chart.numericColumns.map((index) => (
              <option key={index} value={index}>
                {result.columns[index]?.name}
              </option>
            ))}
          </Select>
        </label>
      )}

      <div className="flex-1" />
      <Button variant="ghost" className="h-5" onClick={onClose}>
        Back to rows
      </Button>
    </div>
  );
}
