/**
 * Virtualized result grid.
 *
 * Only the visible rows are in the DOM, so a 100k-row page scrolls at the same
 * cost as a 20-row one. Columns are not virtualized: result sets are wide in
 * pathological cases but almost never wide enough to justify the complexity, and
 * horizontal virtualization breaks native column-drag selection.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { cellClass, editText, formatValue, isInlineEditable, isNumeric, parseEdit } from "@/lib/value";
import { cx } from "../ui/primitives";
import { rowHeightFor } from "@/lib/settings";
import { useSettings } from "@/store/settings";
import type { Column, ResultSet, Value } from "@/lib/types";

const MIN_COL_WIDTH = 80;
const MAX_INITIAL_COL_WIDTH = 320;

type SortDirection = "asc" | "desc";

interface Sort {
  columnIndex: number;
  direction: SortDirection;
}

/** Compare two values for sorting, keeping NULLs together at the end. */
function compareValues(a: Value, b: Value): number {
  if (a.kind === "null" && b.kind === "null") return 0;
  if (a.kind === "null") return 1;
  if (b.kind === "null") return -1;

  if (isNumeric(a) && isNumeric(b)) {
    // Exact numerics arrive as strings and can exceed Number's range, so fall
    // back to a length-then-lexical comparison when parsing is lossy.
    const na = Number(formatValue(a));
    const nb = Number(formatValue(b));
    if (Number.isFinite(na) && Number.isFinite(nb)) return na - nb;
  }
  return formatValue(a).localeCompare(formatValue(b), undefined, { numeric: true });
}

