import { describe, expect, it } from "vitest";
import {
  ACTIONS,
  actionsFor,
  bindingFor,
  conflicts,
  isValidBinding,
  keyOf,
  normalizeKeymap,
} from "./keymap";

const press = (
  key: string,
  mods: Partial<Record<"ctrl" | "meta" | "shift" | "alt", boolean>> = {},
) =>
  keyOf({
    key,
    ctrlKey: mods.ctrl ?? false,
    metaKey: mods.meta ?? false,
    shiftKey: mods.shift ?? false,
    altKey: mods.alt ?? false,
  });

describe("keyOf", () => {
  it("spells a press with the modifiers in one order and the key last", () => {
    expect(press("k", { ctrl: true })).toBe("Ctrl+K");
    expect(press("F", { ctrl: true, shift: true })).toBe("Ctrl+Shift+F");
    expect(press("Enter", { alt: true, shift: true, ctrl: true })).toBe("Ctrl+Alt+Shift+Enter");
    expect(press(" ", { ctrl: true })).toBe("Ctrl+Space");
  });

  it("reads Cmd as Ctrl, so a binding holds across machines", () => {
    expect(press("k", { meta: true })).toBe("Ctrl+K");
  });

  it("is nothing for a modifier on its own", () => {
    expect(press("Control", { ctrl: true })).toBeNull();
    expect(press("Shift", { shift: true })).toBeNull();
  });
});

describe("isValidBinding", () => {
  it("wants a modifier, or a function key", () => {
    expect(isValidBinding("Ctrl+K")).toBe(true);
    expect(isValidBinding("Alt+Enter")).toBe(true);
    expect(isValidBinding("F5")).toBe(true);
    expect(isValidBinding("Shift+F5")).toBe(true);
    // Typing.
    expect(isValidBinding("K")).toBe(false);
    expect(isValidBinding("Shift+K")).toBe(false);
  });

  it("refuses spellings it did not produce", () => {
    expect(isValidBinding("Control+K")).toBe(false);
    expect(isValidBinding("Ctrl+Ctrl+K")).toBe(false);
    expect(isValidBinding("Ctrl+")).toBe(false);
    expect(isValidBinding("Ctrl+Whatever")).toBe(false);
  });
});

describe("bindings", () => {
  it("every default is valid and no two defaults collide", () => {
    for (const action of ACTIONS) expect(isValidBinding(action.key)).toBe(true);
    expect(conflicts({})).toEqual({});
  });

  it("an override replaces the default, and an empty one unbinds", () => {
    expect(bindingFor("ws.history", {})).toBe("Ctrl+H");
    expect(bindingFor("ws.history", { "ws.history": "Ctrl+Y" })).toBe("Ctrl+Y");
    expect(bindingFor("ws.history", { "ws.history": "" })).toBe("");
    expect(actionsFor("Ctrl+Y", { "ws.history": "Ctrl+Y" }).map((a) => a.id)).toEqual([
      "ws.history",
    ]);
    expect(actionsFor("Ctrl+H", { "ws.history": "Ctrl+Y" })).toEqual([]);
  });

  it("names the actions that share a key", () => {
    const shared = conflicts({ "ws.history": "Ctrl+K" });
    expect(shared["ws.history"]?.map((a) => a.id)).toEqual(["app.palette"]);
    expect(shared["app.palette"]?.map((a) => a.id)).toEqual(["ws.history"]);
    expect(shared["ws.format"]).toBeUndefined();
  });
});

describe("normalizeKeymap", () => {
  it("keeps only known actions with bindings it can match", () => {
    expect(
      normalizeKeymap({
        "ws.history": "Ctrl+Y",
        "ws.format": "",
        "gone.action": "Ctrl+G",
        "ws.explain": 3,
        "ws.undo": "Z",
      }),
    ).toEqual({ "ws.history": "Ctrl+Y", "ws.format": "" });
    expect(normalizeKeymap("nope")).toEqual({});
  });
});
