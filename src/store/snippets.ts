/**
 * Saved queries.
 *
 * The backend owns the file, so this is a cache refreshed after every mutation
 * rather than optimistically patched — the same stance the connection store
 * takes, and for the same reason: showing a save that did not reach disk is
 * worse than a round trip.
 */

import { create } from "zustand";
import { ipc, IpcError } from "@/lib/ipc";
import type { Snippet } from "@/lib/types";

interface SnippetState {
  snippets: Snippet[];
  /** Search box contents, matched against name and SQL. */
  filter: string;
  loading: boolean;
  error: string | null;

  load: () => Promise<void>;
  setFilter: (filter: string) => void;
  /** Save a new query under a name, or update one by passing its id. */
  save: (name: string, sql: string, id?: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  /** Snippets matching the current filter, newest first. */
  visible: () => Snippet[];
}

function message(e: unknown): string {
  return e instanceof IpcError || e instanceof Error ? e.message : String(e);
}

export const useSnippets = create<SnippetState>((set, get) => ({
  snippets: [],
  filter: "",
  loading: false,
  error: null,

  load: async () => {
    set({ loading: true, error: null });
    try {
      set({ snippets: await ipc.listSnippets(), loading: false });
    } catch (e) {
      set({ error: message(e), loading: false });
    }
  },

  setFilter: (filter) => set({ filter }),

  save: async (name, sql, id) => {
    // A new snippet gets its id here; the backend fills in the timestamps.
    const snippet: Snippet = {
      id: id ?? crypto.randomUUID(),
      name,
      sql,
      created_at: "",
      updated_at: "",
    };
    try {
      await ipc.saveSnippet(snippet);
      await get().load();
    } catch (e) {
      set({ error: message(e) });
    }
  },

  remove: async (id) => {
    try {
      await ipc.deleteSnippet(id);
      await get().load();
    } catch (e) {
      set({ error: message(e) });
    }
  },

  visible: () => {
    const { snippets, filter } = get();
    const terms = filter.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return snippets;
    // Every term must appear somewhere, matching how history searches — one
    // search behaviour in the app rather than two.
    return snippets.filter((s) => {
      const haystack = `${s.name} ${s.sql}`.toLowerCase();
      return terms.every((t) => haystack.includes(t));
    });
  },
}));
