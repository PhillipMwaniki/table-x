/**
 * Appearance settings: theme, type face, and type size.
 *
 * The shapes and the pure functions live here rather than in the store so they
 * can be tested without a DOM or a Tauri host.
 */

/** Whether a theme is a light or a dark one, which is what `dark:` keys on. */
export type Appearance = "light" | "dark";

export interface Theme {
  id: string;
  name: string;
  appearance: Appearance;
  /** Shown under the name in the picker. */
  note: string;
}

/**
 * The themes on offer.
 *
 * Table X Light and Table X Dark carry no `data-theme` block of their own —
 * they are the base tokens in `styles.css`. The rest each redefine the full
 * token set there.
 */
export const THEMES: Theme[] = [
  {
    id: "tablex-dark",
    name: "Table X Dark",
    appearance: "dark",
    note: "The default. Low-contrast greys, blue accent.",
  },
  {
    id: "tablex-light",
    name: "Table X Light",
    appearance: "light",
    note: "The same palette inverted.",
  },
  {
    id: "nord",
    name: "Nord",
    appearance: "dark",
    note: "Cool blue-greys from the Nord palette.",
  },
  {
    id: "solarized-dark",
    name: "Solarized Dark",
    appearance: "dark",
    note: "Ethan Schoonover's low-glare palette.",
  },
  {
    id: "solarized-light",
    name: "Solarized Light",
    appearance: "light",
    note: "Solarized on its warm paper base.",
  },
  {
    id: "tokyo-night",
    name: "Tokyo Night",
    appearance: "dark",
    note: "Deep indigo ground, cyan and magenta accents.",
  },
  {
    id: "tokyo-night-storm",
    name: "Tokyo Night Storm",
    appearance: "dark",
    note: "The same palette a shade lighter.",
  },
  {
    id: "high-contrast",
    name: "High Contrast",
    appearance: "dark",
    note: "Black ground, visible cell borders.",
  },
];

/** `system` follows the OS and resolves to one of the Table X pair. */
export type ThemeChoice = string | "system";

export interface Settings {
  theme: ThemeChoice;
  /** CSS font stack for chrome. */
  uiFont: string;
  /** CSS font stack for SQL, cell values, and type names. */
  dataFont: string;
  /** Pixel size of everything that is data. Chrome keeps its own sizes. */
  dataFontSize: number;
  /**
   * Share of the query pane given to the editor, as a fraction of its height.
   *
   * One setting for every tab and connection rather than one each: this is a
   * preference about how you work — more room to write, or more room to read —
   * not a property of any particular query.
   */
  editorRatio: number;
  /**
   * Rows fetched per page.
   *
   * A cap rather than a preference about how much you can see: the grid is
   * virtualized and would render a million rows happily, but the *transfer*
   * is what costs, especially through a tunnel. Paging keeps the first screen
   * fast and lets someone who wants more ask for it.
   */
  pageSize: number;
  /**
   * Tint every other result row.
   *
   * Worth a setting rather than being simply on: banding is what stops the eye
   * sliding up a line on a wide result, and it is also the first thing some
   * people turn off. Neither camp is wrong, so neither is overruled.
   */
  stripedRows: boolean;
}

/** Page sizes offered in the grid. */
export const PAGE_SIZES = [100, 500, 1000, 5000, 10000] as const;

const MIN_PAGE_SIZE = 10;
const MAX_PAGE_SIZE = 100_000;

/** Keep a stored or typed page size inside what a single fetch should carry. */
export function clampPageSize(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_SETTINGS.pageSize;
  return Math.min(MAX_PAGE_SIZE, Math.max(MIN_PAGE_SIZE, Math.round(value)));
}

/** Empty string means "whatever `styles.css` already specifies". */
export const DEFAULT_SETTINGS: Settings = {
  theme: "tablex-dark",
  uiFont: "",
  dataFont: "",
  dataFontSize: 12,
  editorRatio: 0.38,
  pageSize: 1000,
  stripedRows: true,
};

/**
 * Font stacks offered in the picker.
 *
 * Only the ones actually installed are shown — a list that offers fonts the
 * machine does not have is a list of ways to change nothing. The first entry of
 * each stack is what gets probed; the rest are the fallbacks that make the
 * choice survive being opened on another machine.
 */
