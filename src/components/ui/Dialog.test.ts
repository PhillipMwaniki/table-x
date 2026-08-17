/**
 * Guards the one thing about `Dialog` that a type checker cannot.
 *
 * These are source-level assertions rather than rendering ones on purpose. The
 * bug they protect against is a layout failure that only WebKitGTK exhibits — the
 * dialog collapsing to a fraction of its content — and layout is exactly what a
 * DOM stub does not compute, so a render test under jsdom would pass while the
 * Linux window stayed broken. Reproducing it for real needs the engine itself,
 * which is not something to ask of every `pnpm test`. What is worth pinning here
 * is the arrangement that engine turned out to need.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const source = readFileSync(fileURLToPath(new URL("./Dialog.tsx", import.meta.url)), "utf8");

/** Just the Tailwind class strings, so prose in the doc comment cannot match. */
const classNames = Array.from(source.matchAll(/"([^"\n]*)"/g))
  .map((m) => m[1] ?? "")
  .filter((s) => /\b(flex|grid|max-h-|min-h-|overflow-)/.test(s))
  .join(" ");

describe("Dialog layout", () => {
  it("does not make the dialog element itself the flex column", () => {
    // Measured in WebKitGTK 2.52.3: with the column on the `<dialog>` and the
    // scroll region at `flex-1`, a dialog holding 996px of fields rendered 110px
    // tall — the body was 24px of pure padding. The same column on a plain inner
    // div rendered 736px and scrolled. `open:` is the only way to put the column
    // back on the dialog element (a bare `display: flex` would beat the
    // user-agent's `display: none` and show a closed dialog), so its absence is
    // what pins the arrangement.
    expect(classNames).not.toMatch(/\bopen:/);
  });

  it("keeps the scroll region shrinkable and scrollable", () => {
    // `min-h-0` and `overflow-y-auto` are load-bearing: without `min-h-0` the
    // region cannot shrink below its content in a flex column, so it overflows
    // instead of scrolling.
    expect(classNames).toMatch(/min-h-0/);
    expect(classNames).toMatch(/overflow-y-auto/);

    // `flex-auto` over `flex-1` is the second belt rather than a requirement:
    // with the column on a wrapper, `flex-1` measured fine too (736px). It is
    // pinned because a `0%` basis contributes nothing to a content-sized column,
    // which is half of what collapsed the dialog to 110px when the column sat on
    // the dialog element — so this is the half worth not relying on luck for.
    expect(classNames).toMatch(/flex-auto/);
    expect(classNames).not.toMatch(/\bflex-1\b/);
  });

  it("caps its height so the scroll region has something to be bounded by", () => {
    // Without a max-height somewhere on the column, the body grows to its
    // content and there is nothing to scroll inside.
    expect(classNames).toMatch(/max-h-\[/);
  });
});
