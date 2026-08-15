/**
 * Exports currently running, and what they have written so far.
 *
 * The backend emits a progress event per batch; this store is what turns that
 * into something on screen. An export over a tunnel to a remote server can take
 * minutes, and silence for minutes is indistinguishable from a hang.
 */

import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "@/lib/ipc";

/** One progress report, as the backend emits it. */
export interface ExportProgress {
  id: string;
  /** What is being worked on: a table, a database, a file. */
  label: string;
  /** What `rows` counts — rows, tables, or kilobytes of a file being read. */
  unit: string;
  rows: number;
  /** An estimate where one exists. Not a count. */
  total: number | null;
  done: boolean;
}

export interface RunningExport extends ExportProgress {
  /** Set once the user has asked to stop, so the button can say so. */
  cancelling: boolean;
}

interface ExportState {
  running: Record<string, RunningExport>;

  /** Subscribe to backend progress. Safe to call more than once. */
  watch: () => Promise<void>;
  /** Register a job before its first event, so the bar appears at once. */
  begin: (id: string, label: string, unit?: string) => void;
  /** Remove one, whether it finished, failed, or was cancelled. */
  end: (id: string) => void;
  cancel: (id: string) => Promise<void>;
}

let watching = false;

export const useExports = create<ExportState>((set, get) => ({
  running: {},

  watch: async () => {
    if (watching) return;
    watching = true;
    await listen<ExportProgress>("export-progress", (event) => {
      const progress = event.payload;
      if (progress.done) {
        get().end(progress.id);
        return;
      }
      set((s) => ({
        running: {
          ...s.running,
          [progress.id]: {
            ...progress,
            cancelling: s.running[progress.id]?.cancelling ?? false,
          },
        },
      }));
    });
  },

  begin: (id, label, unit = "rows") =>
    set((s) => ({
      running: {
        ...s.running,
        // Zero and no estimate yet: the first query has not returned, and that
        // first query is often the slow part.
        [id]: { id, label, unit, rows: 0, total: null, done: false, cancelling: false },
      },
    })),

  end: (id) =>
    set((s) => {
      const running = { ...s.running };
      delete running[id];
      return { running };
    }),

  cancel: async (id) => {
    // Marked immediately, because the export stops at the next batch boundary
    // and a button that looks unpressed until then invites a second press.
    set((s) => ({
      running: s.running[id]
        ? { ...s.running, [id]: { ...s.running[id], cancelling: true } }
        : s.running,
    }));
    await ipc.cancelExport(id);
  },
}));
