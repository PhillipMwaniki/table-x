/**
 * Query history panel state.
 *
 * Searching happens in the backend, which owns the file, rather than by loading
 * everything and filtering here: the history is capped but still thousands of
 * statements long, and shipping all of it across the IPC boundary on every
 * keystroke would cost far more than the search itself.
 *
 * The panel is a single shared surface rather than one per connection — it can
 * be scoped to the active connection or search across all of them, which is the
 * more useful default when you remember a query but not where you ran it.
 */

import { create } from "zustand";
import { ipc, IpcError } from "@/lib/ipc";
import type { HistoryEntry } from "@/lib/types";

export type HistoryScope = "connection" | "all";

/** Which list the side panel is showing. */
export type PanelTab = "history" | "snippets";

interface HistoryState {
  open: boolean;
  tab: PanelTab;
  entries: HistoryEntry[];
  /** The raw search box contents. */
  text: string;
  scope: HistoryScope;
  loading: boolean;
  error: string | null;

  setOpen: (open: boolean) => void;
  setTab: (tab: PanelTab) => void;
  setText: (text: string) => void;
  setScope: (scope: HistoryScope) => void;
  /** Re-run the current search for the given connection. */
  refresh: (connectionId: string) => Promise<void>;
  clear: (connectionId: string) => Promise<void>;
}

function message(e: unknown): string {
  return e instanceof IpcError || e instanceof Error ? e.message : String(e);
}

export const useHistory = create<HistoryState>((set, get) => ({
  open: false,
  tab: "history",
  entries: [],
  text: "",
  scope: "connection",
  loading: false,
  error: null,

  setOpen: (open) => set({ open }),
  setTab: (tab) => set({ tab }),
  setText: (text) => set({ text }),
  setScope: (scope) => set({ scope }),

  refresh: async (connectionId) => {
    const { text, scope } = get();
    set({ loading: true, error: null });
    try {
      const entries = await ipc.queryHistory({
        connection_id: scope === "connection" ? connectionId : undefined,
        text: text.trim() || undefined,
      });
      set({ entries, loading: false });
    } catch (e) {
      set({ error: message(e), loading: false });
    }
  },

  clear: async (connectionId) => {
    const { scope } = get();
    try {
      // Clearing follows the visible scope, so the button can never delete more
      // than the list the user is looking at.
      await ipc.clearQueryHistory(scope === "connection" ? connectionId : undefined);
      await get().refresh(connectionId);
    } catch (e) {
      set({ error: message(e) });
    }
  },
}));
