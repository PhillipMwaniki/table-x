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
import {
  cellClass,
  compareDecimalText,
  editText,
  formatValue,
  isInlineEditable,
  isNumeric,
  parseEdit,
} from "@/lib/value";
import { cx } from "../ui/primitives";
import { FILTER_HINT, matchesFilter, parseFilter } from "@/lib/filter";
import { editorFor } from "@/lib/editors";
import { BinaryViewer, BoolEditor, InlineEditor, ValuePanel } from "./CellEditor";
import { PAGE_SIZES, rowHeightFor } from "@/lib/settings";
import { guaranteesFor } from "@/lib/guarantees";
import type { Precision } from "@/lib/guarantees";
import { GuaranteesPanel } from "./GuaranteesPanel";
import { ChartView } from "./ChartView";
import { useSettings } from "@/store/settings";
import type { Column, ResultSet, Value } from "@/lib/types";

const MIN_COL_WIDTH = 80;
const MAX_INITIAL_COL_WIDTH = 320;

/** Width of the row-number gutter, wide enough for five digits. */
const GUTTER_WIDTH = 52;

/**
 * The empty selection, as one shared instance.
 *
 * A fresh `new Set()` on every clear would be a new object each time, and the
 * memos that derive from the selection compare by identity — so clearing an
 * already-empty selection would recompute everything downstream of it.
 */
const EMPTY_SELECTION: ReadonlySet<number> = new Set<number>();

/** What the grid needs to draw page controls and say where it is. */
export interface PagingProps {
  /** Rows skipped to reach this page. */
  offset: number;
  /** Rows this page asked for. */
  limit: number;
  /** Whether the statement orders its rows — see `hasOrderBy`. */
  ordered: boolean;
  busy: boolean;
  onGoTo: (offset: number) => void;
  onPageSize: (rows: number) => void;
  /**
   * Columns that would make paging reliable, when the result has a key.
   *
   * A table tab has no editor, so telling its reader to add an `ORDER BY` would
   * be advice they cannot take. Offering to add it for them is the same
   * sentence with somewhere to go.
   */
  orderableBy?: string[] | undefined;
  onOrderBy?: (() => void) | undefined;
}

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

  // Digit-by-digit where both sides are decimals, so sorting a NUMERIC column
  // does not collapse values that differ past the 17th digit.
  const exact = compareDecimalText(formatValue(a), formatValue(b));
  if (exact !== null) return exact;
  return formatValue(a).localeCompare(formatValue(b), undefined, { numeric: true });
}

