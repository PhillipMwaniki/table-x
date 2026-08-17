/**
 * Finding a name in a very long list of them.
 *
 * A schema with three thousand tables is a list nobody scrolls. What people do
 * instead is remember part of a name — `order`, `_audit`, `v2` — so the match
 * is a plain case-insensitive substring rather than a fuzzy one. Fuzzy matching
 * is right for a command palette, where the candidates are few and distinct;
 * over thousands of names that all share prefixes and suffixes it returns
 * everything, ranked, which is the same as returning nothing.
 */

/** Whether a name contains the needle, ignoring case. */
export function matchesName(name: string, needle: string): boolean {
  if (!needle) return true;
  return name.toLowerCase().includes(needle.toLowerCase());
}

export interface NamePart {
  text: string;
  /** Whether this run is the part that matched. */
  match: boolean;
}

/**
 * Split a name around every occurrence of the needle, for highlighting.
 *
 * Every occurrence rather than the first: `order_orders` matching `order`
 * should light up both, or the highlight is telling a small lie about why the
 * row is in the list.
 */
export function splitHighlight(name: string, needle: string): NamePart[] {
  if (!needle) return [{ text: name, match: false }];

  const haystack = name.toLowerCase();
  const target = needle.toLowerCase();
  const parts: NamePart[] = [];
  let at = 0;

  for (;;) {
    const found = haystack.indexOf(target, at);
    if (found === -1) break;
    if (found > at) parts.push({ text: name.slice(at, found), match: false });
    parts.push({ text: name.slice(found, found + target.length), match: true });
    at = found + target.length;
  }

  if (parts.length === 0) return [{ text: name, match: false }];
  if (at < name.length) parts.push({ text: name.slice(at), match: false });
  return parts;
}
