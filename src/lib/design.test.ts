import { describe, expect, it } from "vitest";
import {
  addColumn,
  addForeignKey,
  addTable,
  freeTableName,
  removeColumn,
  removeForeignKey,
  removeTable,
  renameTable,
  togglePrimaryKey,
  updateColumn,
} from "./design";
import type { Design, TableDetail } from "./types";

function table(name: string, columns: string[] = ["id"]): TableDetail {
  return {
    name,
    columns: columns.map((c, i) => ({
      name: c,
      type_name: "int",
      nullable: true,
      auto_increment: false,
      ordinal: i,
    })),
    indexes: [],
    foreign_keys: [],
    primary_key: ["id"],
  };
}

function design(tables: TableDetail[] = [], driver = "mysql"): Design {
  return {
    id: "d1",
    name: "Shop",
    driver,
    tables,
    layout: [],
    created_at: "",
    updated_at: "",
  };
}

describe("addTable", () => {
  it("starts a table with the key most tables turn out to want", () => {
    // Tedious to type, present on the overwhelming majority of tables, and
    // quicker to delete than to add.
    const [users] = addTable(design()).tables;
    expect(users?.columns).toHaveLength(1);
    expect(users?.columns[0]?.name).toBe("id");
    expect(users?.columns[0]?.auto_increment).toBe(true);
    expect(users?.primary_key).toEqual(["id"]);
  });

  it("spells the key's type the way the design's engine spells it", () => {
    // A design is written for one engine, so there is no reason to offer a
    // lowest common denominator nobody uses.
    expect(addTable(design([], "postgres")).tables[0]?.columns[0]?.type_name).toBe("integer");
    expect(addTable(design([], "sqlite")).tables[0]?.columns[0]?.type_name).toBe("INTEGER");
  });

  it("does not collide with a table that is already there", () => {
    const twice = addTable(addTable(design()));
    expect(twice.tables.map((t) => t.name)).toEqual(["table", "table_1"]);
  });
});

describe("freeTableName", () => {
  it("ignores case when deciding what is taken", () => {
    // Engines disagree about whether Users and users are the same table; a
    // design should not produce a pair that one of them will refuse.
    expect(freeTableName(design([table("Table")]))).toBe("table_1");
  });
});

describe("renameTable", () => {
  it("follows through to every key pointing at it", () => {
    // A foreign key naming a table that no longer exists is a relation the
    // diagram cannot draw and the script cannot create.
    const orders = { ...table("orders", ["id", "user_id"]) };
    const users = table("users");
    const before = design([orders, users]);
    const withKey = {
      ...before,
      tables: [addForeignKey(orders, "user_id", users), users],
    };

    const after = renameTable(withKey, "users", "account");
    expect(after.tables.map((t) => t.name)).toContain("account");
    expect(after.tables[0]?.foreign_keys[0]?.referenced_table).toBe("account");
  });

  it("takes the table's position with it", () => {
    const moved = { ...design([table("users")]), layout: [{ table: "users", x: 5, y: 6 }] };
    expect(renameTable(moved, "users", "account").layout[0]?.table).toBe("account");
  });

  it("refuses a name that is only whitespace", () => {
    // Half-typed names arrive here on every keystroke; an empty one is not a
    // rename, it is a moment in the middle of one.
    const before = design([table("users")]);
    expect(renameTable(before, "users", "   ").tables[0]?.name).toBe("users");
  });
});

describe("removeTable", () => {
  it("takes the relations that pointed at it", () => {
    // They cannot survive it: a reference to a table that is not in the design
    // would produce a script that fails on a statement nobody can explain.
    const users = table("users");
    const orders = addForeignKey(table("orders", ["id", "user_id"]), "user_id", users);
    const after = removeTable(design([orders, users]), "users");

    expect(after.tables).toHaveLength(1);
    expect(after.tables[0]?.foreign_keys).toEqual([]);
  });
});

describe("updateColumn", () => {
  it("carries a rename into the primary key", () => {
    const users = table("users");
    const renamed = updateColumn(users, 0, { ...users.columns[0]!, name: "user_id" });
    expect(renamed.primary_key).toEqual(["user_id"]);
  });

  it("carries a rename into the keys that use the column", () => {
    const users = table("users");
    const orders = addForeignKey(table("orders", ["id", "user_id"]), "user_id", users);
    const renamed = updateColumn(orders, 1, { ...orders.columns[1]!, name: "owner_id" });
    expect(renamed.foreign_keys[0]?.columns).toEqual(["owner_id"]);
  });
});

describe("removeColumn", () => {
  it("takes the key entries that depended on it", () => {
    const users = table("users");
    const orders = addForeignKey(table("orders", ["id", "user_id"]), "user_id", users);
    const after = removeColumn(orders, 1);

    expect(after.columns.map((c) => c.name)).toEqual(["id"]);
    expect(after.foreign_keys).toEqual([]);
  });

  it("closes the gap in the ordinals", () => {
    const t = table("users", ["a", "b", "c"]);
    expect(removeColumn(t, 1).columns.map((c) => c.ordinal)).toEqual([0, 1]);
  });
});

describe("togglePrimaryKey", () => {
  it("adds and removes, keeping the order columns were chosen in", () => {
    // The order of a composite key is part of it.
    let t = table("users", ["id", "tenant_id"]);
    t = togglePrimaryKey(t, "tenant_id");
    expect(t.primary_key).toEqual(["id", "tenant_id"]);
    t = togglePrimaryKey(t, "id");
    expect(t.primary_key).toEqual(["tenant_id"]);
  });
});

describe("addForeignKey", () => {
  it("points at the target's key", () => {
    const users = table("users");
    const orders = addForeignKey(table("orders", ["id", "user_id"]), "user_id", users);
    expect(orders.foreign_keys[0]?.referenced_table).toBe("users");
    expect(orders.foreign_keys[0]?.referenced_columns).toEqual(["id"]);
  });

  it("replaces where a column pointed rather than adding a second key", () => {
    // Two keys over one column are two contradictory statements about it.
    const users = table("users");
    const teams = table("teams");
    let orders = addForeignKey(table("orders", ["id", "owner_id"]), "owner_id", users);
    orders = addForeignKey(orders, "owner_id", teams);

    expect(orders.foreign_keys).toHaveLength(1);
    expect(orders.foreign_keys[0]?.referenced_table).toBe("teams");
  });

  it("names keys after both ends, so two relations to one table can coexist", () => {
    // `author_id` and `editor_id` both against `users` is ordinary, and two
    // constraints of the same name is an error.
    const users = table("users");
    let posts = addForeignKey(table("posts", ["id", "author_id", "editor_id"]), "author_id", users);
    posts = addForeignKey(posts, "editor_id", users);
    const names = posts.foreign_keys.map((k) => k.name);
    expect(new Set(names).size).toBe(2);
  });
});

describe("removeForeignKey", () => {
  it("stops a column pointing anywhere", () => {
    const users = table("users");
    const orders = addForeignKey(table("orders", ["id", "user_id"]), "user_id", users);
    expect(removeForeignKey(orders, "user_id").foreign_keys).toEqual([]);
  });
});

describe("addColumn", () => {
  it("does not collide with a column already there", () => {
    const t = addColumn(addColumn(table("users")));
    expect(t.columns.map((c) => c.name)).toEqual(["id", "column", "column_1"]);
  });
});
