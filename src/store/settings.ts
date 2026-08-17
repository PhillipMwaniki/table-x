/**
 * Appearance state, applied to the document and persisted between runs.
 *
 * Changes take effect as they are made rather than on an OK button: picking a
 * theme you cannot see until you confirm it is picking blind. Persistence goes
 * through the Tauri store plugin, which owns a small JSON file in the app data
 * directory — this is view preference, not the connection catalogue, so it does
 * not need the atomic-write machinery the backend uses for that.
 */

import { create } from "zustand";
import { load as loadStore } from "@tauri-apps/plugin-store";
import type { Store } from "@tauri-apps/plugin-store";
import {
  clampFontSize,
  clampPageSize,
  clampRatio,
  DEFAULT_SETTINGS,
  normalize,
  resolveTheme,
  rowHeightFor,
} from "@/lib/settings";
import type { Settings, ThemeChoice } from "@/lib/settings";

const FILE = "settings.json";
const KEY = "appearance";

interface SettingsState extends Settings {
  /** False until the stored file has been read, so nothing is saved over it. */
  ready: boolean;

  init: () => Promise<void>;
  setTheme: (theme: ThemeChoice) => void;
  setUiFont: (stack: string) => void;
  setDataFont: (stack: string) => void;
  setDataFontSize: (size: number) => void;
  setEditorRatio: (ratio: number) => void;
  setPageSize: (rows: number) => void;
  setStripedRows: (striped: boolean) => void;
  reset: () => void;
}

let handle: Store | null = null;

async function persist(settings: Settings) {
  try {
    handle ??= await loadStore(FILE);
    await handle.set(KEY, settings);
    await handle.save();
  } catch (e) {
    // A preference that failed to save is a preference that reverts next launch
    // — annoying, but not a reason to interrupt someone mid-query.
    console.warn("could not save appearance settings", e);
  }
}

/** Write the settings onto <html>, which is where every token reads them from. */
export function apply(settings: Settings) {
  const root = document.documentElement;
  const prefersDark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true;
  const theme = resolveTheme(settings.theme, prefersDark);

  root.dataset.theme = theme.id;
  // Tailwind's `dark:` variant keys on the class, and the token blocks key on
  // the attribute. Both have to move together or a themed app ends up with
  // light-mode utilities over a dark palette.
  root.classList.toggle("dark", theme.appearance === "dark");

  // An empty stack means "use what styles.css declares", so the property is
  // removed rather than set to nothing — an empty custom property would
  // override the stylesheet with no font at all.
  setOrClear(root, "--font-sans", settings.uiFont);
  setOrClear(root, "--font-mono", settings.dataFont);

  const size = clampFontSize(settings.dataFontSize);
  root.style.setProperty("--text-data", `${size}px`);
  root.style.setProperty("--row-height", `${rowHeightFor(size)}px`);
}

function setOrClear(root: HTMLElement, property: string, value: string) {
  if (value.trim()) root.style.setProperty(property, value);
  else root.style.removeProperty(property);
}

export const useSettings = create<SettingsState>((set, get) => {
  /** Apply and persist in one step, so the two can never drift apart. */
  const commit = (changes: Partial<Settings>) => {
    // Destructured rather than spread from `get()`, which also carries the
    // actions: writing those into the persisted file would put function-shaped
    // holes in it that `normalize` then has to throw away on the next launch.
    const { theme, uiFont, dataFont, dataFontSize, editorRatio, pageSize, stripedRows } = get();
    const next: Settings = {
      theme,
      uiFont,
      dataFont,
      dataFontSize,
      editorRatio,
      pageSize,
      stripedRows,
      ...changes,
    };
    set(next);
    apply(next);
    void persist(next);
  };

  return {
    ...DEFAULT_SETTINGS,
    ready: false,

    init: async () => {
      let stored: unknown;
      try {
        handle ??= await loadStore(FILE);
        stored = await handle.get(KEY);
      } catch (e) {
        // First run, or a file this build cannot read. Defaults are a working
        // app; refusing to start over a preferences file would not be.
        console.warn("could not read appearance settings", e);
      }

      const settings = normalize(stored);
      set({ ...settings, ready: true });
      apply(settings);

      // Following the OS means following it while the app is open, not only at
      // launch.
      window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => {
        if (get().theme === "system") apply(get());
      });
    },

    setTheme: (theme) => commit({ theme }),
    setUiFont: (uiFont) => commit({ uiFont }),
    setDataFont: (dataFont) => commit({ dataFont }),
    setDataFontSize: (dataFontSize) => commit({ dataFontSize: clampFontSize(dataFontSize) }),
    // Called once when the drag ends rather than on every pointer move: the
    // live position is the dragging component's own state, and a store write
    // per frame would mean a file write per frame.
    setEditorRatio: (editorRatio) => commit({ editorRatio: clampRatio(editorRatio) }),
    setPageSize: (pageSize) => commit({ pageSize: clampPageSize(pageSize) }),
    setStripedRows: (stripedRows) => commit({ stripedRows }),
    reset: () => commit(DEFAULT_SETTINGS),
  };
});
