import { describe, expect, it } from "vitest";
import { parseInline, parseMarkdown, plainText } from "./markdown";
import type { Inline } from "./markdown";

/** The rendered text of an inline run, for asserting without a tree walk. */
function text(nodes: Inline[]): string {
  return nodes
    .map((n) => (n.kind === "text" || n.kind === "code" ? n.text : text(n.children)))
    .join("");
}

describe("blocks", () => {
  it("reads headings, paragraphs and rules", () => {
    const blocks = parseMarkdown("# Title\n\nSome prose.\n\n---\n");
    expect(blocks.map((b) => b.kind)).toEqual(["heading", "paragraph", "rule"]);
    expect(blocks[0]).toMatchObject({ level: 1 });
  });

  it("ends a paragraph at the next block, not only at a blank line", () => {
    // Otherwise a heading written straight after a sentence becomes part of it.
    const blocks = parseMarkdown("Some prose.\n## Heading\n");
    expect(blocks.map((b) => b.kind)).toEqual(["paragraph", "heading"]);
  });

  it("joins a wrapped paragraph into one", () => {
    const blocks = parseMarkdown("one\ntwo\nthree");
    expect(blocks).toHaveLength(1);
    expect(text((blocks[0] as { children: Inline[] }).children)).toBe("one two three");
  });

  it("reads both kinds of list", () => {
    const bullets = parseMarkdown("- one\n- two");
    expect(bullets[0]).toMatchObject({ kind: "list", ordered: false });
    expect((bullets[0] as { items: Inline[][] }).items).toHaveLength(2);

    const numbered = parseMarkdown("1. one\n2. two");
    expect(numbered[0]).toMatchObject({ kind: "list", ordered: true });
  });

  it("keeps a fenced block literal", () => {
    // Everything inside is text, including lines that would otherwise be
    // headings or bullets.
    const blocks = parseMarkdown("```sql\n# not a heading\n- not a list\n```");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ kind: "code", language: "sql" });
    expect((blocks[0] as { text: string }).text).toBe("# not a heading\n- not a list");
  });

  it("does not swallow the document when a fence is never closed", () => {
    const blocks = parseMarkdown("```\nstill here");
    expect(blocks).toHaveLength(1);
    expect((blocks[0] as { text: string }).text).toBe("still here");
  });

  it("gathers consecutive quote lines into one", () => {
    const blocks = parseMarkdown("> first\n> second\n\nafter");
    expect(blocks.map((b) => b.kind)).toEqual(["quote", "paragraph"]);
    expect(text((blocks[0] as { children: Inline[] }).children)).toBe("first second");
  });

  it("is empty for empty input rather than producing a blank paragraph", () => {
    expect(parseMarkdown("")).toEqual([]);
    expect(parseMarkdown("\n\n  \n")).toEqual([]);
  });
});

describe("inline", () => {
  it("reads emphasis, strong and code", () => {
    expect(parseInline("**bold**")[0]).toMatchObject({ kind: "strong" });
    expect(parseInline("_italic_")[0]).toMatchObject({ kind: "em" });
    expect(parseInline("`code`")[0]).toMatchObject({ kind: "code", text: "code" });
  });

  it("treats a backtick span as literal", () => {
    // Parsing emphasis first would eat the asterisks inside it.
    const nodes = parseInline("`**not bold**`");
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({ kind: "code", text: "**not bold**" });
  });

  it("does not turn a stray underscore into emphasis", () => {
    // Column names are full of them, and prose about a database is full of
    // column names.
    expect(text(parseInline("user_id and order_id"))).toBe("user_id and order_id");
    expect(parseInline("user_id and order_id").every((n) => n.kind === "text")).toBe(true);
  });

  it("keeps http and mailto links and refuses the rest", () => {
    const ok = parseInline("[docs](https://example.com)");
    expect(ok[0]).toMatchObject({ kind: "link", href: "https://example.com" });

    // The classic way a markdown renderer becomes a script runner — and a
    // database column is an ordinary place for one to arrive from.
    const dangerous = parseInline("[click](javascript:alert(1))");
    expect(dangerous.every((n) => n.kind !== "link")).toBe(true);
    // Shown as what was written rather than silently vanishing.
    expect(text(dangerous)).toContain("javascript:alert(1)");

    for (const href of ["data:text/html,<script>", "vbscript:x", "JavaScript:x"]) {
      const nodes = parseInline(`[x](${href})`);
      expect(nodes.every((n) => n.kind !== "link"), href).toBe(true);
    }
  });

  it("nests emphasis inside strong", () => {
    const nodes = parseInline("**bold _and italic_**");
    expect(nodes[0]).toMatchObject({ kind: "strong" });
    expect(text(nodes)).toBe("bold and italic");
  });

  it("leaves an unmatched marker as text", () => {
    expect(text(parseInline("2 * 3 = 6"))).toBe("2 * 3 = 6");
    expect(text(parseInline("a ** b"))).toBe("a ** b");
  });

  it("coalesces plain runs instead of one node per character", () => {
    const nodes = parseInline("just some ordinary prose");
    expect(nodes).toHaveLength(1);
  });
});

describe("plainText", () => {
  it("flattens a document for a title or a search", () => {
    const flat = plainText(parseMarkdown("# Title\n\nSome **bold** prose.\n\n- one\n- two"));
    expect(flat).toContain("Title");
    expect(flat).toContain("Some bold prose.");
    expect(flat).toContain("one two");
  });
});
