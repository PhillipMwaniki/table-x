/**
 * Column layouts, kept between runs.
 *
 * One entry per table or column set, written to a small JSON file through the
 * Tauri store plugin the same way appearance settings are: this is how a
 * person likes to look at a result, not the result itself, so it needs no
 * atomic-write machinery and is allowed to be lost.
 */

import { create } from "zustand";
import { load as loadStore } from "@tauri-apps/plugin-store";
import type { Store } from "@tauri-apps/plugin-store";
import { EMPTY_LAYOUT, isEmptyLayout, normalizeLayout } from "@/lib/columns";
import type { ColumnLayout } from "@/lib/columns";

const FILE = "layouts.json";
const KEY = "columns";

/**
 * How many layouts to keep.
 *
 * Every table ever opened would otherwise accumulate a record. The least
 * recently changed go first: a layout untouched for three hundred tables is a
 * layout for a table nobody is coming back to.
 */
const LIMIT = 300;

interface LayoutState {
  layouts: Record<string, ColumnLayout>;
  /** False until the file has been read, so nothing is saved over it. */
  ready: boolean;

  init: () => Promise<void>;
  /**
   * Replace one layout, or remove it when what results is empty — a record
   * saying "nothing special" is indistinguishable from no record, and the
   * shorter file is the one worth keeping.
   */
  set: (key: string, next: (was: ColumnLayout) => ColumnLayout) => void;
}

let handle: Store | null = null;

async function persist(layouts: Record<string, ColumnLayout>) {
  try {
    handle ??= await loadStore(FILE);
    await handle.set(KEY, layouts);
    await handle.save();
  } catch (e) {
    console.warn("could not save column layouts", e);
  }
}

/** Every stored entry made safe, in the order it was stored. */
function normalizeAll(stored: unknown): Record<string, ColumnLayout> {
  if (typeof stored !== "object" || stored === null) return {};
  const out: Record<string, ColumnLayout> = {};
  for (const [key, value] of Object.entries(stored)) {
    const layout = normalizeLayout(value);
    if (!isEmptyLayout(layout)) out[key] = layout;
  }
  return out;
}

export const useLayouts = create<LayoutState>((set, get) => ({
  layouts: {},
  ready: false,

  init: async () => {
    let stored: unknown;
    try {
      handle ??= await loadStore(FILE);
      stored = await handle.get(KEY);
    } catch (e) {
      // A file this build cannot read is a file to start over from; the grid
      // works without any layout at all.
      console.warn("could not read column layouts", e);
    }
    set({ layouts: normalizeAll(stored), ready: true });
  },

  set: (key, next) => {
    const { layouts, ready } = get();
    const updated = next(layouts[key] ?? EMPTY_LAYOUT);
    // Deleting and re-adding moves the key to the end of the record, which is
    // what makes the eviction below drop the one touched longest ago.
    const rest = { ...layouts };
    delete rest[key];
    const entries = Object.entries(rest);
    if (!isEmptyLayout(updated)) entries.push([key, updated]);
    while (entries.length > LIMIT) entries.shift();
    const all = Object.fromEntries(entries);
    set({ layouts: all });
    if (ready) void persist(all);
  },
}));
