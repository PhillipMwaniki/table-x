import { beforeEach, describe, expect, it, vi } from "vitest";

// Held apart from the factories below so both can reach it: `vi.mock` is
// hoisted above every import, and a plain `const` would not exist yet when the
// mocked module is first pulled in.
const { relink } = vi.hoisted(() => ({ relink: vi.fn() }));

// Mocked before the store is imported: the real modules reach for Tauri's
// `invoke`, which does not exist outside the app shell. Every call a reconnect
// makes swallows its own failures, so rejecting stubs are enough to walk the
// whole path.
vi.mock("@/lib/ipc", () => ({
  ipc: {
    sessionInfo: vi.fn().mockRejectedValue(new Error("no session")),
    transactionState: vi.fn().mockRejectedValue(new Error("no transaction")),
    completionScope: vi.fn().mockRejectedValue(new Error("no completion")),
  },
  IpcError: class extends Error {},
}));

// The connection store owns the link itself; what matters here is only whether
// it says the repair worked.
vi.mock("@/store/connections", () => ({
  noteLinkFailure: vi.fn(),
  useConnections: { getState: () => ({ reconnect: relink }) },
}));

vi.mock("@tauri-apps/plugin-store", () => ({
  load: vi.fn().mockResolvedValue({ get: vi.fn(), set: vi.fn(), save: vi.fn() }),
}));

const { tabsOf, useWorkspace } = await import("./workspace");
type Tab = import("./workspace").Tab;

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

describe("reconnecting", () => {
  beforeEach(() => {
    relink.mockReset();
    // One tab, so the reconnect does not go looking for a saved workspace to
    // restore into an empty connection.
    useWorkspace.setState({
      tabs: { "conn-1": [{ id: "tab-1", kind: "query", title: "Query 1" } as Tab] },
      active: { "conn-1": "tab-1" },
      schemaVersion: {},
    });
  });

  it("tells the object tree to rebuild", async () => {
    // The tree caches what it fetched and refetches only when this counter
    // moves. Without the bump it keeps showing whatever it had when the link
    // went — in practice the connection error it failed on, with no way to ask
    // again short of switching connections and back.
    relink.mockResolvedValue(true);

    await useWorkspace.getState().reconnect("conn-1");

    expect(useWorkspace.getState().schemaVersion["conn-1"]).toBe(1);
  });

  it("leaves the tree alone when the link is still down", async () => {
    // Rebuilding would only replace what is on screen with the same failure,
    // and a tree that reloads on a reconnect that did not happen reads as
    // though it did.
    relink.mockResolvedValue(false);

    await useWorkspace.getState().reconnect("conn-1");

    expect(useWorkspace.getState().schemaVersion["conn-1"]).toBeUndefined();
  });
});
