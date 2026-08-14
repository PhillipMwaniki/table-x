import { describe, expect, it } from "vitest";
import {
  clampFontSize,
  DEFAULT_SETTINGS,
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
