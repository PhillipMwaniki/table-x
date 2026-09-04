/**
 * Virtualized result grid.
 *
 * Only the visible rows are in the DOM, so a 100k-row page scrolls at the same
 * cost as a 20-row one. Columns are not virtualized: result sets are wide in
 * pathological cases but almost never wide enough to justify the complexity, and
 * horizontal virtualization breaks native column-drag selection.
 */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  cellClass,
  compareDecimalText,
  editText,
  formatValue,
  isInlineEditable,
  isNumeric,
  parseEdit,
  unchanged,
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
import { RowDetails } from "./RowDetails";
import { ContextMenu } from "../ui/ContextMenu";
import type { MenuItem } from "../ui/ContextMenu";
import { quickFilter, rowsForMenu, sourceTable } from "@/lib/rowcopy";
import { useSettings } from "@/store/settings";
import { useLayouts } from "@/store/layouts";
import {
  arrange,
  clampWidth,
  clearWidth,
  columnKeys,
  EMPTY_LAYOUT,
  freeze,
  frozenOffsets,
  hideColumn,
  isEmptyLayout,
  moveColumn,
  setWidth,
  showAll,
  showColumn,
  widthFor,
} from "@/lib/columns";
import type { ColumnLayout } from "@/lib/columns";
import type { QuickFilter } from "@/lib/rowcopy";
import type { Column, ExportFormat, ResultSet, Value } from "@/lib/types";

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

/**
 * What a grid remembers about a result while it is off screen.
 *
 * Only the active tab has a grid, so switching tabs unmounts it, and a fresh
 * mount starts at the top with nothing sorted or filtered. Coming back to find
 * the rows you had scrolled to gone from under you makes switching tabs cost
 * more than it should. What is kept is display state — where you were and how
 * you had arranged the rows — not the rows themselves, which the tab holds.
 */
interface GridMemory {
  sort: Sort | null;
  filter: string;
  columnFilters: Record<number, string>;
  scrollTop: number;
  scrollLeft: number;
}

/** Remembered grids, keyed by the caller's `memoryKey`, oldest first. */
const memories = new Map<string, GridMemory>();

/**
 * Bounded so a long session does not accumulate a memory for every result it
 * ever showed. Closed tabs are not told apart from open ones — the key is
 * opaque here — so the oldest go first, which is close enough.
 */
const MEMORY_LIMIT = 200;

