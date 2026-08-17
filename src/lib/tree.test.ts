import { describe, expect, it } from "vitest";
import { matchesName, splitHighlight } from "./tree";

describe("matchesName", () => {
  it("matches anywhere in the name, ignoring case", () => {
    expect(matchesName("order_items", "ITEM")).toBe(true);
    expect(matchesName("ORDER_ITEMS", "item")).toBe(true);
    expect(matchesName("order_items", "xyz")).toBe(false);
  });

  it("matches everything when there is nothing to match", () => {
    // An empty box is not a filter, and hiding the tree until something is
    // typed would be a strange way to show a tree.
    expect(matchesName("anything", "")).toBe(true);
  });

  it("treats the needle literally", () => {
    // A name is not a pattern. Someone typing `order.` wants a table with a dot
    // in it, not any character.
    expect(matchesName("order_x", "order.")).toBe(false);
    expect(matchesName("order.x", "order.")).toBe(true);
  });
});

describe("splitHighlight", () => {
  it("splits around the match", () => {
    expect(splitHighlight("order_items", "items")).toEqual([
      { text: "order_", match: false },
      { text: "items", match: true },
    ]);
  });

  it("highlights every occurrence, not just the first", () => {
    // `order_orders` matching `order` should light up both, or the highlight
    // is telling a small lie about why the row is in the list.
    const parts = splitHighlight("order_orders", "order");
    expect(parts.filter((p) => p.match)).toHaveLength(2);
    expect(parts.map((p) => p.text).join("")).toBe("order_orders");
  });

  it("keeps the name's own casing while matching case-insensitively", () => {
    const parts = splitHighlight("OrderItems", "orderitems");
    expect(parts).toEqual([{ text: "OrderItems", match: true }]);
  });

  it("returns the whole name unmatched when nothing matches", () => {
    expect(splitHighlight("users", "zzz")).toEqual([{ text: "users", match: false }]);
    expect(splitHighlight("users", "")).toEqual([{ text: "users", match: false }]);
  });

  it("never loses or duplicates a character", () => {
    // The parts are concatenated back into a rendered name, so anything else
    // would silently corrupt what is on screen.
    for (const [name, needle] of [
      ["aaa", "a"],
      ["order_items_orders", "order"],
      ["x", "x"],
      ["prefix_match", "prefix"],
      ["match_suffix", "suffix"],
    ] as const) {
      expect(
        splitHighlight(name, needle)
          .map((p) => p.text)
          .join(""),
      ).toBe(name);
    }
  });
});
