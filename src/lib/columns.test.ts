import { describe, expect, it } from "vitest";
import {
  arrange,
  clearWidth,
  columnKeys,
  EMPTY_LAYOUT,
  freeze,
  frozenOffsets,
  hideColumn,
  isEmptyLayout,
  layoutKeyFor,
  MAX_COLUMN_WIDTH,
  MIN_COLUMN_WIDTH,
  moveColumn,
  normalizeLayout,
  setWidth,
  showAll,
  showColumn,
  widthFor,
} from "./columns";

const cols = (...names: string[]) => names.map((name) => ({ name }));

describe("columnKeys", () => {
  it("uses the name when names are unique", () => {
    expect(columnKeys(cols("id", "name"))).toEqual(["id", "name"]);
  });

  it("numbers repeats so a join's two ids do not share a layout", () => {
    // `SELECT a.id, b.id` — the same name twice, and dragging one must not
    // move the other.
    expect(columnKeys(cols("id", "name", "id"))).toEqual(["id", "name", "id#2"]);
  });
});

describe("arrange", () => {
  const keys = ["a", "b", "c", "d"];

  it("is the source order when nothing has been arranged", () => {
    expect(arrange(keys, EMPTY_LAYOUT)).toEqual([0, 1, 2, 3]);
  });

  it("follows a stored order and appends columns it has not heard of", () => {
    // The layout was saved when the query had three columns; a fourth appears
    // and must be shown rather than dropped.
    const layout = { ...EMPTY_LAYOUT, order: ["c", "a", "b"] };
    expect(arrange(keys, layout)).toEqual([2, 0, 1, 3]);
  });

  it("ignores stored keys the result does not have", () => {
    const layout = { ...EMPTY_LAYOUT, order: ["gone", "b", "a"] };
    expect(arrange(keys, layout)).toEqual([1, 0, 2, 3]);
  });

  it("leaves hidden columns out", () => {
    const layout = { ...EMPTY_LAYOUT, hidden: ["b"] };
    expect(arrange(keys, layout)).toEqual([0, 2, 3]);
  });
});

describe("moveColumn", () => {
  const keys = ["a", "b", "c", "d"];

  it("puts a column before another", () => {
    const layout = moveColumn(keys, EMPTY_LAYOUT, "d", "b");
    expect(arrange(keys, layout)).toEqual([0, 3, 1, 2]);
  });

  it("puts a column last when there is nothing to go before", () => {
    const layout = moveColumn(keys, EMPTY_LAYOUT, "a", null);
    expect(arrange(keys, layout)).toEqual([1, 2, 3, 0]);
  });

  it("keeps a hidden column's place, so it comes back where it was", () => {
    const hidden = hideColumn(EMPTY_LAYOUT, "b");
    const moved = moveColumn(keys, hidden, "d", "c");
    expect(arrange(keys, moved)).toEqual([0, 3, 2]);
    expect(arrange(keys, showColumn(moved, "b"))).toEqual([0, 1, 3, 2]);
  });

  it("is a no-op for a move onto itself or an unknown target", () => {
    expect(moveColumn(keys, EMPTY_LAYOUT, "a", "a")).toBe(EMPTY_LAYOUT);
    expect(moveColumn(keys, EMPTY_LAYOUT, "a", "nope")).toBe(EMPTY_LAYOUT);
  });
});

describe("widths", () => {
  it("prefers a chosen width and falls back to the measured one", () => {
    expect(widthFor("a", 120, EMPTY_LAYOUT)).toBe(120);
    expect(widthFor("a", 120, setWidth(EMPTY_LAYOUT, "a", 200))).toBe(200);
  });

  it("clamps a dragged width to what a column can be", () => {
    expect(setWidth(EMPTY_LAYOUT, "a", 3).widths["a"]).toBe(MIN_COLUMN_WIDTH);
    expect(setWidth(EMPTY_LAYOUT, "a", 1e9).widths["a"]).toBe(MAX_COLUMN_WIDTH);
    expect(setWidth(EMPTY_LAYOUT, "a", 99.6).widths["a"]).toBe(100);
  });

  it("clears back to measured, and clearing an unset width changes nothing", () => {
    const set = setWidth(EMPTY_LAYOUT, "a", 200);
    expect(widthFor("a", 120, clearWidth(set, "a"))).toBe(120);
    expect(clearWidth(EMPTY_LAYOUT, "a")).toBe(EMPTY_LAYOUT);
  });
});