function remember(key: string, changes: Partial<GridMemory>) {
  const was: GridMemory = memories.get(key) ?? {
    sort: null,
    filter: "",
    columnFilters: {},
    scrollTop: 0,
    scrollLeft: 0,
  };
  // Re-inserting moves the key to the end, which is what keeps the eviction
  // order meaningful: what was touched last is what is wanted next.
  memories.delete(key);
  memories.set(key, { ...was, ...changes });
  if (memories.size > MEMORY_LIMIT) {
    const oldest = memories.keys().next().value;
    if (oldest !== undefined) memories.delete(oldest);
  }
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
  memoryKey,
  layoutKey,
  onEdit,
  paging,
  onExportRows,
  readOnlyDetail,
  onInsertRow,
  onDeleteRows,
  onCopyRows,
  onDuplicateRow,
}: {
  result: ResultSet;
  /**
   * What to remember this grid's display state under while it is unmounted,
   * and to restore it from when a grid with the same key mounts again. Absent
   * where there is nothing to come back to.
   */
  memoryKey?: string | undefined;
  /**
   * What to keep this grid's column arrangement under between runs.
   *
   * Distinct from `memoryKey`, which is per tab and lasts a session: the way
   * you like a table's columns is about the table, and should be there when
   * it is opened again tomorrow from any tab. Absent, the arrangement lasts
   * as long as the grid does.
   */
  layoutKey?: string | undefined;
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
  /**
   * Put rows on the clipboard in one of the export formats.
   *
   * The grid decides which rows and which format; turning them into text is the
   * backend's job, because an INSERT has to be quoted for the engine it will be
   * run against.
   */
  onCopyRows?:
    | ((request: {
        rows: Value[][];
        format: ExportFormat;
        /** Named in generated INSERT statements. */
        table: string;
        /** Whether the delimited formats name their columns first. */
        header: boolean;
      }) => void)
    | undefined;
  /**
   * Open an insert form filled in from this row.
   *
   * Absent where the result is not a single writable table — the same condition
   * that decides whether a row can be added at all.
   */
  onDuplicateRow?: ((row: Value[]) => void) | undefined;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  // Row height and column widths are both measured in characters, so they have
  // to be recomputed when the data font size changes rather than read from CSS.
  const fontSize = useSettings((s) => s.dataFontSize);
  const striped = useSettings((s) => s.stripedRows);
  // A preference rather than tab state: see `Settings.rowDetails`.
  const detailsOpen = useSettings((s) => s.rowDetails);
  const setDetailsOpen = useSettings((s) => s.setRowDetails);
  const rowHeight = rowHeightFor(fontSize);
  // Read once, on mount: this is the state the grid had when it was last
  // unmounted, and everything after is this instance's own.
  const remembered = useRef(memoryKey ? memories.get(memoryKey) : undefined);
  const [sort, setSort] = useState<Sort | null>(remembered.current?.sort ?? null);
  const [filter, setFilter] = useState(remembered.current?.filter ?? "");
  /** Per-column expressions, keyed by column index. */
  const [columnFilters, setColumnFilters] = useState<Record<number, string>>(
    remembered.current?.columnFilters ?? {},
  );

  useEffect(() => {
    if (memoryKey) remember(memoryKey, { sort, filter, columnFilters });
  }, [memoryKey, sort, filter, columnFilters]);

  // The column arrangement: stored under the layout key when there is one,
  // otherwise held here and gone with the grid.
  const stored = useLayouts((s) => (layoutKey ? s.layouts[layoutKey] : undefined));
  const setStored = useLayouts((s) => s.set);
  const [local, setLocal] = useState<ColumnLayout>(EMPTY_LAYOUT);
  const layout = (layoutKey ? stored : local) ?? EMPTY_LAYOUT;
  const updateLayout = useCallback(
    (next: (was: ColumnLayout) => ColumnLayout) => {
      if (layoutKey) setStored(layoutKey, next);
      else setLocal(next);
    },
    [layoutKey, setStored],
  );
  const keys = useMemo(() => columnKeys(result.columns), [result.columns]);
  /** Source indexes of the columns on screen, left to right. */
  const visible = useMemo(() => arrange(keys, layout), [keys, layout]);
  /** A width being dragged, before it is committed to the layout. */
  const [resizing, setResizing] = useState<{ col: number; width: number } | null>(null);
  /** A header being dragged, and the slot it would drop into. */
  const [reorder, setReorder] = useState<{ from: number; slot: number } | null>(null);
  const [headerMenu, setHeaderMenu] = useState<{ x: number; y: number; col: number } | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const headerRow = useRef<HTMLDivElement>(null);

  // Before paint, so the rows do not flash at the top and then jump. The
  // virtualizer is told the same offset below, so it draws the right rows on
  // the first frame rather than the top ones and then correcting.
  useLayoutEffect(() => {
    const el = scroller.current;
    const was = remembered.current;
    if (!el || !was) return;
    el.scrollTop = was.scrollTop;
    el.scrollLeft = was.scrollLeft;
  }, []);
  const [editing, setEditing] = useState<{ row: number; col: number } | null>(null);
  /** A binary cell open for reading. Viewing is not editing. */
  const [viewing, setViewing] = useState<{ row: number; col: number } | null>(null);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [cellError, setCellError] = useState<string | null>(null);
  /** Source indices of the picked rows. */
  const [selected, setSelected] = useState<ReadonlySet<number>>(EMPTY_SELECTION);
  /**
   * The row the details panel is showing, by source index.
   *
   * Separate from the selection, which is a set and answers a different
   * question -- what to export or delete. This one is where the cursor is, and
   * there is only ever one of it.
   */
  const [current, setCurrent] = useState<number | null>(null);
  /** Where the last plain click landed, so shift-click has a range to extend. */
  const anchor = useRef<number | null>(null);
  /** Where a right-click landed, and on what. */
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    row: number;
    /** Null when the right-click landed on the row number rather than a cell. */
    col: number | null;
  } | null>(null);
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
    // The first row rather than nothing: a panel that opens empty and waits to
    // be told where to look is a panel that has to be explained.
    setCurrent(result.rows.length > 0 ? 0 : null);
    // A menu opened against row 4 of the previous result would act on row 4 of
    // this one, which is a different row.
    setMenu(null);
  }, [result]);

  // Column widths are measured from a sample of rows rather than every row:
  // scanning 100k rows to size a column is not worth the frame it costs.
  const measured = useMemo(() => {
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

  // What each column is drawn at: the width under the pointer while one is
  // being dragged, the chosen width when one was, and the measured one
  // otherwise. By source index, like everything else about the result.
  const widths = useMemo(
    () =>
      measured.map((m, i) =>
        resizing?.col === i ? resizing.width : widthFor(keys[i] ?? "", m, layout),
      ),
    [measured, keys, layout, resizing],
  );
  const visibleWidths = useMemo(
    () => visible.map((i) => widths[i] ?? MIN_COL_WIDTH),
    [visible, widths],
  );
  const offsets = useMemo(
    () => frozenOffsets(visibleWidths, layout.frozen, GUTTER_WIDTH),
    [visibleWidths, layout.frozen],
  );

  /** The slot a header dragged to `clientX` would drop into. */
  const slotAt = useCallback(
    (clientX: number) => {
      const row = headerRow.current;
      if (!row) return 0;
      const x = clientX - row.getBoundingClientRect().left - GUTTER_WIDTH;
      let left = 0;
      for (let d = 0; d < visibleWidths.length; d++) {
        const width = visibleWidths[d] ?? 0;
        if (x < left + width / 2) return d;
        left += width;
      }
      return visibleWidths.length;
    },
    [visibleWidths],
  );

  const dropReorder = useCallback(() => {
    if (!reorder) return;
    const { from, slot } = reorder;
    setReorder(null);
    // Dropping a column on either side of where it already is moves nothing.
    if (slot === from || slot === from + 1) return;
    const key = keys[visible[from] ?? -1];
    const before = slot < visible.length ? keys[visible[slot] ?? -1] : null;
    if (key === undefined || before === undefined) return;
    updateLayout((was) => moveColumn(keys, was, key, before));
  }, [reorder, keys, visible, updateLayout]);

  const hide = useCallback(
    (col: number) => {
      const key = keys[col];
      if (key === undefined) return;
      updateLayout((was) => hideColumn(was, key));
      // A filter on a column you cannot see is a filter you cannot account
      // for, so it goes with the column.
      setColumnFilters((was) => {
        if (!(col in was)) return was;
        const next = { ...was };
        delete next[col];
        return next;
      });
    },
    [keys, updateLayout],
  );

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
    initialOffset: remembered.current?.scrollTop ?? 0,
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

      // Picking rows and moving the cursor are the same gesture: the row you
      // just clicked is the one you want to look at.
      setCurrent(entry.index);

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
    if (unchanged(next, original)) {
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
      if (unchanged(next, original)) {
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

  const totalWidth = visibleWidths.reduce((sum, w) => sum + w, 0);

  /**
   * What a right-click on a cell offers.
   *
   * Two subjects in the order the pointer implies: the value under it first,
   * then the row it sits in. The row items are the export formats, because a
   * row copied to the clipboard is wanted for the same reasons a file is —
   * pasted into a ticket, a script, or another database — and because they are
   * already written and already correct.
   */
  const menuItems = useMemo((): MenuItem[] => {
    if (!menu) return [];

    const items: MenuItem[] = [];
    // Absent when the click landed on the row number, which points at a row and
    // at no value in particular.
    const col = menu.col;
    const value = col === null ? undefined : result.rows[menu.row]?.[col];
    if (value) {
      items.push({
        label: "Copy value",
        onSelect: () => void navigator.clipboard?.writeText(formatValue(value)),
      });
    }

    const indexes = rowsForMenu(menu.row, selected);
    const rows = indexes.map((i) => result.rows[i]).filter((row): row is Value[] => Boolean(row));
    const what = rows.length === 1 ? "row" : `${rows.length} rows`;
    const table = sourceTable(result.columns);

    if (onCopyRows && rows.length > 0) {
      const copy = (format: ExportFormat, header: boolean) => () =>
        onCopyRows({ rows, format, header, table: table ?? "rows" });

      items.push({
        label: `Copy ${what} as`,
        items: [
          // Plain text is tab-separated with no header, which is the same bytes
          // a headerless TSV would be. Two items producing one result would be
          // two items to choose between for no reason, so it is named the way
          // people look for it and the TSV entry is the one with the header.
          { label: "Plain text", onSelect: copy("tsv", false) },
          { label: "TSV with header", onSelect: copy("tsv", true) },
          { label: "CSV", onSelect: copy("csv", false) },
          { label: "CSV with header", onSelect: copy("csv", true) },
          // The rest carry their column names inside the format itself: a
          // Markdown table without a header row is not a table, JSON's keys
          // are the names, and an INSERT lists them in the statement.
          { label: "Markdown table", separated: true, onSelect: copy("markdown", true) },
          { label: "JSON", onSelect: copy("json", true) },
          {
            label: "INSERT statement",
            // Offered greyed rather than hidden: the reason it cannot be built
            // is worth knowing, and a menu whose items move around depending on
            // the query is a menu you have to read every time.
            disabledReason: table
              ? undefined
              : "These rows do not come from one table, so there is no table to insert them into",
            onSelect: table ? copy("sql", true) : undefined,
          },
        ],
      });
    }

    // Editing, for the cell that was actually pointed at. A menu that set forty
    // cells because forty rows were selected would be a menu that needs an undo
    // per cell.
    if (value && col !== null) {
      const column = result.columns[col];
      const why = result.editable
        ? undefined
        : (readOnlyDetail?.reason ?? "This result is read-only");
      items.push({
        label: "Set value",
        separated: true,
        disabledReason: why,
        items: [
          {
            label: "NULL",
            disabledReason:
              why ?? (column?.nullable === false ? "This column does not accept NULL" : undefined),
            onSelect: () => void commitValue(menu.row, col, { kind: "null" }),
          },
          {
            // The one edit the grid cannot express: clearing a text cell there
            // means the empty string, and there is no way to type NULL.
            label: "Empty text",
            disabledReason: why,
            onSelect: () => void commitValue(menu.row, col, { kind: "text", value: "" }),
          },
        ],
      });
    }

    if (onDuplicateRow) {
      const row = result.rows[menu.row];
      items.push({
        label: "Duplicate row…",
        separated: !value,
        disabledReason: table ? undefined : "These rows do not come from one table",
        // A form rather than a straight insert: the copy usually collides with
        // the original on a key, and the place to settle that is before the
        // statement runs rather than in the error it returns.
        onSelect: row && table ? () => onDuplicateRow(row) : undefined,
      });
    }

    if (value && col !== null) {
      const text = formatValue(value);
      const brief = text.length > 24 ? `${text.slice(0, 24)}…` : text;
      const isNull = value.kind === "null";
      const setFilter = (kind: QuickFilter) => () =>
        setColumnFilters((was) => ({ ...was, [col]: quickFilter(kind, text) }));

      items.push({
        label: "Quick filter",
        separated: true,
        items: [
          // Against NULL, "equals" and "contains" are questions with no useful
          // answer — every row that has a value fails both — so a NULL cell is
          // offered the two tests that do apply.
          ...(isNull
            ? []
            : [
                { label: `Equals ${brief}`, onSelect: setFilter("equals") },
                { label: `Not ${brief}`, onSelect: setFilter("not") },
                { label: `Contains ${brief}`, onSelect: setFilter("contains") },
              ]),
          { label: "Is NULL", separated: !isNull, onSelect: setFilter("null") },
          { label: "Is not NULL", onSelect: setFilter("notNull") },
          ...(columnFilters[col]
            ? [
                {
                  label: "Clear this column's filter",
                  separated: true,
                  onSelect: () =>
                    setColumnFilters((was) => {
                      const next = { ...was };
                      delete next[col];
                      return next;
                    }),
                },
              ]
            : []),
        ],
      });
    }

    return items;
  }, [
    menu,
    result.rows,
    result.columns,
    result.editable,
    readOnlyDetail,
    selected,
    columnFilters,
    commitValue,
    onCopyRows,
    onDuplicateRow,
  ]);

  /**
   * The row the panel is showing.
   *
   * Guarded rather than indexed blindly: a delete or a refetch can leave the
   * cursor pointing past the end of a shorter result for one render.
   */
  const currentRow = current === null ? null : (result.rows[current] ?? null);

  // The value under an open panel or viewer, if any.
  const panelValue = editing ? result.rows[editing.row]?.[editing.col] : undefined;
  const panelKind = panelValue ? editorFor(panelValue) : null;
  const viewingValue = viewing ? result.rows[viewing.row]?.[viewing.col] : undefined;

  /**
   * What a right-click on a column name offers.
   *
   * The things that are about the column rather than its values: how it is
   * sorted, how wide it is, whether it is shown, and whether it stays put.
   */
  const headerMenuItems = useMemo((): MenuItem[] => {
    if (!headerMenu) return [];
    const col = headerMenu.col;
    const key = keys[col];
    const column = result.columns[col];
    if (key === undefined || !column) return [];
    const at = visible.indexOf(col);
    const isFrozen = at !== -1 && at < layout.frozen;

    return [
      {
        label: "Sort ascending",
        onSelect: () => setSort({ columnIndex: col, direction: "asc" }),
      },
      {
        label: "Sort descending",
        onSelect: () => setSort({ columnIndex: col, direction: "desc" }),
      },
      {
        label: "Clear sort",
        disabledReason: sort?.columnIndex === col ? undefined : "Not sorted by this column",
        onSelect: () => setSort(null),
      },
      {
        label: "Reset width",
        separated: true,
        disabledReason:
          layout.widths[key] === undefined ? "Already at its measured width" : undefined,
        onSelect: () => updateLayout((was) => clearWidth(was, key)),
      },
      {
        label: isFrozen ? "Unfreeze columns" : `Freeze up to ${column.name}`,
        onSelect: () => updateLayout((was) => freeze(was, isFrozen ? 0 : at + 1)),
      },
      { label: "Hide column", onSelect: () => hide(col) },
      {
        label: "Show all columns",
        separated: true,
        disabledReason: layout.hidden.length === 0 ? "Nothing is hidden" : undefined,
        onSelect: () => updateLayout(showAll),
      },
      {
        label: "Reset column layout",
        disabledReason: isEmptyLayout(layout) ? "Nothing has been changed" : undefined,
        onSelect: () => updateLayout(() => EMPTY_LAYOUT),
      },
    ];
  }, [headerMenu, keys, result.columns, visible, layout, sort, updateLayout, hide]);

  if (charting) {
    return <ChartView result={result} onClose={() => setCharting(false)} />;
  }

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
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
        detailsOpen={detailsOpen}
        onToggleDetails={() => setDetailsOpen(!detailsOpen)}
        hiddenCount={layout.hidden.length}
        pickerOpen={pickerOpen}
        onTogglePicker={() => setPickerOpen((was) => !was)}
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

      {pickerOpen && (
        <ColumnPicker
          columns={result.columns}
          keys={keys}
          hidden={layout.hidden}
          onToggle={(col, shown) => {
            if (shown) {
              const key = keys[col];
              if (key !== undefined) updateLayout((was) => showColumn(was, key));
            } else hide(col);
          }}
          onShowAll={() => updateLayout(showAll)}
          onClose={() => setPickerOpen(false)}
        />
      )}

      {cellError && (
        <div role="alert" className="shrink-0 bg-danger/10 px-2 py-1 text-[11px] text-danger">
          {cellError}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <div
          ref={scroller}
          className="min-h-0 flex-1 overflow-auto"
          onScroll={(e) => {
            // A map write per scroll event, not a render: nothing subscribes
            // to the memory, it is only read by the next mount.
            if (memoryKey) {
              remember(memoryKey, {
                scrollTop: e.currentTarget.scrollTop,
                scrollLeft: e.currentTarget.scrollLeft,
              });
            }
          }}
        >
          <div style={{ width: totalWidth, minWidth: "100%" }}>
            {/* Header stays put while the body scrolls under it. */}
            <div
              ref={headerRow}
              className="sticky top-0 z-10 flex border-b border-border bg-surface-2"
            >
              {/* Where a dragged column would land: a line at the boundary
                  nearest the pointer, in content coordinates, so it scrolls
                  with the columns it sits between. */}
              {reorder && (
                <div
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-y-0 z-30 w-0.5 bg-accent"
                  style={{
                    left:
                      GUTTER_WIDTH +
                      visibleWidths.slice(0, reorder.slot).reduce((sum, w) => sum + w, 0) -
                      1,
                  }}
                />
              )}
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
              {visible.map((i, d) => {
                const col = result.columns[i];
                if (!col) return null;
                const left = offsets[d] ?? null;
                return (
                  <HeaderCell
                    key={keys[i]}
                    column={col}
                    width={widths[i] ?? MIN_COL_WIDTH}
                    sort={sort?.columnIndex === i ? sort.direction : null}
                    isKey={result.key_columns.includes(col.name)}
                    precision={guarantees.columns[i]?.precision ?? "none"}
                    frozen={left === null ? undefined : { left, last: d === layout.frozen - 1 }}
                    dragging={reorder?.from === d}
                    onSort={() =>
                      setSort((s) =>
                        s?.columnIndex === i && s.direction === "asc"
                          ? { columnIndex: i, direction: "desc" }
                          : s?.columnIndex === i && s.direction === "desc"
                            ? null
                            : { columnIndex: i, direction: "asc" },
                      )
                    }
                    onResize={(width) => setResizing({ col: i, width })}
                    onResizeEnd={() => {
                      setResizing((was) => {
                        const key = keys[i];
                        if (was && key !== undefined)
                          updateLayout((layout) => setWidth(layout, key, was.width));
                        return null;
                      });
                    }}
                    onFit={() => {
                      const key = keys[i];
                      if (key !== undefined) updateLayout((was) => clearWidth(was, key));
                    }}
                    onDrag={(clientX) => setReorder({ from: d, slot: slotAt(clientX) })}
                    onDrop={dropReorder}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setHeaderMenu({ x: e.clientX, y: e.clientY, col: i });
                    }}
                  />
                );
              })}
            </div>

            {/* Filter row, directly under the names it filters — the association
              is positional, so it needs no labels of its own. */}
            <div className="sticky top-[var(--header-height,2.6rem)] z-10 flex border-b border-border bg-surface-1">
              <div
                style={{ width: GUTTER_WIDTH }}
                className="sticky left-0 z-20 shrink-0 border-r border-border bg-surface-1"
              />
              {visible.map((i, d) => {
                const col = result.columns[i];
                if (!col) return null;
                const left = offsets[d] ?? null;
                return (
                  <div
                    key={`filter-${keys[i]}`}
                    style={{ width: widths[i] ?? MIN_COL_WIDTH, left: left ?? undefined }}
                    className={cx(
                      "shrink-0 border-r border-border p-0.5",
                      left !== null && "sticky z-[15] bg-surface-1",
                      left !== null && d === layout.frozen - 1 && "border-r-2",
                    )}
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
                );
              })}
            </div>

            <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
              {virtualizer.getVirtualItems().map((virtual) => {
                const entry = view[virtual.index];
                if (!entry) return null;
                const { row, index: sourceIndex } = entry;
                const isSelected = selected.has(sourceIndex);
                // Only marked while the panel is open. A cursor with nothing
                // reading it is a highlight the user cannot account for.
                const isCurrent = detailsOpen && sourceIndex === current;
                // Banded on the row's position in the view, not its index in the
                // result: under a filter or a sort those differ, and banding by
                // the source index would put two of the same shade side by side —
                // which is the one thing banding exists to prevent.
                const banded = striped && virtual.index % 2 === 1;
                // A frozen cell paints its own ground, or the cells scrolling
                // under it would show through; this is the row's tint, for it
                // to paint on top of that ground so the two match.
                const tint = isSelected
                  ? "before:bg-accent/15"
                  : cx(banded && "before:bg-surface-1", "group-hover:before:bg-surface-2");
                return (
                  <div
                    key={virtual.key}
                    // A click anywhere in the row moves the cursor, cells
                    // included -- a cell opens for editing on the second click,
                    // so the first is free to mean "look at this row".
                    onMouseDown={() => setCurrent(sourceIndex)}
                    className={cx(
                      "group absolute flex border-b border-border/40",
                      // Hover is a step above the band rather than equal to it, or
                      // it would be invisible on every other row.
                      isSelected
                        ? "bg-accent/15"
                        : cx(banded && "bg-surface-1", "hover:bg-surface-2"),
                      // Outlined rather than filled, so it reads as a cursor
                      // beside a selection rather than as a paler selection.
                      isCurrent && "ring-1 ring-accent/50 ring-inset",
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
                      onContextMenu={(e) => {
                        e.preventDefault();
                        setCurrent(sourceIndex);
                        setMenu({ x: e.clientX, y: e.clientY, row: sourceIndex, col: null });
                      }}
                      className={cx(
                        "sticky left-0 z-10 shrink-0 cursor-pointer border-r border-border select-none",
                        "text-right font-mono text-[10px] leading-[var(--row-height)] tabular-nums",
                        isSelected
                          ? "bg-accent/25 text-text"
                          : cx(
                              banded ? "bg-surface-2" : "bg-surface-1",
                              "text-text-muted/60 hover:text-text",
                            ),
                      )}
                      // The number is the row's place in the page, not its id —
                      // and with an offset it continues from where the last page
                      // ended rather than restarting at one.
                      title={`Row ${(paging?.offset ?? 0) + sourceIndex + 1}`}
                    >
                      <span className="px-1">{(paging?.offset ?? 0) + sourceIndex + 1}</span>
                    </div>

                    {visible.map((colIndex, d) => {
                      const cell = row[colIndex];
                      if (!cell) return null;
                      const isEditing = editing?.row === sourceIndex && editing.col === colIndex;
                      const left = offsets[d] ?? null;
                      return (
                        <Cell
                          key={keys[colIndex]}
                          value={cell}
                          width={widths[colIndex] ?? MIN_COL_WIDTH}
                          frozen={
                            left === null
                              ? undefined
                              : { left, last: d === layout.frozen - 1, tint }
                          }
                          editable={result.editable}
                          nullable={result.columns[colIndex]?.nullable !== false}
                          editing={isEditing}
                          saving={isEditing && saving}
                          draft={draft}
                          onDraft={setDraft}
                          onBegin={() => beginEdit(sourceIndex, colIndex, cell)}
                          onCommit={commit}
                          onCommitValue={(next) => void commitValue(sourceIndex, colIndex, next)}
                          onContextMenu={(e) => {
                            e.preventDefault();
                            // The cursor follows the pointer, so the panel and
                            // the menu are about the same row.
                            setCurrent(sourceIndex);
                            setMenu({
                              x: e.clientX,
                              y: e.clientY,
                              row: sourceIndex,
                              col: colIndex,
                            });
                          }}
                        />
                      );
                    })}
                  </div>
                );
              })}
            </div>
          </div>
        </div>

        {detailsOpen && (
          <RowDetails
            columns={result.columns}
            row={currentRow}
            // The gutter's number, so the panel and the row it came from agree
            // even on the second page.
            rowNumber={current === null ? null : (paging?.offset ?? 0) + current + 1}
            editable={result.editable}
            onEdit={async (colIndex, next) => {
              if (current === null) return;
              // Straight to the grid's own edit path, rejection included: the
              // panel shows the failure against the field that caused it rather
              // than in the strip above the grid.
              await onEdit(current, colIndex, next);
            }}
            onClose={() => setDetailsOpen(false)}
          />
        )}
      </div>

      {paging && <PagingBar paging={paging} rows={result.rows.length} />}

      {menu && menuItems.length > 0 && (
        <ContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />
      )}

      {headerMenu && headerMenuItems.length > 0 && (
        <ContextMenu
          x={headerMenu.x}
          y={headerMenu.y}
          items={headerMenuItems}
          onClose={() => setHeaderMenu(null)}
        />
      )}

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
  detailsOpen,
  onToggleDetails,
  hiddenCount,
  pickerOpen,
  onTogglePicker,
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
  /** Whether the row panel is showing, and how to change that. */
  detailsOpen: boolean;
  onToggleDetails: () => void;
  /** Columns taken off screen, said out loud so they are not lost. */
  hiddenCount: number;
  pickerOpen: boolean;
  onTogglePicker: () => void;
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

      {/* The count is the point: a hidden column leaves no gap behind, so
          without it the only sign one was hidden would be its absence. */}
      <button
        onClick={onTogglePicker}
        aria-expanded={pickerOpen}
        className={cx(
          "rounded px-1.5 py-0.5 text-text-muted hover:bg-surface-3 hover:text-text",
          pickerOpen && "bg-surface-3 text-text",
          hiddenCount > 0 && "text-accent",
        )}
        title="Choose which columns are shown"
      >
        Columns{hiddenCount > 0 && ` · ${hiddenCount} hidden`}
      </button>

      {/* The connections pane's control, mirrored: the same window outline with
          the divider on the right instead of the left. Filled while the panel
          is hidden, which is the convention that button already set — the
          button says which state you are in and not only what it would do.
          Drawn rather than typed for the same reason: no font we can rely on
          has a glyph that means "panel". */}
      <button
        onClick={onToggleDetails}
        title={detailsOpen ? "Hide row details" : "Show row details"}
        aria-label={detailsOpen ? "Hide row details" : "Show row details"}
        aria-pressed={detailsOpen}
        className="flex size-5 items-center justify-center rounded text-text-muted hover:bg-surface-3 hover:text-text"
      >
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" />
          <line x1="10" y1="2.5" x2="10" y2="13.5" stroke="currentColor" />
          {!detailsOpen && <rect x="10" y="3" width="4" height="10" fill="currentColor" />}
        </svg>
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

/**
 * How far a header has to move before it is a drag rather than a click.
 *
 * A pointer never lands perfectly still; below this the movement is the
 * hand, not an intention.
 */
const DRAG_THRESHOLD = 5;

/** Where a frozen cell sits, and whether it is the last one before the rest scroll. */
interface Frozen {
  left: number;
  last: boolean;
}

function HeaderCell({
  column,
  width,
  sort,
  isKey,
  precision,
  frozen,
  dragging,
  onSort,
  onResize,
  onResizeEnd,
  onFit,
  onDrag,
  onDrop,
  onContextMenu,
}: {
  column: Column;
  width: number;
  sort: SortDirection | null;
  isKey: boolean;
  /** How this column's numbers survived the trip — see `precisionOf`. */
  precision: Precision;
  frozen?: Frozen | undefined;
  /** Whether this is the header being dragged to a new place. */
  dragging: boolean;
  onSort: () => void;
  /** The width under the pointer, as it moves. */
  onResize: (width: number) => void;
  onResizeEnd: () => void;
  /** Back to the measured width. */
  onFit: () => void;
  /** The pointer's position while the header is being dragged. */
  onDrag: (clientX: number) => void;
  onDrop: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  /** Where a resize began, and the width it began from. */
  const resize = useRef<{ x: number; width: number } | null>(null);
  /** Where a press on the name landed, and whether it has moved far enough to be a drag. */
  const press = useRef<{ x: number; moved: boolean } | null>(null);
  /** Set by a drag's release so the click that follows it does not also sort. */
  const dragged = useRef(false);

  return (
    <div
      style={{ width, left: frozen?.left }}
      onContextMenu={onContextMenu}
      className={cx(
        "relative flex shrink-0 border-r border-border",
        frozen && "sticky z-[15] bg-surface-2",
        frozen?.last && "border-r-2",
        dragging && "opacity-50",
      )}
    >
      <button
        onClick={() => {
          if (dragged.current) {
            dragged.current = false;
            return;
          }
          onSort();
        }}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          press.current = { x: e.clientX, moved: false };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const p = press.current;
          if (!p) return;
          if (!p.moved) {
            if (Math.abs(e.clientX - p.x) < DRAG_THRESHOLD) return;
            p.moved = true;
          }
          onDrag(e.clientX);
        }}
        onPointerUp={() => {
          const p = press.current;
          press.current = null;
          if (p?.moved) {
            dragged.current = true;
            onDrop();
          }
        }}
        onPointerCancel={() => {
          if (press.current?.moved) onDrop();
          press.current = null;
        }}
        title={`${column.name} — ${column.type_name}${column.nullable === false ? " NOT NULL" : ""}`}
        className="flex min-w-0 flex-1 touch-none items-center gap-1 px-2 py-1 text-left hover:bg-surface-3"
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
      {/* The column's right edge. Dragged, it sets the width; double-clicked,
          it gives the width back to the measurement. Wider than the line it
          draws, because a two-pixel target is a target people miss. */}
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label={`Resize ${column.name}`}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          e.stopPropagation();
          e.preventDefault();
          resize.current = { x: e.clientX, width };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const r = resize.current;
          if (!r) return;
          onResize(clampWidth(r.width + e.clientX - r.x));
        }}
        onPointerUp={() => {
          if (!resize.current) return;
          resize.current = null;
          onResizeEnd();
        }}
        onPointerCancel={() => {
          if (!resize.current) return;
          resize.current = null;
          onResizeEnd();
        }}
        onDoubleClick={(e) => {
          e.stopPropagation();
          onFit();
        }}
        className="absolute inset-y-0 -right-1 z-10 w-2 cursor-col-resize touch-none hover:bg-accent/60"
      />
    </div>
  );
}

/**
 * Every column, shown or not, with a box beside each.
 *
 * The header menu can hide a column but cannot bring one back — there is no
 * header to right-click once it is gone — so this is the way back, and the
 * place to hide several at once without opening a menu per column.
 */
function ColumnPicker({
  columns,
  keys,
  hidden,
  onToggle,
  onShowAll,
  onClose,
}: {
  columns: Column[];
  keys: string[];
  hidden: string[];
  onToggle: (columnIndex: number, shown: boolean) => void;
  onShowAll: () => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [search, setSearch] = useState("");

  useEffect(() => {
    const closeIfOutside = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const closeOnEscape = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("pointerdown", closeIfOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeIfOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [onClose]);

  const needle = search.trim().toLowerCase();
  const shown = columns
    .map((column, i) => ({ column, i }))
    .filter(({ column }) => !needle || column.name.toLowerCase().includes(needle));

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label="Columns"
      className="absolute top-7 right-2 z-30 flex w-60 flex-col rounded-md border border-border bg-surface-1 shadow-lg"
    >
      <div className="flex items-center gap-1 border-b border-border p-1.5">
        <input
          autoFocus
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Find a column…"
          aria-label="Find a column"
          className="h-5 min-w-0 flex-1 rounded border border-border bg-surface-0 px-1.5 text-[11px] focus:border-accent focus:outline-none"
        />
        <button
          onClick={onShowAll}
          disabled={hidden.length === 0}
          className="rounded px-1.5 py-0.5 text-[11px] text-accent hover:bg-accent/15 disabled:opacity-40 disabled:hover:bg-transparent"
        >
          Show all
        </button>
      </div>
      <div className="max-h-72 overflow-y-auto p-1.5">
        {shown.length === 0 ? (
          <p className="px-1 py-2 text-[11px] text-text-muted">No column matches</p>
        ) : (
          shown.map(({ column, i }) => {
            const key = keys[i] ?? column.name;
            return (
              <label
                key={key}
                className="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 hover:bg-surface-2"
              >
                <input
                  type="checkbox"
                  checked={!hidden.includes(key)}
                  onChange={(e) => onToggle(i, e.target.checked)}
                  className="size-3.5 shrink-0 accent-[var(--color-accent)]"
                />
                <span className="truncate font-mono text-[11px] text-text">{column.name}</span>
                <span className="ml-auto shrink-0 font-mono text-[10px] text-text-muted">
                  {column.type_name}
                </span>
              </label>
            );
          })
        )}
      </div>
    </div>
  );
}

function Cell({
  value,
  width,
  frozen,
  editable,
  nullable,
  editing,
  saving,
  draft,
  onDraft,
  onBegin,
  onCommit,
  onCommitValue,
  onContextMenu,
}: {
  value: Value;
  width: number;
  /**
   * Where to stay while the row scrolls, with the row's tint to paint on the
   * ground this cell has to supply for itself.
   */
  frozen?: (Frozen & { tint: string }) | undefined;
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
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  // A frozen cell needs a ground of its own — otherwise the cells scrolling
  // under it show through — and then the row's tint over that ground, so it
  // reads as part of the row it is in. The tint is a pseudo-element behind
  // the text rather than a second background, because one element has one.
  const stay = frozen
    ? cx(
        "sticky isolate z-[5] bg-surface-0",
        "before:pointer-events-none before:absolute before:inset-0 before:-z-10 before:content-['']",
        frozen.tint,
        frozen.last && "border-r-2",
      )
    : undefined;
  const place = { width, left: frozen?.left };

  if (editing) {
    const kind = editorFor(value);

    // JSON and long text are edited in a panel, which renders above the grid;
    // the cell keeps its place underneath so the row does not jump.
    if (kind === "json" || kind === "text") {
      return (
        <div
          style={place}
          className={cx(
            "shrink-0 truncate border border-accent px-2 font-mono text-[length:var(--text-data)] leading-[var(--row-height)]",
            stay,
          )}
        >
          {formatValue(value)}
        </div>
      );
    }

    return (
      <div style={place} className={cx("shrink-0 border-r border-border p-0", stay)}>
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
      style={place}
      onDoubleClick={editable || value.kind === "bytes" ? onBegin : undefined}
      onContextMenu={onContextMenu}
      className={cx(
        "shrink-0 truncate border-r border-border px-2 font-mono",
        "text-[length:var(--text-data)] leading-[var(--row-height)]",
        isNumeric(value) && "text-right",
        cellClass(value),
        editable && "cursor-text",
        stay,
      )}
      title={value.kind === "bytes" ? undefined : formatValue(value)}
    >
      {formatValue(value)}
    </div>
  );
}
