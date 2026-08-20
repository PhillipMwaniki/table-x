/**
 * Tooltips for the SQL keywords that catch people out.
 *
 * The content lives in `lib/sqldocs`; this is only the plumbing that decides
 * which word the pointer is over and draws the box.
 *
 * Deliberately built as a DOM node rather than an HTML string. The text is
 * static and ours, so `innerHTML` would be safe today, but a tooltip that
 * renders markup is one edit away from rendering something that came out of a
 * database — and this app's whole job is showing text it did not write.
 */

import { hoverTooltip } from "@codemirror/view";
import type { Extension } from "@codemirror/state";
import { MAX_TERM_WORDS, lookupSqlDoc } from "@/lib/sqldocs";

/** What counts as part of a word. Underscores are in; `ROW_NUMBER` is one word. */
const WORD = /[A-Za-z_][A-Za-z0-9_]*/;

/** Left and right edges of the word under `pos`, or null if it is not on one. */
function wordAt(text: string, pos: number): { from: number; to: number; word: string } | null {
  const isWordChar = (c: string) => /[A-Za-z0-9_]/.test(c);

  let from = pos;
  let to = pos;
  while (from > 0 && isWordChar(text[from - 1]!)) from--;
  while (to < text.length && isWordChar(text[to]!)) to++;
  if (from === to) return null;

  const word = text.slice(from, to);
  return WORD.test(word) ? { from, to, word } : null;
}

/** The words immediately before `from`, nearest last, at most `count` of them. */
function wordsBefore(text: string, from: number, count: number): string[] {
  // Only the same statement's worth of text is worth scanning; a term is three
  // words at most, so a short window is plenty and keeps this O(1) on a long
  // document.
  const window = text.slice(Math.max(0, from - 60), from);
  const words = window.match(/[A-Za-z_][A-Za-z0-9_]*/g) ?? [];
  return words.slice(-count);
}

/**
 * Hover documentation for SQL keywords, in this engine's own wording.
 *
 * Returns null for every word the corpus has no opinion about, which is most of
 * them — the value is in firing on the hard parts and staying quiet elsewhere.
 */
export function sqlHover(driver: string): Extension {
  return hoverTooltip((view, pos) => {
    const text = view.state.doc.toString();
    const found = wordAt(text, pos);
    if (!found) return null;

    const doc = lookupSqlDoc(found.word, wordsBefore(text, found.from, MAX_TERM_WORDS - 1), driver);
    if (!doc) return null;

    // The term may be more than the word hovered — `LEFT JOIN` when the pointer
    // is on JOIN — so the tooltip is anchored to the start of the whole term.
    const extra = doc.term.split(" ").length - 1;
    const from = extra > 0 ? Math.max(0, findTermStart(text, found.from, extra)) : found.from;

    return {
      pos: from,
      end: found.to,
      above: true,
      create: () => ({ dom: render(doc.term, doc.summary, doc.note) }),
    };
  });
}

/** Walk back over `extra` words, so a multi-word term underlines all of itself. */
function findTermStart(text: string, from: number, extra: number): number {
  let at = from;
  for (let i = 0; i < extra; i++) {
    while (at > 0 && /\s/.test(text[at - 1]!)) at--;
    while (at > 0 && /[A-Za-z0-9_]/.test(text[at - 1]!)) at--;
  }
  return at;
}

function render(term: string, summary: string, note?: string): HTMLElement {
  const box = document.createElement("div");
  box.className = "tx-sql-hover";

  const heading = document.createElement("div");
  heading.className = "tx-sql-hover-term";
  heading.textContent = term;
  box.appendChild(heading);

  const body = document.createElement("div");
  body.className = "tx-sql-hover-summary";
  body.textContent = summary;
  box.appendChild(body);

  if (note) {
    const caution = document.createElement("div");
    caution.className = "tx-sql-hover-note";
    caution.textContent = note;
    box.appendChild(caution);
  }

  return box;
}
