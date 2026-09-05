/**
 * Keyboard shortcuts: which keys mean which actions, and how to change them.
 *
 * Every shortcut the app answers to is listed here, once, with its default.
 * The actions themselves are the commands the palette already knows about —
 * a shortcut is a second way to reach one — so binding a key is a matter of
 * naming the command it runs, and there is no second list of behaviours to
 * keep in step with the first.
 *
 * Overrides are stored by action id, so a default that changes in a later
 * version changes for everyone who did not choose otherwise.
 */

export interface Action {
  /** The command this runs; see the palette's registry. */
  id: string;
  label: string;
  /** The default binding, in the canonical spelling `keyOf` produces. */
  key: string;
  /**
   * Whether the shortcut also fires with the caret in the SQL editor.
   *
   * False for anything the editor has its own meaning for — Ctrl+Z is the
   * editor's undo, Ctrl+F its find, Ctrl+Enter runs through its keymap so the
   * selection is honoured — and true for what should work from anywhere.
   */
  inEditor: boolean;
}

export const ACTIONS: Action[] = [
  { id: "app.palette", label: "Command palette", key: "Ctrl+K", inEditor: true },
  { id: "app.settings", label: "Settings", key: "Ctrl+,", inEditor: true },
  { id: "app.sidebar", label: "Show or hide the connections pane", key: "Ctrl+B", inEditor: true },
  { id: "ws.run", label: "Run the statement", key: "Ctrl+Enter", inEditor: false },
  { id: "ws.run-keep", label: "Run into a new result", key: "Ctrl+Shift+Enter", inEditor: false },
  { id: "ws.format", label: "Format SQL", key: "Ctrl+Shift+F", inEditor: true },
  { id: "ws.explain", label: "Explain the statement", key: "Ctrl+Shift+E", inEditor: true },
  { id: "ws.new-tab", label: "New query tab", key: "Ctrl+T", inEditor: true },
  { id: "ws.close-tab", label: "Close tab", key: "Ctrl+W", inEditor: true },
  { id: "ws.next-tab", label: "Next tab", key: "Ctrl+PageDown", inEditor: true },
  { id: "ws.prev-tab", label: "Previous tab", key: "Ctrl+PageUp", inEditor: true },
  { id: "ws.history", label: "Query history", key: "Ctrl+H", inEditor: true },
  { id: "ws.undo", label: "Undo cell edit", key: "Ctrl+Z", inEditor: false },
  { id: "ws.redo", label: "Redo cell edit", key: "Ctrl+Shift+Z", inEditor: false },
  { id: "grid.find", label: "Find in results", key: "Ctrl+F", inEditor: false },
];

/** Overrides by action id. An empty string unbinds the action. */
export type Keymap = Record<string, string>;

/** Keys that stand on their own rather than being typed. */
const NAMED = new Set([
  "Enter",
  "Escape",
  "Backspace",
  "Delete",
  "Tab",
  "Space",
  "PageUp",
  "PageDown",
  "Home",
  "End",
  "Insert",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
]);

/**
 * The canonical spelling of a key event: modifiers in a fixed order, then the
 * key. Null for a press that is only modifiers, which is not a shortcut yet.
 *
 * Cmd counts as Ctrl, so a binding reads the same on every platform and a
 * setting made on one machine holds on another.
 */
export function keyOf(e: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}): string | null {
  let key = e.key;
  if (key === "Control" || key === "Meta" || key === "Shift" || key === "Alt" || key === "OS") {
    return null;
  }
  if (key === " ") key = "Space";
  else if (key.length === 1) key = key.toUpperCase();
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  parts.push(key);
  return parts.join("+");
}

/**
 * Whether a spelling is one this will match.
 *
 * A bare letter is typing, not a shortcut; a modifier or a function key is
 * what makes it one. Written to accept exactly what `keyOf` produces.
 */
export function isValidBinding(binding: string): boolean {
  const parts = binding.split("+");
  const key = parts.pop();
  if (!key) return false;
  const mods = new Set(parts);
  if (parts.some((p) => !["Ctrl", "Alt", "Shift"].includes(p)) || mods.size !== parts.length) {
    return false;
  }
  const isFunction = /^F([1-9]|1[0-2])$/.test(key);
  const isKey = key.length === 1 || NAMED.has(key) || isFunction;
  if (!isKey) return false;
  return mods.has("Ctrl") || mods.has("Alt") || isFunction;
}

/** The binding in force for an action: the override, else the default. */
export function bindingFor(id: string, keymap: Keymap): string {
  const action = ACTIONS.find((a) => a.id === id);
  if (!action) return "";
  return id in keymap ? (keymap[id] ?? "") : action.key;
}

/** Every action a key press means, under this keymap. */
export function actionsFor(key: string, keymap: Keymap): Action[] {
  return ACTIONS.filter((a) => bindingFor(a.id, keymap) === key);
}

/**
 * Which actions share a binding, by id.
 *
 * Two actions on one key is not always wrong — one may only fire in the
 * editor and the other only outside it — but it is always worth saying.
 */
export function conflicts(keymap: Keymap): Record<string, Action[]> {
  const out: Record<string, Action[]> = {};
  for (const action of ACTIONS) {
    const key = bindingFor(action.id, keymap);
    if (!key) continue;
    const others = ACTIONS.filter((o) => o.id !== action.id && bindingFor(o.id, keymap) === key);
    if (others.length > 0) out[action.id] = others;
  }
  return out;
}

/** Whatever was stored, reduced to overrides this version knows and can match. */
export function normalizeKeymap(raw: unknown): Keymap {
  if (typeof raw !== "object" || raw === null) return {};
  const out: Keymap = {};
  for (const [id, value] of Object.entries(raw)) {
    if (!ACTIONS.some((a) => a.id === id)) continue;
    if (typeof value !== "string") continue;
    if (value === "" || isValidBinding(value)) out[id] = value;
  }
  return out;
}
