/**
 * How a result's columns are arranged on screen: their widths, their order,
 * which are hidden, and how many stay put while the rest scroll.
 *
 * Kept apart from the result itself because it outlives it. The rows of a table
 * change every time it is opened; the fact that you dragged `notes` to the end
 * and narrowed `id` should not. So a layout is addressed by column *name* rather
 * than position, and applied to whatever result turns up under the same key.
 *
 * Pure functions over a plain record, so the arithmetic — where a frozen
 * column's left edge is, what order to draw in after a drag — is testable
 * without a grid.
 */

export interface ColumnLayout {
  /** Chosen widths in pixels, by column key. Absent means "measure it". */
  widths: Record<string, number>;
  /**
   * Column keys in the order they are shown.
   *
   * Partial on purpose: a query that gains a column should show it, at the end,
   * rather than lose it because the stored order predates it. Keys not listed
   * follow the listed ones in the order the result gave them.
   */
  order: string[];
  hidden: string[];
  /** How many of the leading *visible* columns stay put while the rest scroll. */
  frozen: number;
}

export const EMPTY_LAYOUT: ColumnLayout = Object.freeze({
  widths: {},
  order: [],
  hidden: [],
  frozen: 0,
}) as ColumnLayout;

/** Narrower than this and the header cannot show a name; wider is a mistake. */
export const MIN_COLUMN_WIDTH = 40;
export const MAX_COLUMN_WIDTH = 2000;

/**
 * A stable key per column.
 *
 * The name, except that a result can carry the same name twice — `a.id, b.id`
 * — and two columns sharing a key would share a width and swap places under a
 * drag. The second and later occurrences are numbered, so `id` and `id#2`
 * address different columns and a result with one `id` is unaffected.
 */
export function columnKeys(columns: { name: string }[]): string[] {
  const seen = new Map<string, number>();
  return columns.map((column) => {
    const n = (seen.get(column.name) ?? 0) + 1;
    seen.set(column.name, n);
    return n === 1 ? column.name : `${column.name}#${n}`;
  });
}

/** Every key in display order, hidden ones included. */
function fullOrder(keys: string[], layout: ColumnLayout): string[] {
  const known = new Set(keys);
  const listed = layout.order.filter((key) => known.has(key));
  const seen = new Set(listed);
  return [...listed, ...keys.filter((key) => !seen.has(key))];
}

/**
 * Source indexes of the columns to draw, in the order to draw them.
 *
 * Indexes rather than keys because everything else about a result — its rows,
 * its filters, its sort — is addressed by the position the driver gave the
 * column, and the layout only changes where that position is painted.
 */
export function arrange(keys: string[], layout: ColumnLayout): number[] {
  const hidden = new Set(layout.hidden);
  const at = new Map(keys.map((key, index) => [key, index]));
  return fullOrder(keys, layout)
    .filter((key) => !hidden.has(key))
    .map((key) => at.get(key)!);
}

export function clampWidth(width: number): number {
  if (!Number.isFinite(width)) return MIN_COLUMN_WIDTH;
  return Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, Math.round(width)));
}

/** Width to draw a column at: the chosen one if there is one, else the measured one. */
export function widthFor(key: string, measured: number, layout: ColumnLayout): number {
  const chosen = layout.widths[key];
  return chosen === undefined ? measured : clampWidth(chosen);
}

export function setWidth(layout: ColumnLayout, key: string, width: number): ColumnLayout {
  return { ...layout, widths: { ...layout.widths, [key]: clampWidth(width) } };
}

/** Back to the measured width. */
export function clearWidth(layout: ColumnLayout, key: string): ColumnLayout {
  if (!(key in layout.widths)) return layout;
  const widths = { ...layout.widths };
  delete widths[key];
  return { ...layout, widths };
}

/**
 * Put `key` immediately before `before`, or last when `before` is null.
 *
 * Works on the full order rather than the visible one, so a hidden column keeps
 * its place relative to its neighbours and comes back where it was.
 */
export function moveColumn(
  keys: string[],
  layout: ColumnLayout,
  key: string,
  before: string | null,
): ColumnLayout {
  if (key === before) return layout;
  const order = fullOrder(keys, layout).filter((k) => k !== key);
  const at = before === null ? order.length : order.indexOf(before);
  if (at === -1) return layout;
  order.splice(at, 0, key);
  return { ...layout, order };
}