export function ResultGrid({
  result,
  onEdit,
  readOnlyReason,
}: {
  result: ResultSet;
  onEdit: (rowIndex: number, columnIndex: number, next: Value) => Promise<void>;
  /** Why editing is unavailable, shown when a user tries. */
  readOnlyReason?: string | undefined;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  // Row height and column widths are both measured in characters, so they have
  // to be recomputed when the data font size changes rather than read from CSS.
  const fontSize = useSettings((s) => s.dataFontSize);
  const rowHeight = rowHeightFor(fontSize);
  const [sort, setSort] = useState<Sort | null>(null);
  const [filter, setFilter] = useState("");
  const [editing, setEditing] = useState<{ row: number; col: number } | null>(null);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [cellError, setCellError] = useState<string | null>(null);

  // Column widths are measured from a sample of rows rather than every row:
  // scanning 100k rows to size a column is not worth the frame it costs.
  const widths = useMemo(() => {
    const sample = result.rows.slice(0, 100);
    // A monospace advance is close enough to 0.6em for sizing; measuring text
    // properly would cost a layout pass per column for a few pixels.
    const charWidth = fontSize * 0.6;
    return result.columns.map((col, i) => {
      const longest = sample.reduce((max, row) => {
        const cell = row[i];
        return Math.max(max, cell ? formatValue(cell).length : 0);
      }, col.name.length);
      return Math.min(
        Math.max(Math.round(longest * charWidth) + 24, MIN_COL_WIDTH),
        MAX_INITIAL_COL_WIDTH,
      );
    });
  }, [result, fontSize]);

  /**
   * Rows after filtering and sorting, carrying their original index so edits
   * still address the right row in the underlying result.
   */
  const view = useMemo(() => {
    let indexed = result.rows.map((row, index) => ({ row, index }));

    if (filter.trim()) {
      const needle = filter.toLowerCase();
      indexed = indexed.filter(({ row }) =>
        row.some((cell) => formatValue(cell).toLowerCase().includes(needle)),
      );
    }

    if (sort) {
      const { columnIndex, direction } = sort;
      indexed = [...indexed].sort((a, b) => {
        const av = a.row[columnIndex];
        const bv = b.row[columnIndex];
        if (!av || !bv) return 0;
        const cmp = compareValues(av, bv);
        return direction === "asc" ? cmp : -cmp;
      });
    }
    return indexed;
  }, [result.rows, filter, sort]);

  const virtualizer = useVirtualizer({
    count: view.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => rowHeight,
    overscan: 12,
  });

  // The virtualizer caches measurements, so a size change has to invalidate
  // them or every row keeps the height it had at the old font size.
  useEffect(() => {
    virtualizer.measure();
  }, [rowHeight, virtualizer]);

  const beginEdit = useCallback(
    (rowIndex: number, colIndex: number, value: Value) => {
      if (!result.editable) return;
      if (!isInlineEditable(value)) return;
      setEditing({ row: rowIndex, col: colIndex });
      setDraft(editText(value));
      setCellError(null);
    },
    [result.editable],
  );

  const commit = useCallback(async () => {
    if (!editing) return;
    const row = result.rows[editing.row];
    const original = row?.[editing.col];
    if (!original) return setEditing(null);

    const next = parseEdit(draft, original);
    if (formatValue(next) === formatValue(original) && next.kind === original.kind) {
      // Nothing changed — skip the round trip rather than writing an identical
      // value and pushing a no-op onto the undo stack.
      return setEditing(null);
    }

    setSaving(true);
    try {
      await onEdit(editing.row, editing.col, next);
      setEditing(null);
    } catch (e) {
      // Stay in edit mode so the typed value is not lost.
      setCellError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }, [editing, draft, result.rows, onEdit]);

  // Escape leaves edit mode from anywhere in the grid.
  useEffect(() => {
    if (!editing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setEditing(null);
        setCellError(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [editing]);

  const totalWidth = widths.reduce((sum, w) => sum + w, 0);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <GridToolbar
        result={result}
        filter={filter}
        onFilter={setFilter}
        visible={view.length}
        readOnlyReason={readOnlyReason}
      />

      {cellError && (
        <div role="alert" className="shrink-0 bg-danger/10 px-2 py-1 text-[11px] text-danger">
          {cellError}
        </div>
      )}

      <div ref={scroller} className="min-h-0 flex-1 overflow-auto">
        <div style={{ width: totalWidth, minWidth: "100%" }}>
          {/* Header stays put while the body scrolls under it. */}
          <div className="sticky top-0 z-10 flex border-b border-border bg-surface-2">
            {result.columns.map((col, i) => (
              <HeaderCell
                key={`${col.name}-${i}`}
                column={col}
                width={widths[i] ?? MIN_COL_WIDTH}
                sort={sort?.columnIndex === i ? sort.direction : null}
                isKey={result.key_columns.includes(col.name)}
                onSort={() =>
                  setSort((s) =>
                    s?.columnIndex === i && s.direction === "asc"
                      ? { columnIndex: i, direction: "desc" }
                      : s?.columnIndex === i && s.direction === "desc"
                        ? null
                        : { columnIndex: i, direction: "asc" },
                  )
                }
              />
            ))}
          </div>

          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((virtual) => {
              const entry = view[virtual.index];
              if (!entry) return null;
              const { row, index: sourceIndex } = entry;
              return (
                <div
                  key={virtual.key}
                  className="absolute flex border-b border-border/40 hover:bg-surface-1"
                  style={{
                    top: 0,
                    left: 0,
                    height: virtual.size,
                    transform: `translateY(${virtual.start}px)`,
                  }}
                >
                  {row.map((cell, colIndex) => {
                    const isEditing =
                      editing?.row === sourceIndex && editing.col === colIndex;
                    return (
                      <Cell
                        key={colIndex}
                        value={cell}
                        width={widths[colIndex] ?? MIN_COL_WIDTH}
                        editable={result.editable}
                        editing={isEditing}
                        saving={isEditing && saving}
                        draft={draft}
                        onDraft={setDraft}
                        onBegin={() => beginEdit(sourceIndex, colIndex, cell)}
                        onCommit={commit}
                      />
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

function GridToolbar({
  result,
  filter,
  onFilter,
  visible,
  readOnlyReason,
}: {
  result: ResultSet;
  filter: string;
  onFilter: (value: string) => void;
  visible: number;
  readOnlyReason?: string | undefined;
}) {
  return (
    <div className="flex h-7 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 text-[11px]">
      <input
        value={filter}
        onChange={(e) => onFilter(e.target.value)}
        placeholder="Filter rows…"
        className="h-5 w-44 rounded border border-border bg-surface-0 px-1.5 text-[11px] focus:border-accent focus:outline-none"
      />

      <span className="text-text-muted">
        {filter ? `${visible} of ${result.rows.length}` : `${result.rows.length}`} rows
      </span>

      {/* Being explicit that a capped page is not the whole table. Presenting a
          partial result as complete is the kind of thing that misleads someone
          into a wrong conclusion about their data. */}
      {result.truncated && (
        <span className="rounded bg-warn/15 px-1.5 py-0.5 text-warn">
          Showing the first {result.rows.length} rows — more exist
        </span>
      )}

      {/* Sorting a truncated page sorts only what was fetched, which is not the
          same as the top N of the table. Say so rather than implying otherwise. */}
      {result.truncated && (
        <span className="text-text-muted/70">Sort and filter apply to loaded rows only</span>
      )}

      <div className="flex-1" />

      {result.editable ? (
        <span className="text-text-muted">Double-click to edit</span>
      ) : (
        <span className="text-text-muted" title={readOnlyReason}>
          Read-only
        </span>
      )}
    </div>
  );
}

function HeaderCell({
  column,
  width,
  sort,
  isKey,
  onSort,
}: {
  column: Column;
  width: number;
  sort: SortDirection | null;
  isKey: boolean;
  onSort: () => void;
}) {
  return (
    <button
      onClick={onSort}
      style={{ width }}
      title={`${column.name} — ${column.type_name}${column.nullable === false ? " NOT NULL" : ""}`}
      className="flex shrink-0 items-center gap-1 border-r border-border px-2 py-1 text-left hover:bg-surface-3"
    >
      <span className="flex min-w-0 flex-col leading-tight">
        <span className="flex items-center gap-1 truncate text-[length:var(--text-data)] font-medium text-text">
          {isKey && (
            <span aria-label="Key column" title="Key column" className="text-accent">
              ⚿
            </span>
          )}
          {column.name}
        </span>
        {/* The type line sits a fixed ratio under the column name, so it stays
            legible rather than vanishing as the data size grows. */}
        <span className="truncate font-mono text-[length:calc(var(--text-data)*0.78)] text-text-muted">
          {column.type_name}
        </span>
      </span>
      <span className="ml-auto text-[9px] text-text-muted">
        {sort === "asc" ? "▲" : sort === "desc" ? "▼" : ""}
      </span>
    </button>
  );
}

function Cell({
  value,
  width,
  editable,
  editing,
  saving,
  draft,
  onDraft,
  onBegin,
  onCommit,
}: {
  value: Value;
  width: number;
  editable: boolean;
  editing: boolean;
  saving: boolean;
  draft: string;
  onDraft: (value: string) => void;
  onBegin: () => void;
  onCommit: () => void;
}) {
  if (editing) {
    return (
      <div style={{ width }} className="shrink-0 border-r border-border p-0">
        <input
          autoFocus
          value={draft}
          disabled={saving}
          onChange={(e) => onDraft(e.target.value)}
          onBlur={onCommit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onCommit();
            }
          }}
          style={{ height: "var(--row-height)" }}
          className="w-full border border-accent bg-surface-0 px-1.5 font-mono text-[length:var(--text-data)] outline-none"
        />
      </div>
    );
  }

  return (
    <div
      style={{ width }}
      onDoubleClick={editable ? onBegin : undefined}
      className={cx(
        "shrink-0 truncate border-r border-border px-2 font-mono",
        "text-[length:var(--text-data)] leading-[var(--row-height)]",
        isNumeric(value) && "text-right",
        cellClass(value),
        editable && "cursor-text",
      )}
      title={value.kind === "bytes" ? undefined : formatValue(value)}
    >
      {formatValue(value)}
    </div>
  );
}