export function ResultGrid({
  result,
  onEdit,
  paging,
  onExportRows,
  readOnlyDetail,
  onInsertRow,
  onDeleteRows,
}: {
  result: ResultSet;
  onEdit: (rowIndex: number, columnIndex: number, next: Value) => Promise<void>;
  /** Page controls, absent for results that are not a page of anything. */
  paging?: PagingProps | undefined;
  /** Write the given rows to a file. Absent where there is nothing to write to. */
  onExportRows?: ((rows: Value[][]) => void) | undefined;
  /** Why editing is off, and what would turn it on. */
  readOnlyDetail?: { reason: string; remedy: string } | undefined;
  /** Add a row. Absent where the result is not a single writable table. */
  onInsertRow?: (() => void) | undefined;
  /** Remove these rows, given by their index in the result. */
  onDeleteRows?: ((rowIndexes: number[]) => void) | undefined;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  // Row height and column widths are both measured in characters, so they have
  // to be recomputed when the data font size changes rather than read from CSS.
  const fontSize = useSettings((s) => s.dataFontSize);
  const rowHeight = rowHeightFor(fontSize);
  const [sort, setSort] = useState<Sort | null>(null);
  const [filter, setFilter] = useState("");
  /** Per-column expressions, keyed by column index. */
  const [columnFilters, setColumnFilters] = useState<Record<number, string>>({});
  const [editing, setEditing] = useState<{ row: number; col: number } | null>(null);
  /** A binary cell open for reading. Viewing is not editing. */
  const [viewing, setViewing] = useState<{ row: number; col: number } | null>(null);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [cellError, setCellError] = useState<string | null>(null);
  /** Source indices of the picked rows. */
  const [selected, setSelected] = useState<ReadonlySet<number>>(EMPTY_SELECTION);
  /** Where the last plain click landed, so shift-click has a range to extend. */
  const anchor = useRef<number | null>(null);
  const [showGuarantees, setShowGuarantees] = useState(false);
  /** Charting replaces the rows rather than sitting beside them: both want the
      whole pane, and half a grid next to half a chart serves neither. */
  const [charting, setCharting] = useState(false);

  // Derived from the values that arrived rather than the declared types, so it
  // is evidence rather than a claim — see `precisionOf`.
  const guarantees = useMemo(() => guaranteesFor(result), [result]);

  // A new result set is a different set of rows; carrying a selection across
  // would leave row 4 of the old result selected in the new one, which is a
  // different row.
  useEffect(() => {
    setSelected(EMPTY_SELECTION);
    anchor.current = null;
  }, [result]);

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

    // Column filters are parsed once per keystroke rather than once per cell:
    // on a hundred thousand rows that is the difference between typing and
    // waiting.
    const active = Object.entries(columnFilters)
      .map(([index, text]) => ({ index: Number(index), predicate: parseFilter(text) }))
      .filter(({ predicate }) => predicate.kind !== "any");

    if (active.length > 0) {
      // Every filter must pass: adding a second one narrows, which is what a
      // row of boxes above columns leads a person to expect.
      indexed = indexed.filter(({ row }) =>
        active.every(({ index, predicate }) => {
          const cell = row[index];
          return cell ? matchesFilter(cell, predicate) : false;
        }),
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
  }, [result.rows, filter, columnFilters, sort]);

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

  /**
   * Apply a click on the row gutter.
   *
   * The three modifiers do what they do in every file list: plain replaces,
   * ctrl/cmd toggles one, shift extends from the last plain click. Ranges are
   * taken in *view* order rather than source order, because the user is
   * pointing at what they can see — with a sort applied, the rows between two
   * clicks are the ones drawn between them.
   */
  const clickRow = useCallback(
    (viewIndex: number, modifiers: { shift: boolean; toggle: boolean }) => {
      const entry = view[viewIndex];
      if (!entry) return;

      setSelected((was) => {
        if (modifiers.shift && anchor.current !== null) {
          const from = view.findIndex((v) => v.index === anchor.current);
          if (from !== -1) {
            const [lo, hi] = from < viewIndex ? [from, viewIndex] : [viewIndex, from];
            const next = new Set(was);
            for (let i = lo; i <= hi; i++) {
              const row = view[i];
              if (row) next.add(row.index);
            }
            return next;
          }
        }

        if (modifiers.toggle) {
          const next = new Set(was);
          if (!next.delete(entry.index)) next.add(entry.index);
          anchor.current = entry.index;
          return next;
        }

        anchor.current = entry.index;
        // Clicking an already-alone selection clears it, so there is a way back
        // to nothing selected that does not involve a modifier key.
        if (was.size === 1 && was.has(entry.index)) return EMPTY_SELECTION;
        return new Set([entry.index]);
      });
    },
    [view],
  );

  /** The picked rows, in the order they are displayed. */
  const selectedRows = useMemo(
    () => view.filter((entry) => selected.has(entry.index)).map((entry) => entry.row),
    [view, selected],
  );

  const beginEdit = useCallback(
    (rowIndex: number, colIndex: number, value: Value) => {
      // Binary is shown rather than edited, and worth showing even when the
      // result as a whole cannot be written to.
      if (value.kind === "bytes") {
        setViewing({ row: rowIndex, col: colIndex });
        return;
      }
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

  /**
   * Write a value that was chosen rather than typed.
   *
   * The text path exists to turn a draft string back into a value; a control
   * with three options has already produced one, and routing it through text
   * would only add a way to get it wrong.
   */
  const commitValue = useCallback(
    async (rowIndex: number, colIndex: number, next: Value) => {
      const original = result.rows[rowIndex]?.[colIndex];
      if (!original) return;
      if (formatValue(next) === formatValue(original) && next.kind === original.kind) {
        setEditing(null);
        return;
      }
      setSaving(true);
      try {
        await onEdit(rowIndex, colIndex, next);
        setEditing(null);
      } catch (e) {
        setCellError((e as Error).message);
      } finally {
        setSaving(false);
      }
    },
    [result.rows, onEdit],
  );

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

  // The value under an open panel or viewer, if any.
  const panelValue = editing ? result.rows[editing.row]?.[editing.col] : undefined;
  const panelKind = panelValue ? editorFor(panelValue) : null;
  const viewingValue = viewing ? result.rows[viewing.row]?.[viewing.col] : undefined;

  if (charting) {
    return <ChartView result={result} onClose={() => setCharting(false)} />;
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {panelValue && (panelKind === "json" || panelKind === "text") && (
        <ValuePanel
          title={`${result.columns[editing!.col]?.name ?? "Value"} — row ${editing!.row + 1}`}
          draft={draft}
          json={panelKind === "json"}
          saving={saving}
          onDraft={setDraft}
          onCommit={() => void commit()}
          onCancel={() => {
            setEditing(null);
            setCellError(null);
          }}
        />
      )}

      {viewingValue?.kind === "bytes" && (
        <BinaryViewer bytes={viewingValue.value} onClose={() => setViewing(null)} />
      )}

      <GridToolbar
        result={result}
        filter={filter}
        onFilter={setFilter}
        visible={view.length}
        columnFilterCount={Object.keys(columnFilters).length}
        onClearColumnFilters={() => setColumnFilters({})}
        selectedCount={selected.size}
        onClearSelection={() => setSelected(EMPTY_SELECTION)}
        onExportSelected={onExportRows ? () => onExportRows(selectedRows) : undefined}
        onExplain={() => setShowGuarantees(true)}
        onChart={() => setCharting(true)}
        onInsertRow={onInsertRow}
        onDeleteSelected={
          onDeleteRows
            ? () =>
                onDeleteRows(
                  // Source indices, in display order, so the confirmation lists
                  // them the way they are on screen.
                  view.filter((entry) => selected.has(entry.index)).map((entry) => entry.index),
                )
            : undefined
        }
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
            <div
              style={{ width: GUTTER_WIDTH }}
              className="sticky left-0 z-20 shrink-0 border-r border-border bg-surface-2 p-0"
            >
              <button
                type="button"
                onClick={() =>
                  setSelected((was) =>
                    was.size === view.length && view.length > 0
                      ? EMPTY_SELECTION
                      : new Set(view.map((entry) => entry.index)),
                  )
                }
                // Everything *visible*, which with a filter on is not
                // everything fetched. The count beside it says which.
                title={
                  selected.size === view.length && view.length > 0
                    ? "Clear the selection"
                    : "Select every row shown"
                }
                className="flex h-full w-full items-center justify-center text-[10px] text-text-muted hover:text-text"
              >
                {selected.size > 0 && selected.size === view.length ? "■" : "□"}
              </button>
            </div>
            {result.columns.map((col, i) => (
              <HeaderCell
                key={`${col.name}-${i}`}
                column={col}
                width={widths[i] ?? MIN_COL_WIDTH}
                sort={sort?.columnIndex === i ? sort.direction : null}
                isKey={result.key_columns.includes(col.name)}
                precision={guarantees.columns[i]?.precision ?? "none"}
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

          {/* Filter row, directly under the names it filters — the association
              is positional, so it needs no labels of its own. */}
          <div className="sticky top-[var(--header-height,2.6rem)] z-10 flex border-b border-border bg-surface-1">
            <div
              style={{ width: GUTTER_WIDTH }}
              className="sticky left-0 z-20 shrink-0 border-r border-border bg-surface-1"
            />
            {result.columns.map((col, i) => (
              <div
                key={`filter-${col.name}-${i}`}
                style={{ width: widths[i] ?? MIN_COL_WIDTH }}
                className="shrink-0 border-r border-border p-0.5"
              >
                <input
                  value={columnFilters[i] ?? ""}
                  onChange={(e) =>
                    setColumnFilters((was) => {
                      const next = { ...was };
                      // Removed rather than stored empty, so the count of
                      // active filters is simply the size of this object.
                      if (e.target.value) next[i] = e.target.value;
                      else delete next[i];
                      return next;
                    })
                  }
                  placeholder="filter"
                  aria-label={`Filter ${col.name}`}
                  title={FILTER_HINT}
                  className={cx(
                    "h-5 w-full rounded-sm border bg-surface-0 px-1 font-mono outline-none",
                    "text-[length:calc(var(--text-data)*0.85)]",
                    "placeholder:text-text-muted/40 focus:border-accent",
                    columnFilters[i] ? "border-accent/60" : "border-transparent",
                  )}
                />
              </div>
            ))}
          </div>

          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((virtual) => {
              const entry = view[virtual.index];
              if (!entry) return null;
              const { row, index: sourceIndex } = entry;
              const isSelected = selected.has(sourceIndex);
              return (
                <div
                  key={virtual.key}
                  className={cx(
                    "absolute flex border-b border-border/40",
                    isSelected ? "bg-accent/15" : "hover:bg-surface-1",
                  )}
                  style={{
                    top: 0,
                    left: 0,
                    height: virtual.size,
                    transform: `translateY(${virtual.start}px)`,
                  }}
                >
                  <div
                    style={{ width: GUTTER_WIDTH }}
                    onMouseDown={(e) => {
                      // A shift-click inside a scroller selects text as well as
                      // rows unless the default is refused.
                      if (e.shiftKey) e.preventDefault();
                      clickRow(virtual.index, {
                        shift: e.shiftKey,
                        toggle: e.ctrlKey || e.metaKey,
                      });
                    }}
                    className={cx(
                      "sticky left-0 z-10 shrink-0 cursor-pointer border-r border-border select-none",
                      "text-right font-mono text-[10px] leading-[var(--row-height)] tabular-nums",
                      isSelected
                        ? "bg-accent/25 text-text"
                        : "bg-surface-1 text-text-muted/60 hover:text-text",
                    )}
                    // The number is the row's place in the page, not its id —
                    // and with an offset it continues from where the last page
                    // ended rather than restarting at one.
                    title={`Row ${(paging?.offset ?? 0) + sourceIndex + 1}`}
                  >
                    <span className="px-1">{(paging?.offset ?? 0) + sourceIndex + 1}</span>
                  </div>

                  {row.map((cell, colIndex) => {
                    const isEditing = editing?.row === sourceIndex && editing.col === colIndex;
                    return (
                      <Cell
                        key={colIndex}
                        value={cell}
                        width={widths[colIndex] ?? MIN_COL_WIDTH}
                        editable={result.editable}
                        nullable={result.columns[colIndex]?.nullable !== false}
                        editing={isEditing}
                        saving={isEditing && saving}
                        draft={draft}
                        onDraft={setDraft}
                        onBegin={() => beginEdit(sourceIndex, colIndex, cell)}
                        onCommit={commit}
                        onCommitValue={(next) => void commitValue(sourceIndex, colIndex, next)}
                      />
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </div>

      {paging && <PagingBar paging={paging} rows={result.rows.length} />}

      <GuaranteesPanel
        open={showGuarantees}
        onClose={() => setShowGuarantees(false)}
        guarantees={guarantees}
        readOnly={result.editable ? undefined : readOnlyDetail}
      />
    </div>
  );
}

/**
 * Where this page sits, and how to move.
 *
 * No total is shown, because there is not one to show: counting the rows a
 * statement would return means running it to the end, which on a large table is
 * the very thing paging exists to avoid. What can be known honestly is where
 * this page starts, how many came back, and whether a full page came back —
 * which is the only sound way to tell there may be more.
 */
function PagingBar({ paging, rows }: { paging: PagingProps; rows: number }) {
  const { offset, limit, ordered, busy, onGoTo, onPageSize, orderableBy, onOrderBy } = paging;
  const first = rows === 0 ? 0 : offset + 1;
  const last = offset + rows;
  // A short page is the end of the result. A full one only *might* have more,
  // and saying "might" is the accurate version.
  const maybeMore = rows > 0 && rows >= limit;

  return (
    <div className="flex h-7 shrink-0 items-center gap-2 border-t border-border bg-surface-1 px-2 text-[11px]">
      <button
        onClick={() => onGoTo(0)}
        disabled={offset === 0 || busy}
        className="rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text disabled:opacity-30 disabled:hover:bg-transparent"
        title="First page"
      >
        ⇤
      </button>
      <button
        onClick={() => onGoTo(Math.max(0, offset - limit))}
        disabled={offset === 0 || busy}
        className="rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text disabled:opacity-30 disabled:hover:bg-transparent"
      >
        ← Previous
      </button>

      <span className="tabular-nums text-text-muted">
        {rows === 0 ? "No rows" : `Rows ${first.toLocaleString()}–${last.toLocaleString()}`}
      </span>

      <button
        onClick={() => onGoTo(offset + limit)}
        disabled={!maybeMore || busy}
        className="rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text disabled:opacity-30 disabled:hover:bg-transparent"
      >
        Next →
      </button>

      {!ordered && maybeMore && (
        // Worth saying every time the page can move: no engine here promises a
        // stable row order for an unordered query, so page two can repeat rows
        // from page one and skip others, and nothing in the result says so.
        <span className="flex items-center gap-1">
          <span
            className="rounded bg-warn/15 px-1.5 py-0.5 text-warn"
            title="Without an ORDER BY the server may return rows in a different order each time, so pages can overlap or miss rows."
          >
            unordered — pages may overlap
          </span>
          {onOrderBy && orderableBy && orderableBy.length > 0 && (
            <button
              onClick={onOrderBy}
              disabled={busy}
              className="rounded px-1.5 py-0.5 text-accent hover:bg-accent/15 disabled:opacity-40"
              title={`Add ORDER BY ${orderableBy.join(", ")} and run again`}
            >
              Order by {orderableBy.join(", ")}
            </button>
          )}
        </span>
      )}

      <div className="flex-1" />

      <label className="flex items-center gap-1 text-text-muted">
        Page size
        <select
          value={limit}
          onChange={(e) => onPageSize(Number(e.target.value))}
          disabled={busy}
          className="h-5 rounded border border-border bg-surface-0 px-1 text-[11px] outline-none focus:border-accent"
        >
          {PAGE_SIZES.map((size) => (
            <option key={size} value={size}>
              {size.toLocaleString()}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

function GridToolbar({
  result,
  filter,
  onFilter,
  visible,
  columnFilterCount,
  onClearColumnFilters,
  selectedCount,
  onClearSelection,
  onExportSelected,
  onExplain,
  onChart,
  onInsertRow,
  onDeleteSelected,
}: {
  result: ResultSet;
  filter: string;
  onFilter: (value: string) => void;
  visible: number;
  columnFilterCount: number;
  onClearColumnFilters: () => void;
  selectedCount: number;
  onClearSelection: () => void;
  onExportSelected?: (() => void) | undefined;
  onExplain: () => void;
  onChart: () => void;
  onInsertRow?: (() => void) | undefined;
  onDeleteSelected?: (() => void) | undefined;
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
        {filter || columnFilterCount > 0
          ? `${visible} of ${result.rows.length}`
          : `${result.rows.length}`}{" "}
        rows
      </span>

      {selectedCount > 0 && (
        <span className="flex items-center gap-1.5">
          <span className="rounded bg-accent/20 px-1.5 py-0.5 font-medium text-accent">
            {selectedCount} selected
          </span>
          {onExportSelected && (
            <button
              onClick={onExportSelected}
              className="rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text"
            >
              Export…
            </button>
          )}
          {onDeleteSelected && (
            <button
              onClick={onDeleteSelected}
              className="rounded px-1.5 py-0.5 text-text-muted hover:bg-danger/10 hover:text-danger"
            >
              Delete…
            </button>
          )}
          <button
            onClick={onClearSelection}
            className="rounded px-1 py-0.5 text-text-muted hover:text-text"
            title="Clear the selection"
          >
            ✕
          </button>
        </span>
      )}

      {/* A filter typed into a narrow column is easy to lose track of; saying
          how many are on, with one click to clear them, is the antidote. */}
      {columnFilterCount > 0 && (
        <button
          onClick={onClearColumnFilters}
          className="rounded bg-accent/15 px-1.5 py-0.5 text-accent hover:bg-accent/25"
        >
          {columnFilterCount} column filter{columnFilterCount === 1 ? "" : "s"} · clear
        </button>
      )}

      {/* Being explicit that a capped page is not the whole table. Presenting a
          partial result as complete is the kind of thing that misleads someone
          into a wrong conclusion about their data. */}
      {result.truncated && (
        <span className="rounded bg-warn/15 px-1.5 py-0.5 text-warn">
          Showing the first {result.rows.length} rows — more exist
        </span>
      )}

      {/* Sorting or filtering a truncated page covers only what was fetched,
          which is not the same as the top N of the table. Say so rather than
          implying otherwise. */}
      {result.truncated && (
        <span className="text-text-muted/70">Sort and filter apply to loaded rows only</span>
      )}

      <div className="flex-1" />

      {onInsertRow && (
        <button
          onClick={onInsertRow}
          className="rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text"
          title="Add a row to this table"
        >
          + Row
        </button>
      )}

      <button
        onClick={onChart}
        className="rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text"
        title="Chart these rows"
      >
        Chart
      </button>

      <button
        onClick={onExplain}
        className="rounded px-1.5 py-0.5 text-text-muted underline decoration-dotted underline-offset-2 hover:bg-surface-3 hover:text-text"
        title="What is guaranteed about these rows"
      >
        {result.editable ? "Double-click to edit" : "Read-only"}
      </button>
    </div>
  );
}

function HeaderCell({
  column,
  width,
  sort,
  isKey,
  precision,
  onSort,
}: {
  column: Column;
  width: number;
  sort: SortDirection | null;
  isKey: boolean;
  /** How this column's numbers survived the trip — see `precisionOf`. */
  precision: Precision;
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
          {/* Only "exact" gets a mark. An approximate column is the ordinary
              case and badging every one of them would be noise; the panel
              names them when it matters. */}
          {precision === "exact" && (
            <span
              aria-label="Exact — carried without rounding"
              title="Carried as text from the server, digit for digit. Nothing on the path converts this column to a floating-point number."
              className="text-ok"
            >
              ≡
            </span>
          )}
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
  nullable,
  editing,
  saving,
  draft,
  onDraft,
  onBegin,
  onCommit,
  onCommitValue,
}: {
  value: Value;
  width: number;
  editable: boolean;
  /** Whether the column accepts NULL, so the boolean list can offer it. */
  nullable: boolean;
  editing: boolean;
  saving: boolean;
  draft: string;
  onDraft: (value: string) => void;
  onBegin: () => void;
  onCommit: () => void;
  /** Commit a value directly, for controls where choosing is the edit. */
  onCommitValue: (next: Value) => void;
}) {
  if (editing) {
    const kind = editorFor(value);

    // JSON and long text are edited in a panel, which renders above the grid;
    // the cell keeps its place underneath so the row does not jump.
    if (kind === "json" || kind === "text") {
      return (
        <div
          style={{ width }}
          className="shrink-0 truncate border border-accent px-2 font-mono text-[length:var(--text-data)] leading-[var(--row-height)]"
        >
          {formatValue(value)}
        </div>
      );
    }

    return (
      <div style={{ width }} className="shrink-0 border-r border-border p-0">
        {kind === "bool" ? (
          <BoolEditor
            value={value}
            nullable={nullable}
            saving={saving}
            onChoose={(choice) => {
              onDraft(choice === "null" ? "" : choice);
              // Chosen from three options, so there is nothing to review: the
              // choice is the edit.
              onCommitValue(
                choice === "null" ? { kind: "null" } : { kind: "bool", value: choice === "true" },
              );
            }}
          />
        ) : (
          <InlineEditor draft={draft} saving={saving} onDraft={onDraft} onCommit={onCommit} />
        )}
      </div>
    );
  }

  return (
    <div
      style={{ width }}
      onDoubleClick={editable || value.kind === "bytes" ? onBegin : undefined}
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