export function hideColumn(layout: ColumnLayout, key: string): ColumnLayout {
  if (layout.hidden.includes(key)) return layout;
  return { ...layout, hidden: [...layout.hidden, key] };
}

export function showColumn(layout: ColumnLayout, key: string): ColumnLayout {
  if (!layout.hidden.includes(key)) return layout;
  return { ...layout, hidden: layout.hidden.filter((k) => k !== key) };
}

export function showAll(layout: ColumnLayout): ColumnLayout {
  return layout.hidden.length === 0 ? layout : { ...layout, hidden: [] };
}

/** Freeze the first `count` visible columns; zero unfreezes. */
export function freeze(layout: ColumnLayout, count: number): ColumnLayout {
  const frozen = Math.max(0, Math.round(count));
  return frozen === layout.frozen ? layout : { ...layout, frozen };
}

/**
 * Left edges of the frozen columns, given the visible widths in display order.
 *
 * `null` for a column that scrolls. The first frozen column sits after the row
 * gutter, and each next one after the one before it — the same sum the eye
 * does when it reads the header.
 */
export function frozenOffsets(widths: number[], frozen: number, gutter: number): (number | null)[] {
  let left = gutter;
  return widths.map((width, i) => {
    if (i >= frozen) return null;
    const at = left;
    left += width;
    return at;
  });
}

/** Whether there is anything here worth keeping. */
export function isEmptyLayout(layout: ColumnLayout): boolean {
  return (
    Object.keys(layout.widths).length === 0 &&
    layout.order.length === 0 &&
    layout.hidden.length === 0 &&
    layout.frozen === 0
  );
}

/**
 * Whatever was on disk, made safe to use.
 *
 * Field by field rather than trusting the shape: a layout file is written by
 * this version and read by every later one, and a stored value that is not
 * what it should be is dropped rather than allowed to throw inside a render.
 */
export function normalizeLayout(stored: unknown): ColumnLayout {
  if (typeof stored !== "object" || stored === null) return EMPTY_LAYOUT;
  const raw = stored as Partial<Record<keyof ColumnLayout, unknown>>;

  const widths: Record<string, number> = {};
  if (typeof raw.widths === "object" && raw.widths !== null) {
    for (const [key, value] of Object.entries(raw.widths)) {
      if (typeof value === "number" && Number.isFinite(value)) widths[key] = clampWidth(value);
    }
  }
  const strings = (value: unknown): string[] =>
    Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];

  return {
    widths,
    order: strings(raw.order),
    hidden: strings(raw.hidden),
    frozen:
      typeof raw.frozen === "number" && Number.isFinite(raw.frozen)
        ? Math.max(0, Math.round(raw.frozen))
        : 0,
  };
}

/**
 * What a layout is stored under.
 *
 * The table, when the result is one table's rows — however it was reached, a
 * query tab's `SELECT * FROM orders` and the table tab for `orders` are the
 * same columns and want the same arrangement. Otherwise the connection and the
 * column names together: a join has no table, but the same join run tomorrow
 * has the same columns, and that is what the arrangement was made for.
 */
export function layoutKeyFor(
  connectionId: string,
  tab: { kind: string; schema?: string | undefined; title: string },
  result: {
    columns: {
      name: string;
      source?: { schema?: string | null | undefined; table: string } | undefined;
    }[];
  },
): string {
  let table: string | null = null;
  let schema: string | null = null;
  for (const column of result.columns) {
    const source = column.source;
    if (!source) continue;
    if (table === null) {
      table = source.table;
      schema = source.schema ?? null;
    } else if (table !== source.table) {
      table = null;
      break;
    }
  }
  // A driver without column provenance still knows which table a table tab
  // opened, which is enough.
  if (table === null && tab.kind === "table") {
    table = tab.title;
    schema = tab.schema ?? null;
  }
  if (table !== null) return `${connectionId} table ${schema ?? ""} ${table}`;
  return `${connectionId} columns ${columnKeys(result.columns).join(" ")}`;
}