export const DATA_FONTS: { label: string; stack: string }[] = [
  { label: "Default", stack: "" },
  { label: "JetBrains Mono", stack: '"JetBrains Mono", ui-monospace, monospace' },
  { label: "Cascadia Code", stack: '"Cascadia Code", ui-monospace, monospace' },
  { label: "Cascadia Mono", stack: '"Cascadia Mono", ui-monospace, monospace' },
  { label: "Fira Code", stack: '"Fira Code", ui-monospace, monospace' },
  { label: "Source Code Pro", stack: '"Source Code Pro", ui-monospace, monospace' },
  { label: "IBM Plex Mono", stack: '"IBM Plex Mono", ui-monospace, monospace' },
  { label: "Consolas", stack: "Consolas, ui-monospace, monospace" },
  { label: "SF Mono", stack: '"SF Mono", ui-monospace, monospace' },
  { label: "Menlo", stack: "Menlo, ui-monospace, monospace" },
  { label: "DejaVu Sans Mono", stack: '"DejaVu Sans Mono", ui-monospace, monospace' },
  { label: "Courier New", stack: '"Courier New", monospace' },
];

export const UI_FONTS: { label: string; stack: string }[] = [
  { label: "Default", stack: "" },
  { label: "Inter", stack: '"Inter var", Inter, system-ui, sans-serif' },
  { label: "Segoe UI", stack: '"Segoe UI Variable", "Segoe UI", system-ui, sans-serif' },
  { label: "System UI", stack: "system-ui, sans-serif" },
  { label: "Helvetica Neue", stack: '"Helvetica Neue", Helvetica, sans-serif' },
  { label: "Roboto", stack: "Roboto, system-ui, sans-serif" },
  { label: "SF Pro Text", stack: '"SF Pro Text", system-ui, sans-serif' },
];

export const MIN_FONT_SIZE = 9;
export const MAX_FONT_SIZE = 24;

/**
 * How far the editor/results split can be dragged.
 *
 * Neither pane can be dragged shut. A pane collapsed to nothing looks like a
 * bug rather than a choice, and the handle that would restore it is exactly
 * what has just disappeared.
 */
export const MIN_EDITOR_RATIO = 0.12;
export const MAX_EDITOR_RATIO = 0.85;

export function clampRatio(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_SETTINGS.editorRatio;
  return Math.min(MAX_EDITOR_RATIO, Math.max(MIN_EDITOR_RATIO, value));
}

/** Keep a stored or typed size inside what the grid can actually lay out. */
export function clampFontSize(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_SETTINGS.dataFontSize;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, Math.round(value)));
}

/**
 * Row height for a given data size.
 *
 * Proportional rather than fixed: a 20px font in a 24px row clips its own
 * descenders, and the virtualizer needs the height up front to place rows.
 */
export function rowHeightFor(fontSize: number): number {
  return Math.round(clampFontSize(fontSize) * 1.85) + 2;
}

/**
 * The theme actually in force, resolving `system` against the OS preference.
 *
 * Falls back to the default rather than leaving the app unstyled when a stored
 * theme id is one this build does not have — which is what happens to anyone
 * who downgrades after trying a newer version.
 */
export function resolveTheme(choice: ThemeChoice, prefersDark: boolean): Theme {
  if (choice === "system") {
    const id = prefersDark ? "tablex-dark" : "tablex-light";
    return THEMES.find((t) => t.id === id)!;
  }
  return THEMES.find((t) => t.id === choice) ?? THEMES[0]!;
}

/** Merge whatever was on disk over the defaults, discarding anything unusable. */
export function normalize(stored: unknown): Settings {
  if (typeof stored !== "object" || stored === null) return { ...DEFAULT_SETTINGS };
  const raw = stored as Partial<Settings>;
  const theme = typeof raw.theme === "string" ? raw.theme : "";
  const known = theme === "system" || THEMES.some((t) => t.id === theme);
  return {
    theme: known ? theme : DEFAULT_SETTINGS.theme,
    uiFont: typeof raw.uiFont === "string" ? raw.uiFont : DEFAULT_SETTINGS.uiFont,
    dataFont: typeof raw.dataFont === "string" ? raw.dataFont : DEFAULT_SETTINGS.dataFont,
    dataFontSize: clampFontSize(Number(raw.dataFontSize ?? DEFAULT_SETTINGS.dataFontSize)),
    editorRatio: clampRatio(Number(raw.editorRatio ?? DEFAULT_SETTINGS.editorRatio)),
    pageSize: clampPageSize(Number(raw.pageSize ?? DEFAULT_SETTINGS.pageSize)),
    // Only an actual boolean, so a file holding a string keeps the default
    // rather than turning banding on because "false" is truthy.
    stripedRows:
      typeof raw.stripedRows === "boolean" ? raw.stripedRows : DEFAULT_SETTINGS.stripedRows,
  };
}
