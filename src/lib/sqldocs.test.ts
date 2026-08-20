import { describe, expect, it } from "vitest";
import { MAX_TERM_WORDS, SQL_DOCS, lookupSqlDoc } from "./sqldocs";

describe("lookupSqlDoc", () => {
  it("is case insensitive, because people type either", () => {
    expect(lookupSqlDoc("count", [], "postgres")?.term).toBe("COUNT");
    expect(lookupSqlDoc("Count", [], "postgres")?.term).toBe("COUNT");
  });

  it("prefers the two-word term the cursor is at the end of", () => {
    // Hovering JOIN in "LEFT JOIN" must explain the left join, not joins in
    // general — the general entry has nothing to warn about and the specific
    // one does.
    expect(lookupSqlDoc("JOIN", ["LEFT"], "postgres")?.term).toBe("LEFT JOIN");
    expect(lookupSqlDoc("BY", ["GROUP"], "postgres")?.term).toBe("GROUP BY");
    expect(lookupSqlDoc("ALL", ["UNION"], "postgres")?.term).toBe("UNION ALL");
  });

  it("falls back to the single word when the pair is not a term", () => {
    expect(lookupSqlDoc("COUNT", ["SELECT"], "postgres")?.term).toBe("COUNT");
  });

  it("returns nothing for words it has no opinion about", () => {
    // The point of forty entries rather than four hundred: a tooltip on every
    // word teaches people to ignore tooltips.
    expect(lookupSqlDoc("users", [], "postgres")).toBeNull();
    expect(lookupSqlDoc("SELECT", [], "postgres")).toBeNull();
  });

  it("gives an engine its own spelling where they differ", () => {
    expect(lookupSqlDoc("STRING_AGG", [], "postgres")?.summary).toContain("delimited string");
    expect(lookupSqlDoc("STRING_AGG", [], "mysql")?.summary).toContain("GROUP_CONCAT");
    expect(lookupSqlDoc("STRING_AGG", [], "clickhouse")?.summary).toContain("groupArray");
  });

  it("drops the generic note when an engine replaces the summary", () => {
    // The note explains the summary. Kept beside a different engine's spelling
    // it would describe behaviour that engine does not have.
    const generic = lookupSqlDoc("CONFLICT", ["ON"], "postgres");
    expect(generic?.term).toBe("ON CONFLICT");

    const mysql = lookupSqlDoc("CONFLICT", ["ON"], "mysql");
    expect(mysql?.summary).toContain("ON DUPLICATE KEY UPDATE");
    expect(mysql?.note).toBeUndefined();
  });

  it("keeps the generic note when an engine only replaces the note", () => {
    const pg = lookupSqlDoc("DISTINCT", [], "postgres");
    expect(pg?.summary).toBe(SQL_DOCS["DISTINCT"]!.summary);
    expect(pg?.note).toContain("DISTINCT ON");
  });
});

describe("the corpus itself", () => {
  it("is upper case throughout, since lookup upper-cases the word", () => {
    // A lower-case key would simply never be found, silently.
    for (const term of Object.keys(SQL_DOCS)) {
      expect(term, term).toBe(term.toUpperCase());
    }
  });

  it("has no term longer than the lookup looks back", () => {
    // A four-word term would be unreachable rather than broken, which is the
    // kind of dead entry nobody notices. This caught `IS DISTINCT FROM`.
    for (const term of Object.keys(SQL_DOCS)) {
      expect(term.split(" ").length, term).toBeLessThanOrEqual(MAX_TERM_WORDS);
    }
  });

  it("writes every summary as a sentence", () => {
    // Length is not the measure -- "How many rows." is a fine summary. Being a
    // finished sentence is, because these sit together in one tooltip style.
    for (const [term, doc] of Object.entries(SQL_DOCS)) {
      // A digit is fine: "1, 2, 3 …" is the clearest way to say ROW_NUMBER.
      expect(doc.summary, term).toMatch(/^[A-Z0-9(]/);
      expect(doc.summary.endsWith("."), `${term}: ${doc.summary}`).toBe(true);
      expect(doc.note?.endsWith(".") ?? true, term).toBe(true);
    }
  });

  it("reaches every entry through the lookup", () => {
    // The real guard against a dead entry: walk each term in as if it were
    // hovered, and check the same one comes back.
    for (const term of Object.keys(SQL_DOCS)) {
      const words = term.split(" ");
      const word = words[words.length - 1]!;
      expect(lookupSqlDoc(word, words.slice(0, -1), "postgres")?.term, term).toBe(term);
    }
  });

  it("only overrides for drivers that exist", () => {
    // A typo in a driver id is invisible at runtime: the override is simply
    // never applied and the generic text shows instead.
    const drivers = new Set(["postgres", "mysql", "sqlite", "mssql", "clickhouse"]);
    for (const [term, doc] of Object.entries(SQL_DOCS)) {
      for (const id of Object.keys(doc.byDriver ?? {})) {
        expect(drivers.has(id), `${term} overrides unknown driver ${id}`).toBe(true);
      }
    }
  });
});
