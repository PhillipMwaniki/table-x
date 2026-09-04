import { describe, expect, it } from "vitest";
import { insertValues, nowText, seedFields, temporalKind } from "./insert";
import type { ColumnDef } from "./types";

function column(name: string, overrides: Partial<ColumnDef> = {}): ColumnDef {
  return {
    name,
    type_name: "text",
    nullable: true,
    auto_increment: false,
    ordinal: 0,
    ...overrides,
  };
}

const key = column("iditinerary_meta", {
  type_name: "int",
  nullable: false,
  auto_increment: true,
});
const columns = [key, column("monthbill"), column("current_meta"), column("cms_value")];

describe("seedFields", () => {
  it("leaves the generated key out of a duplicate", () => {
    // Carrying it over is a duplicate key error every time, so all it gives the
    // user is a field to clear before the form will submit.
    const { entered } = seedFields(columns, {
      iditinerary_meta: "41",
      monthbill: "March",
      current_meta: "1200",
    });
    expect(entered).toEqual({ monthbill: "March", current_meta: "1200" });
  });

  it("carries a NULL across as a NULL rather than as a blank", () => {
    const { entered, nulled } = seedFields(columns, { monthbill: null, current_meta: "7" });
    expect(nulled).toEqual(["monthbill"]);
    expect(entered).toEqual({ current_meta: "7" });
  });

  it("skips columns the row said nothing about", () => {
    // A SELECT of three columns out of twenty says nothing about the other
    // seventeen; the server's defaults are a better answer than empty.
    const { entered } = seedFields(columns, { monthbill: "March" });
    expect(entered).toEqual({ monthbill: "March" });
  });

  it("starts empty for a new row rather than a duplicated one", () => {
    expect(seedFields(columns, undefined)).toEqual({ entered: {}, nulled: [] });
  });
});

describe("insertValues", () => {
  it("omits a field that was cleared rather than sending an empty string", () => {
    // The reported failure: an emptied generated key reached MySQL as '' and it
    // refused the statement -- "Incorrect integer value: '' for column
    // 'iditinerary_meta'". Omitted, the server generates one.
    const values = insertValues(columns, { iditinerary_meta: "", monthbill: "March" }, new Set());
    expect(values).toEqual([["monthbill", { kind: "text", value: "March" }]]);
  });

  it("treats whitespace as blank, the same way the missing-field check does", () => {
    expect(insertValues(columns, { monthbill: "   " }, new Set())).toEqual([]);
  });

  it("sends an explicit NULL for a field set to NULL", () => {
    const values = insertValues(columns, {}, new Set(["monthbill"]));
    expect(values).toEqual([["monthbill", { kind: "null" }]]);
  });

  it("prefers NULL over whatever text the field held", () => {
    // The NULL toggle is the later decision: it disables the box rather than
    // clearing it, so the text is still there and must not win.
    const values = insertValues(columns, { monthbill: "March" }, new Set(["monthbill"]));
    expect(values).toEqual([["monthbill", { kind: "null" }]]);
  });

  it("keeps the values in the table's column order", () => {
    const values = insertValues(
      columns,
      { cms_value: "3", monthbill: "March", current_meta: "2" },
      new Set(),
    );
    expect(values.map(([name]) => name)).toEqual(["monthbill", "current_meta", "cms_value"]);
  });
});

describe("temporalKind", () => {
  it("recognises the clock families by their catalog names", () => {
    expect(temporalKind("datetime")).toBe("datetime");
    expect(temporalKind("timestamp")).toBe("datetime");
    expect(temporalKind("timestamp without time zone")).toBe("datetime");
    expect(temporalKind("timestamptz")).toBe("datetime");
    expect(temporalKind("datetime2(3)")).toBe("datetime");
    expect(temporalKind("smalldatetime")).toBe("datetime");
    expect(temporalKind("DATE")).toBe("date");
    expect(temporalKind("time")).toBe("time");
    expect(temporalKind("time with time zone")).toBe("time");
  });

  it("says nothing about a column that stores no clock", () => {
    expect(temporalKind("int unsigned")).toBeNull();
    expect(temporalKind("varchar(255)")).toBeNull();
    expect(temporalKind("interval")).toBeNull();
    expect(temporalKind("year")).toBeNull();
  });
});

describe("nowText", () => {
  const at = new Date(2026, 7, 3, 9, 2, 44); // local time, months are 0-based

  it("writes the moment the way the column expects it", () => {
    expect(nowText("datetime", at)).toBe("2026-08-03 09:02:44");
    expect(nowText("date", at)).toBe("2026-08-03");
    expect(nowText("time", at)).toBe("09:02:44");
  });

  it("pads every part to two digits", () => {
    expect(nowText("datetime", new Date(2026, 0, 5, 3, 4, 5))).toBe("2026-01-05 03:04:05");
  });
});
