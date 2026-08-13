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
  selectedId: string | null;
  loading: boolean;
  error: string | null;

  init: () => Promise<void>;
  refresh: () => Promise<void>;
  select: (id: string | null) => void;
  save: (config: ConnectionConfig, secret?: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  connect: (id: string) => Promise<void>;
  disconnect: (id: string) => Promise<void>;
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
      const [connections, open] = await Promise.all([
        ipc.listConnections(),
        ipc.openConnections(),
      ]);
      set({ connections, open: new Set(open) });
    } catch (e) {
      set({ error: message(e) });
    }
  },

  select: (id) => set({ selectedId: id }),
  clearError: () => set({ error: null }),

  save: async (config, secret) => {
    // Deliberately not caught: the dialog needs the failure so it can stay open
    // with the user's input intact rather than closing over a lost edit.
    await ipc.saveConnection(config, secret);
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
      set((s) => ({ open: withId(s.open, id, true) }));
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
      set((s) => ({ open: withId(s.open, id, false) }));
    } catch (e) {
      set({ error: message(e) });
    } finally {
      set((s) => ({ busy: withId(s.busy, id, false) }));
    }
  },
}));
