import { beforeEach, describe, expect, it, vi } from "vitest";

// Mocked before the store is imported: the real module reaches for Tauri's
// `invoke`, which does not exist outside the app shell.
vi.mock("@/lib/ipc", () => ({
  ipc: {
    reconnect: vi.fn(),
  },
  IpcError: class extends Error {},
}));

const { ipc } = await import("@/lib/ipc");
const { useConnections } = await import("./connections");

const reconnect = ipc.reconnect as unknown as ReturnType<typeof vi.fn>;

/** An open connection, as the store holds one. */
function opened(id: string) {
  useConnections.setState({
    open: new Set([id]),
    broken: new Set(),
    busy: new Set(),
    error: null,
  });
}

beforeEach(() => {
  reconnect.mockReset();
});

describe("a link that has broken", () => {
  it("is marked without closing the connection", () => {
    // The workspace is mounted on `open`, and unmounting it would throw away
    // the tabs and results the user is trying to get back to. A dropped socket
    // is a reason to offer a repair, not to clear the screen.
    opened("c1");
    useConnections.getState().markBroken("c1");

    expect(useConnections.getState().broken.has("c1")).toBe(true);
    expect(useConnections.getState().open.has("c1")).toBe(true);
  });

  it("is repaired by a reconnect that succeeds", async () => {
    opened("c1");
    useConnections.getState().markBroken("c1");
    reconnect.mockResolvedValue(undefined);

    await expect(useConnections.getState().reconnect("c1")).resolves.toBe(true);
    expect(useConnections.getState().broken.has("c1")).toBe(false);
    expect(useConnections.getState().open.has("c1")).toBe(true);
    expect(useConnections.getState().busy.has("c1")).toBe(false);
  });

  it("stays marked when the reconnect fails", async () => {
    // The server has not come back yet. Clearing the flag would take away the
    // button that offers to try again, on the one screen where the user needs
    // it most.
    opened("c1");
    reconnect.mockRejectedValue(new Error("connection refused"));

    await expect(useConnections.getState().reconnect("c1")).resolves.toBe(false);
    expect(useConnections.getState().broken.has("c1")).toBe(true);
    expect(useConnections.getState().busy.has("c1")).toBe(false);
    expect(useConnections.getState().error).toBe("connection refused");
  });
});
