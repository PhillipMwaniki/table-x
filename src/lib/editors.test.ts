import { describe, expect, it } from "vitest";
import {
  boolChoiceOf,
  byteSize,
  checkJson,
  editorFor,
  hexDump,
  LONG_VALUE,
  looksLikeJson,
  prettyJson,
} from "./editors";
import type { Value } from "./types";

describe("editorFor", () => {
  it("gives a boolean three states rather than a text box", () => {
    expect(editorFor({ kind: "bool", value: true })).toBe("bool");
  });

  it("gives JSON columns a JSON editor", () => {
    expect(editorFor({ kind: "json", value: { a: 1 } })).toBe("json");
  });

  it("gives a JSON document in a text column a JSON editor too", () => {
    // Storing JSON in a TEXT column is common enough that treating it as prose
    // would be the wrong answer most of the time.
    expect(editorFor({ kind: "text", value: '{"a": 1}' })).toBe("json");
    expect(editorFor({ kind: "text", value: "[1, 2, 3]" })).toBe("json");
  });

  it("gives long or multi-line text room", () => {
    expect(editorFor({ kind: "text", value: "x".repeat(LONG_VALUE + 1) })).toBe("text");
    expect(editorFor({ kind: "text", value: "line one\nline two" })).toBe("text");
  });

  it("keeps short scalars on one line", () => {
    expect(editorFor({ kind: "text", value: "Jo Smith" })).toBe("inline");
    expect(editorFor({ kind: "int", value: 42 })).toBe("inline");
    expect(editorFor({ kind: "numeric", value: "1.25" })).toBe("inline");
    expect(editorFor({ kind: "date", value: "2026-08-15" })).toBe("inline");
  });

  it("marks binary for viewing, not typing", () => {
    expect(editorFor({ kind: "bytes", value: [1, 2, 3] })).toBe("binary");
  });
});

describe("looksLikeJson", () => {
  it("recognises objects and arrays, not every string with a brace", () => {
    expect(looksLikeJson('  {"a":1} ')).toBe(true);
    expect(looksLikeJson("[]")).toBe(true);
    expect(looksLikeJson("use {braces} in prose")).toBe(false);
    expect(looksLikeJson("{")).toBe(false);
  });
});

describe("checkJson", () => {
  it("accepts valid JSON and an empty box", () => {
    expect(checkJson('{"a": 1}').valid).toBe(true);
    // Empty means NULL or empty string, which the value parser decides — not a
    // JSON error to block on.
    expect(checkJson("   ").valid).toBe(true);
  });

  it("explains what is wrong rather than just refusing", () => {
    const result = checkJson('{"a": }');
    expect(result.valid).toBe(false);
    // The database would answer with a message about the column; this one is
    // about the character, while the text is still on screen to fix.
    expect(result.error).toBeTruthy();
  });
});

describe("prettyJson", () => {
  it("indents JSON", () => {
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}');
  });

  it("leaves text it cannot parse exactly as it was", () => {
    // Reformatting on the way to failing would destroy the user's draft.
    expect(prettyJson('{"a": }')).toBe('{"a": }');
  });
});

describe("boolChoiceOf", () => {
  it("reads booleans and NULL", () => {
    expect(boolChoiceOf({ kind: "bool", value: true })).toBe("true");
    expect(boolChoiceOf({ kind: "bool", value: false })).toBe("false");
    expect(boolChoiceOf({ kind: "null" })).toBe("null");
  });

  it("reads the 0/1 an engine without a bool type returns", () => {
    expect(boolChoiceOf({ kind: "int", value: 1 } as Value)).toBe("true");
    expect(boolChoiceOf({ kind: "int", value: 0 } as Value)).toBe("false");
  });
});

describe("hexDump", () => {
  it("lays out offset, hex, and ASCII in fixed columns", () => {
    const bytes = [0x48, 0x69, 0x00, 0xff];
    const line = hexDump(bytes).split("\n")[0]!;
    expect(line.startsWith("00000000  48 69 00 ff")).toBe(true);
    // Unprintable bytes show as dots: a control character would move the cursor
    // and break the alignment the dump exists for.
    expect(line.endsWith("Hi..")).toBe(true);
  });

  it("keeps the ASCII column aligned on a short final row", () => {
    const lines = hexDump([1, 2, 3, 4], 4);
    expect(lines.split("\n")).toHaveLength(1);
    const twoRows = hexDump([1, 2, 3, 4, 5], 4).split("\n");
    expect(twoRows).toHaveLength(2);
    // Both rows put ASCII at the same column despite the second holding one byte.
    expect(twoRows[0]!.indexOf("....")).toBe(twoRows[1]!.length - 1);
  });

  it("produces nothing for no bytes", () => {
    expect(hexDump([])).toBe("");
  });
});

describe("byteSize", () => {
  it("scales the unit to the size", () => {
    expect(byteSize(512)).toBe("512 bytes");
    expect(byteSize(2048)).toBe("2.0 KB");
    expect(byteSize(5 * 1024 * 1024)).toBe("5.0 MB");
  });
});
