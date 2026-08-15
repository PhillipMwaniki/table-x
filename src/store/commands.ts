/**
 * The registry behind the command palette.
 *
 * Components register what they can do while they are mounted, so the palette
 * offers exactly the actions that are actually available: no "Run query" with
 * no connection open, and no stale entry pointing at a tab that closed.
 */

import { create } from "zustand";

export interface Command {
  /** Stable across re-registrations, so ordering does not jitter. */
  id: string;
  title: string;
  /** Shown to the right — "Query", "Connection", "View". */
  group: string;
  /** Displayed hint, e.g. "Ctrl+Enter". Not a binding; the owner binds it. */
  shortcut?: string | undefined;
  run: () => void;
}

interface CommandState {
  open: boolean;
  /** Registered sets, keyed by owner, so unmounting removes exactly its own. */
  sources: Record<string, Command[]>;

  setOpen: (open: boolean) => void;
  /** Replace one owner's commands. Returns a function that removes them. */
  register: (owner: string, commands: Command[]) => () => void;
  /** Every registered command, in registration order. */
  all: () => Command[];
}

const NO_COMMANDS: readonly Command[] = Object.freeze([]);

export const useCommands = create<CommandState>((set, get) => ({
  open: false,
  sources: {},

  setOpen: (open) => set({ open }),

  register: (owner, commands) => {
    set((s) => ({ sources: { ...s.sources, [owner]: commands } }));
    return () =>
      set((s) => {
        const sources = { ...s.sources };
        delete sources[owner];
        return { sources };
      });
  },

  all: () => Object.values(get().sources).flat() as Command[],
}));

/** Commands for an owner that has registered none — a stable empty array. */
export const noCommands = NO_COMMANDS as Command[];
