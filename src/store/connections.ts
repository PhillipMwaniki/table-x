/**
 * Connection state.
 *
 * The backend owns the truth — the JSON file and the keychain — so this store is
 * a cache that is refreshed after every mutation rather than optimistically
 * patched. For an operation whose failure modes include "the disk is full" or
 * "the keychain is locked", showing a change that did not actually persist is
 * worse than a brief round trip.
 */

import { create } from "zustand";
import { ipc, IpcError } from "@/lib/ipc";
import type { ConnectionConfig, DriverInfo } from "@/lib/types";

interface ConnectionState {
  drivers: DriverInfo[];
  connections: ConnectionConfig[];
  /** Ids with a live session. */
  open: Set<string>;
  /** Ids currently connecting or disconnecting, for per-row spinners. */
  busy: Set<string>;
  /**
   * Ids whose link has failed under us.
   *
   * A session the backend still lists as open but that the network has taken
   * away: the socket is gone, and everything sent down it will fail until it is
   * rebuilt. Held separately from `open` rather than folded into it, because the
   * two mean different things to the UI — the tabs, results, and history of a
   * broken connection are still worth showing, and closing the workspace over a
   * dropped Wi-Fi connection would throw away work the user can still recover.
   */
  broken: Set<string>;
  selectedId: string | null;
  loading: boolean;
  error: string | null;

  init: () => Promise<void>;
  refresh: () => Promise<void>;
  select: (id: string | null) => void;
  save: (
    config: ConnectionConfig,
    secret?: string,
    sshSecrets?: (string | null)[],
  ) => Promise<void>;
  remove: (id: string) => Promise<void>;
  connect: (id: string) => Promise<void>;
  disconnect: (id: string) => Promise<void>;
  /**
   * Rebuild the link for a connection, keeping its place.
   *
   * Returns whether it worked, so a caller can decide what to do next rather
   * than having to re-read the store to find out.
   */
  reconnect: (id: string) => Promise<boolean>;
  /** Record that this connection's link has failed. */
  markBroken: (id: string) => void;
  clearError: () => void;
}

/** Add or remove an id from a Set without mutating the original. */
function withId(set: Set<string>, id: string, present: boolean): Set<string> {
  const next = new Set(set);
  if (present) next.add(id);
  else next.delete(id);
  return next;
}

function message(e: unknown): string {
  return e instanceof IpcError || e instanceof Error ? e.message : String(e);
}

export const useConnections = create<ConnectionState>((set, get) => ({
  drivers: [],
  connections: [],
  open: new Set(),
  busy: new Set(),
  broken: new Set(),
  selectedId: null,
  loading: true,
  error: null,

  init: async () => {
    set({ loading: true, error: null });
    try {
      const [drivers, connections, open] = await Promise.all([
        ipc.listDrivers(),
        ipc.listConnections(),
        ipc.openConnections(),
      ]);
      set({ drivers, connections, open: new Set(open), loading: false });
    } catch (e) {
      set({ error: message(e), loading: false });
    }
  },

  refresh: async () => {
    try {
      const [connections, open] = await Promise.all([ipc.listConnections(), ipc.openConnections()]);
      set({ connections, open: new Set(open) });
    } catch (e) {
      set({ error: message(e) });
    }
  },

  select: (id) => set({ selectedId: id }),
  clearError: () => set({ error: null }),

  save: async (config, secret, sshSecrets) => {
    // Deliberately not caught: the dialog needs the failure so it can stay open
    // with the user's input intact rather than closing over a lost edit.
    await ipc.saveConnection(config, secret, sshSecrets);
    await get().refresh();
  },

  remove: async (id) => {
    await ipc.deleteConnection(id);
    if (get().selectedId === id) set({ selectedId: null });
    await get().refresh();
  },

  connect: async (id) => {
    set((s) => ({ busy: withId(s.busy, id, true), error: null }));
    try {
      await ipc.connect(id);
      set((s) => ({ open: withId(s.open, id, true), broken: withId(s.broken, id, false) }));
    } catch (e) {
      set({ error: message(e) });
    } finally {
      set((s) => ({ busy: withId(s.busy, id, false) }));
    }
  },

  disconnect: async (id) => {
    set((s) => ({ busy: withId(s.busy, id, true) }));
    try {
      await ipc.disconnect(id);
      set((s) => ({ open: withId(s.open, id, false), broken: withId(s.broken, id, false) }));
    } catch (e) {
      set({ error: message(e) });
    } finally {
      set((s) => ({ busy: withId(s.busy, id, false) }));
    }
  },

  reconnect: async (id) => {
    set((s) => ({ busy: withId(s.busy, id, true), error: null }));
    try {
      await ipc.reconnect(id);
      // `open` is set rather than left alone: reconnecting is also how a
      // connection that was closed by a failure gets back, and the backend has
      // a live session either way by the time this resolves.
      set((s) => ({ open: withId(s.open, id, true), broken: withId(s.broken, id, false) }));
      return true;
    } catch (e) {
      // Still broken, and still marked as such — the server has not come back
      // yet, and the button that offers to try again must stay in front of the
      // user rather than disappearing on the first failed attempt.
      set((s) => ({ error: message(e), broken: withId(s.broken, id, true) }));
      return false;
    } finally {
      set((s) => ({ busy: withId(s.busy, id, false) }));
    }
  },

  markBroken: (id) => set((s) => ({ broken: withId(s.broken, id, true) })),
}));

/**
 * Record a failed call against its connection, when the link itself was at fault.
 *
 * Anything that reaches the backend through a connection id can hand its error
 * here, whichever corner of the UI caught it. A broken link is rarely noticed
 * first in a query result — more often it is a schema tree that will not expand
 * — and the offer to rebuild it should not depend on which call happened to be
 * the one that hit the dead socket.
 *
 * Failures of any other kind are left alone: a syntax error says nothing about
 * the connection.
 */
export function noteLinkFailure(connectionId: string, e: unknown): void {
  if (e instanceof IpcError && e.category === "connection") {
    useConnections.getState().markBroken(connectionId);
  }
}
