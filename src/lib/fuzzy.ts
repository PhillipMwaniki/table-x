/**
 * Subsequence matching for the command palette.
 *
 * Typing `fmt` should find "Format SQL" and `nq` should find "New query" —
 * which substring matching cannot do. The scoring is deliberately simple and
 * predictable: a person types a few letters and expects the obvious command
 * first, not the cleverest one.
 */

export interface Match {
  score: number;
  /** Indices of the matched characters, for highlighting. */
  positions: number[];
}

/**
 * Score `text` against `query`, or `null` when it does not match.
 *
 * Every character of the query must appear in order. Matches score higher when
 * they land at the start of a word, land consecutively, or cover more of a
 * short string — the three signals that separate "the one I meant" from "the
 * one that happens to contain those letters".
 */
export function fuzzyMatch(query: string, text: string): Match | null {
  const needle = query.trim().toLowerCase();
  if (needle === "") return { score: 0, positions: [] };

  const haystack = text.toLowerCase();
  const positions: number[] = [];
  let score = 0;
  let cursor = 0;
  let previous = -2;

  for (const char of needle) {
    // Spaces in the query are separators, not characters to find.
    if (char === " ") continue;

    const at = haystack.indexOf(char, cursor);
    if (at === -1) return null;

    if (at === previous + 1) score += 8; // consecutive
    if (at === 0 || /[\s\-_./]/.test(haystack[at - 1] ?? "")) score += 12; // word start
    score += 1;

    positions.push(at);
    previous = at;
    cursor = at + 1;
  }

  // A short label matching the same letters is more likely to be the one meant:
  // "Run" over "Run every statement in a file".
  score += Math.max(0, 20 - text.length / 4);
  return { score, positions };
}

/** Keep and order the items that match, best first. */
export function fuzzyFilter<T>(query: string, items: T[], label: (item: T) => string): T[] {
  const scored = items
    .map((item) => ({ item, match: fuzzyMatch(query, label(item)) }))
    .filter((entry): entry is { item: T; match: Match } => entry.match !== null);

  // Stable within equal scores, so an unfiltered palette keeps its declared
  // order rather than shuffling as characters are typed and deleted.
  return scored.sort((a, b) => b.match.score - a.match.score).map((entry) => entry.item);
}
