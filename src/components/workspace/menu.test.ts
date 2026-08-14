import { describe, expect, it } from "vitest";
import { menuFor } from "./Workspace";
import type { NodeKind, SchemaNode } from "@/lib/types";

function node(kind: NodeKind, overrides: Partial<SchemaNode> = {}): SchemaNode {
  return {
    id: "app/tables/users",
    name: "users",
    kind,
    expandable: kind === "table",
    qualified: "`app`.`users`",
    ...overrides,
  };
}

const noop = () => {};
const actions = {
  onOpen: noop,
  onScript: noop,
  onNewTab: noop,
  onCopy: noop,
  onRefresh: noop,
};

function labels(...args: Parameters<typeof menuFor>) {
  return menuFor(...args).map((i) => i.label);
}

describe("menuFor", () => {
  it("offers a table everything a table can do", () => {
    const items = labels(node("table"), { driver: "mysql", tableScripts: true }, actions);

    expect(items).toContain("Open rows");
    expect(items).toContain("New tab: SELECT");
    expect(items).toContain("Show CREATE statement");
    expect(items).toContain("Truncate table…");
    expect(items).toContain("Drop table…");
  });

  it("hides the CREATE statement where the driver cannot produce one", () => {
    // PostgreSQL has no catalogue function that renders a table, and SQL
    // Server's OBJECT_DEFINITION returns nothing for one. An item that fails
    // when clicked is worse than an item that is not there.
    const items = labels(node("table"), { driver: "postgres", tableScripts: false }, actions);
    expect(items).not.toContain("Show CREATE statement");
    expect(items).toContain("Open rows");
  });

  it("does not offer to truncate a view", () => {
    // A view holds no rows of its own; the statement would be a category error.
    const items = labels(node("view"), { driver: "mysql", tableScripts: true }, actions);
    expect(items).not.toContain("Truncate table…");
    expect(items).toContain("Drop view…");
  });

  it("offers a routine its script and nothing about rows", () => {
    const items = labels(node("function"), { driver: "mysql", tableScripts: true }, actions);
    expect(items).toContain("Edit script");
    expect(items).not.toContain("Open rows");
    expect(items).not.toContain("Drop table…");
  });

  it("omits refresh for a node with no children to re-fetch", () => {
    const items = labels(node("function"), { driver: "mysql", tableScripts: true }, {
      ...actions,
      onRefresh: null,
    });
    expect(items).not.toContain("Refresh");
  });

  it("offers a folder only what applies to a folder", () => {
    // A folder is a level of the tree, not an object: it has no name worth
    // copying as SQL and nothing to open.
    const items = labels(
      node("folder", { qualified: undefined, name: "Tables" }),
      { driver: "mysql", tableScripts: true },
      actions,
    );
    expect(items).toEqual(["Refresh"]);
  });

  it("gives every menu at least one usable item or none at all", () => {
    // A menu that opens empty is a menu that should not have opened.
    for (const kind of ["table", "view", "function", "trigger"] as NodeKind[]) {
      expect(labels(node(kind), { driver: "mysql", tableScripts: true }, actions).length)
        .toBeGreaterThan(0);
    }
  });
});