describe("hiding", () => {
  it("hides once, shows once, and showAll empties the list", () => {
    const once = hideColumn(EMPTY_LAYOUT, "a");
    expect(hideColumn(once, "a")).toBe(once);
    expect(showColumn(once, "a").hidden).toEqual([]);
    expect(showColumn(EMPTY_LAYOUT, "a")).toBe(EMPTY_LAYOUT);
    expect(showAll(hideColumn(once, "b")).hidden).toEqual([]);
    expect(showAll(EMPTY_LAYOUT)).toBe(EMPTY_LAYOUT);
  });
});

describe("freezing", () => {
  it("places each frozen column after the gutter and the one before it", () => {
    expect(frozenOffsets([100, 50, 80], 2, 52)).toEqual([52, 152, null]);
  });

  it("freezes nothing at zero and never goes negative", () => {
    expect(frozenOffsets([100, 50], 0, 52)).toEqual([null, null]);
    expect(freeze(EMPTY_LAYOUT, -3).frozen).toBe(0);
    expect(freeze(EMPTY_LAYOUT, 0)).toBe(EMPTY_LAYOUT);
  });
});

describe("normalizeLayout", () => {
  it("is empty for anything that is not a layout", () => {
    expect(normalizeLayout(undefined)).toEqual(EMPTY_LAYOUT);
    expect(normalizeLayout("no")).toEqual(EMPTY_LAYOUT);
  });

  it("keeps the usable parts of a file and drops the rest", () => {
    const layout = normalizeLayout({
      widths: { a: 120, b: "wide", c: Number.NaN },
      order: ["b", 3, "a"],
      hidden: [null, "c"],
      frozen: "2",
    });
    expect(layout).toEqual({ widths: { a: 120 }, order: ["b", "a"], hidden: ["c"], frozen: 0 });
  });

  it("knows an empty layout when it sees one", () => {
    expect(isEmptyLayout(EMPTY_LAYOUT)).toBe(true);
    expect(isEmptyLayout(freeze(EMPTY_LAYOUT, 1))).toBe(false);
    expect(isEmptyLayout(clearWidth(setWidth(EMPTY_LAYOUT, "a", 90), "a"))).toBe(true);
  });
});

describe("layoutKeyFor", () => {
  const query = { kind: "query", title: "Query 1" };
  const source = (table: string, schema: string | null = "public") => ({ table, schema });

  it("keys one table's rows by the table, however they were reached", () => {
    const fromQuery = layoutKeyFor("c1", query, {
      columns: [
        { name: "id", source: source("orders") },
        { name: "total", source: source("orders") },
      ],
    });
    const fromTab = layoutKeyFor(
      "c1",
      { kind: "table", schema: "public", title: "orders" },
      { columns: [{ name: "id" }, { name: "total" }] },
    );
    expect(fromQuery).toBe(fromTab);
  });

  it("keys a join by its columns, since it has no table", () => {
    const key = layoutKeyFor("c1", query, {
      columns: [
        { name: "id", source: source("orders") },
        { name: "name", source: source("customers") },
      ],
    });
    expect(key).toContain("columns");
    expect(key).not.toContain("orders");
  });

  it("keeps connections apart", () => {
    const columns = { columns: [{ name: "id" }] };
    expect(layoutKeyFor("c1", query, columns)).not.toBe(layoutKeyFor("c2", query, columns));
  });
});
