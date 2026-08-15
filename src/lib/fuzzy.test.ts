import { describe, expect, it } from "vitest";
import { fuzzyFilter, fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches letters in order, not only substrings", () => {
    // The whole reason for fuzzy matching: `fmt` finds "Format SQL".
    expect(fuzzyMatch("fmt", "Format SQL")).not.toBeNull();
    expect(fuzzyMatch("nq", "New query")).not.toBeNull();
  });

  it("rejects letters that are out of order", () => {
    expect(fuzzyMatch("qn", "New query")).toBeNull();
  });

  it("rejects a letter that is not there at all", () => {
    expect(fuzzyMatch("fmtz", "Format SQL")).toBeNull();
  });

  it("matches everything on an empty query", () => {
    // An empty palette shows every command rather than none.
    expect(fuzzyMatch("", "anything")?.score).toBe(0);
  });

  it("ignores spaces in the query", () => {
    expect(fuzzyMatch("n q", "New query")).not.toBeNull();
  });

  it("reports where it matched, for highlighting", () => {
    expect(fuzzyMatch("nq", "New query")?.positions).toEqual([0, 4]);
  });

  it("scores word starts above letters buried mid-word", () => {
    const wordStart = fuzzyMatch("sq", "Save query")!;
    const buried = fuzzyMatch("sq", "Assorted quibbles")!;
    expect(wordStart.score).toBeGreaterThan(buried.score);
  });

  it("scores consecutive letters above scattered ones", () => {
    const together = fuzzyMatch("form", "Format SQL")!;
    const scattered = fuzzyMatch("form", "Fetch or remove metadata")!;
    expect(together.score).toBeGreaterThan(scattered.score);
  });
});

describe("fuzzyFilter", () => {
  const commands = ["Run query", "Format SQL", "Save query", "New query tab", "Close tab"];

  it("puts the obvious command first", () => {
    // What a person types `sq` for.
    expect(fuzzyFilter("sq", commands, (c) => c)[0]).toBe("Save query");
    expect(fuzzyFilter("fmt", commands, (c) => c)[0]).toBe("Format SQL");
  });

  it("drops what does not match", () => {
    expect(fuzzyFilter("zzz", commands, (c) => c)).toEqual([]);
  });

  it("keeps the declared order when nothing is typed", () => {
    // A palette that reshuffles between keystrokes is one you cannot aim at.
    expect(fuzzyFilter("", commands, (c) => c)).toEqual(commands);
  });

  it("prefers the shorter of two equally good matches", () => {
    const items = ["Run", "Run every statement in the current file"];
    expect(fuzzyFilter("run", items, (c) => c)[0]).toBe("Run");
  });
});
