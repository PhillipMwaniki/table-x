/**
 * A small markdown parser for notebook prose.
 *
 * Written rather than installed for two reasons. The obvious one is size: this
 * application's position is that it is small, and a full CommonMark
 * implementation to render some headings and bullet points is a poor trade.
 *
 * The better one is that this produces a tree, not a string of HTML. Nothing
 * downstream ever calls `dangerouslySetInnerHTML`, so there is no path from
 * text somebody typed — or pasted out of a database — to executable markup. A
 * markdown library that emits HTML makes sanitising it your problem; not
 * emitting HTML makes it nobody's.
 *
 * The subset is deliberately what prose around a query needs: headings,
 * paragraphs, lists, quotes, code, and emphasis. No tables, no footnotes, no
 * raw HTML — raw HTML least of all.
 */

export type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "strong"; children: Inline[] }
  | { kind: "em"; children: Inline[] }
  | { kind: "link"; href: string; children: Inline[] };

export type Block =
  | { kind: "heading"; level: 1 | 2 | 3; children: Inline[] }
  | { kind: "paragraph"; children: Inline[] }
  | { kind: "list"; ordered: boolean; items: Inline[][] }
  | { kind: "quote"; children: Inline[] }
  | { kind: "code"; language: string | null; text: string }
  | { kind: "rule" };

/** Parse markdown into blocks. */
export function parseMarkdown(source: string): Block[] {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i] ?? "";

    if (line.trim() === "") {
      i++;
      continue;
    }

    // Fenced code first: everything inside is literal, including things that
    // would otherwise be headings or lists.
    const fence = /^```(\w*)\s*$/.exec(line.trim());
    if (fence) {
      const language = fence[1] ? fence[1] : null;
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i]?.trim() ?? "")) {
        body.push(lines[i] ?? "");
        i++;
      }
      // An unclosed fence runs to the end rather than swallowing the document
      // into nothing.
      i++;
      blocks.push({ kind: "code", language, text: body.join("\n") });
      continue;
    }

    if (/^(-{3,}|\*{3,}|_{3,})\s*$/.test(line.trim())) {
      blocks.push({ kind: "rule" });
      i++;
      continue;
    }

    const heading = /^(#{1,3})\s+(.*)$/.exec(line);
    if (heading) {
      blocks.push({
        kind: "heading",
        level: heading[1]!.length as 1 | 2 | 3,
        children: parseInline(heading[2] ?? ""),
      });
      i++;
      continue;
    }

    if (/^>\s?/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i] ?? "")) {
        body.push((lines[i] ?? "").replace(/^>\s?/, ""));
        i++;
      }
      blocks.push({ kind: "quote", children: parseInline(body.join(" ")) });
      continue;
    }

    const bullet = /^\s*[-*+]\s+/;
    const numbered = /^\s*\d+[.)]\s+/;
    if (bullet.test(line) || numbered.test(line)) {
      const ordered = numbered.test(line);
      const items: Inline[][] = [];
      const matcher = ordered ? numbered : bullet;
      while (i < lines.length && matcher.test(lines[i] ?? "")) {
        items.push(parseInline((lines[i] ?? "").replace(matcher, "")));
        i++;
      }
      blocks.push({ kind: "list", ordered, items });
      continue;
    }

    // A paragraph runs until a blank line or the start of another block, so a
    // heading immediately after a sentence is still a heading.
    const body: string[] = [];
    while (i < lines.length) {
      const next = lines[i] ?? "";
      if (
        next.trim() === "" ||
        /^#{1,3}\s/.test(next) ||
        /^>\s?/.test(next) ||
        bullet.test(next) ||
        numbered.test(next) ||
        /^```/.test(next.trim()) ||
        /^(-{3,}|\*{3,}|_{3,})\s*$/.test(next.trim())
      ) {
        break;
      }
      body.push(next);
      i++;
    }
    blocks.push({ kind: "paragraph", children: parseInline(body.join(" ")) });
  }

  return blocks;
}

/**
 * Parse the inline span syntax.
 *
 * Code first and greedily, because a backtick span is literal: `**not bold**`
 * inside one is text, and parsing emphasis first would eat it.
 */
export function parseInline(source: string): Inline[] {
  const out: Inline[] = [];
  let i = 0;

  const push = (node: Inline) => {
    const last = out[out.length - 1];
    if (node.kind === "text" && last?.kind === "text") last.text += node.text;
    else out.push(node);
  };

  while (i < source.length) {
    const rest = source.slice(i);

    const code = /^`([^`]+)`/.exec(rest);
    if (code) {
      push({ kind: "code", text: code[1] ?? "" });
      i += code[0].length;
      continue;
    }

    const link = /^\[([^\]]*)\]\(([^)\s]+)\)/.exec(rest);
    if (link) {
      const href = link[2] ?? "";
      // Only these two schemes are ever rendered as a link. `javascript:` in an
      // href is the classic way a markdown renderer becomes a script runner,
      // and a database column is a perfectly ordinary place for one to arrive
      // from.
      if (/^(https?:\/\/|mailto:)/i.test(href)) {
        push({ kind: "link", href, children: parseInline(link[1] ?? "") });
      } else {
        // Kept as the text it was written as, so nothing silently vanishes.
        push({ kind: "text", text: link[0] });
      }
      i += link[0].length;
      continue;
    }

    const strong = /^(\*\*|__)([\s\S]+?)\1/.exec(rest);
    if (strong && opensEmphasis(source, i, strong[1] ?? "")) {
      push({ kind: "strong", children: parseInline(strong[2] ?? "") });
      i += strong[0].length;
      continue;
    }

    const em = /^(\*|_)(?!\s)([\s\S]+?)\1/.exec(rest);
    if (em && opensEmphasis(source, i, em[1] ?? "")) {
      push({ kind: "em", children: parseInline(em[2] ?? "") });
      i += em[0].length;
      continue;
    }

    // Plain text up to the next character that could start something, taken in
    // one piece so the common case is not one node per character.
    const next = /[`[*_]/.exec(source.slice(i + 1));
    const take = next ? next.index + 1 : source.length - i;
    push({ kind: "text", text: source.slice(i, i + take) });
    i += take;
  }

  return out;
}

/**
 * Whether a delimiter at this position opens emphasis.
 *
 * Underscores inside a word do not, which is the rule that keeps `user_id and
 * order_id` as two identifiers rather than one italicised phrase. Prose about a
 * database is mostly column names, so this is the common case rather than an
 * edge one. Asterisks are left alone: nobody writes `total*count` meaning a
 * literal asterisk nearly as often, and `2 * 3` is caught by the no-space rule.
 */
function opensEmphasis(source: string, index: number, delimiter: string): boolean {
  if (!delimiter.startsWith("_")) return true;
  const before = source[index - 1];
  return before === undefined || !/[\p{L}\p{N}]/u.test(before);
}

/** The plain text of a block, for a title or a search index. */
export function plainText(blocks: Block[]): string {
  const inline = (nodes: Inline[]): string =>
    nodes
      .map((node) => {
        switch (node.kind) {
          case "text":
          case "code":
            return node.text;
          default:
            return inline(node.children);
        }
      })
      .join("");

  return blocks
    .map((block) => {
      switch (block.kind) {
        case "code":
          return block.text;
        case "rule":
          return "";
        case "list":
          return block.items.map(inline).join(" ");
        default:
          return inline(block.children);
      }
    })
    .filter(Boolean)
    .join("\n");
}
