import { describe, expect, it } from "vitest";
import { folderNames, groupConnections, normalizeFolder } from "./folders";
import type { ConnectionConfig } from "./types";

function connection(name: string, folder?: string): ConnectionConfig {
  return {
    id: name,
    name,
    driver: "postgres",
    folder,
    tls: { mode: "prefer" },
    read_only: false,
    options: {},
  };
}

describe("normalizeFolder", () => {
  it("treats blank and whitespace as no folder", () => {
    // Otherwise a stray space becomes a folder named " " that nobody can find.
    expect(normalizeFolder("")).toBeUndefined();
    expect(normalizeFolder("   ")).toBeUndefined();
    expect(normalizeFolder(undefined)).toBeUndefined();
  });

  it("trims what it keeps", () => {
    expect(normalizeFolder("  Work ")).toBe("Work");
  });
});

describe("groupConnections", () => {
  it("orders folders alphabetically and puts the ungrouped last", () => {
    // Last, because a new connection starts ungrouped: first would push the
    // folders someone organised down the list every time they add one.
    const groups = groupConnections([
      connection("loose"),
      connection("b", "Work"),
      connection("a", "Clients"),
    ]);

    expect(groups.map((g) => g.folder)).toEqual(["Clients", "Work", null]);
  });

  it("omits the ungrouped section when every connection is filed", () => {
    const groups = groupConnections([connection("a", "Work")]);
    expect(groups).toHaveLength(1);
    expect(groups[0]!.folder).toBe("Work");
  });

  it("folds names that differ only in case into one folder", () => {
    // Two folders that render identically would be a bug the user cannot see.
    const groups = groupConnections([
      connection("a", "Work"),
      connection("b", "work"),
      connection("c", "WORK"),
    ]);

    expect(groups).toHaveLength(1);
    // The first spelling wins: it is the one the user typed.
    expect(groups[0]!.folder).toBe("Work");
    expect(groups[0]!.connections).toHaveLength(3);
  });

  it("keeps the order connections arrived in within a folder", () => {
    const groups = groupConnections([connection("second", "Work"), connection("first", "Work")]);
    expect(groups[0]!.connections.map((c) => c.name)).toEqual(["second", "first"]);
  });

  it("returns nothing for no connections", () => {
    expect(groupConnections([])).toEqual([]);
  });
});

describe("folderNames", () => {
  it("lists each folder once, in alphabetical order", () => {
    expect(
      folderNames([
        connection("a", "Work"),
        connection("b", "clients"),
        connection("c", "work"),
        connection("d"),
      ]),
    ).toEqual(["clients", "Work"]);
  });
});
