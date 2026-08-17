import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  clampFontSize,
  clampRatio,
  DEFAULT_SETTINGS,
  MIN_EDITOR_RATIO,
  MAX_EDITOR_RATIO,
  MAX_FONT_SIZE,
  MIN_FONT_SIZE,
  normalize,
  resolveTheme,
  rowHeightFor,
  THEMES,
} from "./settings";

describe("resolveTheme", () => {
  it("follows the OS when the choice is 'system'", () => {
    expect(resolveTheme("system", true).id).toBe("tablex-dark");
    expect(resolveTheme("system", false).id).toBe("tablex-light");
  });

  it("returns the chosen theme regardless of the OS preference", () => {
    // Wanting a dark app on a light desktop is the normal case, not an edge one.
    expect(resolveTheme("nord", false).id).toBe("nord");
    expect(resolveTheme("solarized-light", true).id).toBe("solarized-light");
  });

  it("falls back to the default for a theme this build does not have", () => {
    // What a downgrade looks like: a stored id from a newer version. An
    // unstyled app is a worse answer than the default one.
    expect(resolveTheme("theme-from-the-future", true).id).toBe(THEMES[0]!.id);
  });

  it("pairs every theme with an appearance", () => {
    // The `dark:` class and the token block have to agree, so a theme with no
    // appearance would render light utilities over a dark palette.
    for (const theme of THEMES) {
      expect(["light", "dark"]).toContain(theme.appearance);
    }
  });
});

describe("clampFontSize", () => {
  it("keeps sizes within what the grid can lay out", () => {
    expect(clampFontSize(2)).toBe(MIN_FONT_SIZE);
    expect(clampFontSize(400)).toBe(MAX_FONT_SIZE);
    expect(clampFontSize(14)).toBe(14);
  });

  it("rounds fractional sizes rather than passing them through", () => {
    expect(clampFontSize(12.6)).toBe(13);
  });

  it("falls back to the default for a value that is not a number", () => {
    // A hand-edited settings file should not be able to blank the interface.
    expect(clampFontSize(Number.NaN)).toBe(DEFAULT_SETTINGS.dataFontSize);
  });
});

describe("clampRatio", () => {
  it("keeps both panes on screen", () => {
    // Neither can be dragged shut: a pane collapsed to nothing looks like a
    // bug, and the handle that would bring it back is what just vanished.
    expect(clampRatio(0)).toBe(MIN_EDITOR_RATIO);
    expect(clampRatio(1)).toBe(MAX_EDITOR_RATIO);
    expect(clampRatio(-5)).toBe(MIN_EDITOR_RATIO);
  });

  it("passes through a position inside the range", () => {
    expect(clampRatio(0.5)).toBe(0.5);
  });

  it("falls back to the default for a value that is not a number", () => {
    // A hand-edited settings file should not be able to make the pane vanish.
    expect(clampRatio(Number.NaN)).toBe(DEFAULT_SETTINGS.editorRatio);
  });
});

describe("rowHeightFor", () => {
  it("grows with the font", () => {
    expect(rowHeightFor(20)).toBeGreaterThan(rowHeightFor(12));
  });

  it("always leaves room for descenders", () => {
    // A row shorter than its own text clips it; the virtualizer positions rows
    // from this number, so the clipping cannot be absorbed by the layout.
    for (let size = MIN_FONT_SIZE; size <= MAX_FONT_SIZE; size++) {
      expect(rowHeightFor(size)).toBeGreaterThan(size * 1.4);
    }
  });
});

describe("normalize", () => {
  it("returns defaults for a missing or unusable file", () => {
    expect(normalize(undefined)).toEqual(DEFAULT_SETTINGS);
    expect(normalize("nonsense")).toEqual(DEFAULT_SETTINGS);
  });

  it("keeps recognised values and replaces the rest", () => {
    const settings = normalize({
      theme: "nord",
      dataFont: '"Fira Code", monospace',
      dataFontSize: 99,
      uiFont: 42,
    });

    expect(settings.theme).toBe("nord");
    expect(settings.dataFont).toBe('"Fira Code", monospace');
    expect(settings.dataFontSize).toBe(MAX_FONT_SIZE);
    // A number where a font stack belongs would reach the DOM as a broken
    // custom property, so it is dropped rather than stringified.
    expect(settings.uiFont).toBe(DEFAULT_SETTINGS.uiFont);
  });

  it("preserves 'system' as a choice in its own right", () => {
    expect(normalize({ theme: "system" }).theme).toBe("system");
  });
});

describe("stripedRows", () => {
  it("bands rows unless somebody has turned it off", () => {
    expect(DEFAULT_SETTINGS.stripedRows).toBe(true);
    expect(normalize({ stripedRows: false }).stripedRows).toBe(false);
  });

  it("keeps the default for anything that is not a boolean", () => {
    // A file holding the string "false" would otherwise turn banding *on*,
    // because a non-empty string is truthy.
    expect(normalize({ stripedRows: "false" }).stripedRows).toBe(true);
    expect(normalize({ stripedRows: 0 }).stripedRows).toBe(true);
    expect(normalize({}).stripedRows).toBe(true);
  });
});

describe("the theme list", () => {
  it("offers Tokyo Night, and calls it dark", () => {
    const tokyo = THEMES.filter((t) => t.id.startsWith("tokyo-night"));
    expect(tokyo.map((t) => t.id)).toEqual(["tokyo-night", "tokyo-night-storm"]);
    // `appearance` is what the `dark:` variant keys on; getting it wrong gives
    // a dark palette light-theme component styling.
    expect(tokyo.every((t) => t.appearance === "dark")).toBe(true);
  });

  it("has no two themes sharing an id", () => {
    // resolveTheme finds by id, so a duplicate would make one unreachable.
    expect(new Set(THEMES.map((t) => t.id)).size).toBe(THEMES.length);
  });

  it("resolves every listed theme, which is what stops an unstyled app", () => {
    for (const theme of THEMES) {
      expect(resolveTheme(theme.id, false).id).toBe(theme.id);
    }
  });
});

describe("every theme has a palette", () => {
  // A theme listed in the picker with no block in styles.css does not fail —
  // it silently renders as the base tokens, so picking it appears to do
  // nothing. Checked against the stylesheet because that is the only place
  // that would know.
  const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

  /** The pair that *are* the base tokens, and so correctly have no block. */
  const BASE = ["tablex-dark", "tablex-light"];

  for (const theme of THEMES.filter((t) => !BASE.includes(t.id))) {
    it(`defines tokens for ${theme.id}`, () => {
      // Found by scanning rather than by regex: the selector contains brackets
      // and quotes, and escaping those correctly is a worse problem than
      // slicing to the closing brace.
      const selector = `[data-theme="${theme.id}"]`;
      const start = css.indexOf(selector);
      expect(start, `no ${selector} block in styles.css`).toBeGreaterThan(-1);
      const block = css.slice(start, css.indexOf("}", start));

      // The whole set, not just some of it: a block that redefines the ground
      // but not the text colour gives one theme's text on another's background.
      for (const token of [
        "--color-surface-0",
        "--color-surface-1",
        "--color-surface-2",
        "--color-surface-3",
        "--color-border",
        "--color-text",
        "--color-text-muted",
        "--color-accent",
        "--color-accent-fg",
        "--color-ok",
        "--color-warn",
        "--color-danger",
        "--color-null",
      ]) {
        expect(block, `${theme.id} is missing ${token}`).toContain(`${token}:`);
      }
    });
  }
});
