import { describe, expect, it } from "vitest";
import { tabsOf } from "./workspace";
import type { Tab } from "./workspace";

/** A state shaped like the store's, without needing the store itself. */
function state(tabs: Record<string, Tab[]>) {
  return { tabs };
}

describe("tabsOf", () => {
  it("returns the same array every time for a connection with no tabs", () => {
    // This is not a micro-optimisation. A zustand selector's result is compared
    // by identity, so a fresh `[]` per call makes every render look like a
    // change; React gives up with "Maximum update depth exceeded" and the pane
    // renders as a blank screen.
    const s = state({});
    expect(tabsOf(s, "conn-1")).toBe(tabsOf(s, "conn-1"));
    expect(tabsOf(s, "conn-1")).toBe(tabsOf(s, "conn-2"));
  });

  it("returns the connection's own list when it has one", () => {
    const tab = { id: "tab-1", kind: "query", title: "Query 1" } as Tab;
    const s = state({ "conn-1": [tab] });
    expect(tabsOf(s, "conn-1")).toEqual([tab]);
    expect(tabsOf(s, "conn-1")).toBe(s.tabs["conn-1"]);
  });

  it("does not let a caller grow the shared empty list", () => {
    // Frozen, because a caller pushing onto the fallback would give every
    // connection in the app that tab.
    const s = state({});
    expect(() => (tabsOf(s, "conn-1") as Tab[]).push({} as Tab)).toThrow();
    expect(tabsOf(s, "conn-2")).toHaveLength(0);
  });
});
