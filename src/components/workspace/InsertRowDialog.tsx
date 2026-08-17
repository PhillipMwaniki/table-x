/**
 * Adding a row.
 *
 * A form rather than a draft row in the grid. A table with forty columns is a
 * horizontal scroll to fill in one record, and a form can say which fields the
 * server will refuse to leave empty — which is the thing that actually stops
 * an insert.
 *
 * Blank means *omitted*, not NULL. Sending an explicit NULL for every field
 * somebody skipped would override the defaults and generated keys that are the
 * reason those fields were skipped, so an untouched field is left out of the
 * statement entirely and the server decides.
 */

import { useMemo, useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Button, Input, cx } from "../ui/primitives";
import type { ColumnDef, Value } from "@/lib/types";

export function InsertRowDialog({
  open,
  table,
  columns,
  onClose,
  onInsert,
}: {
  open: boolean;
  table: string;
  columns: ColumnDef[];
  onClose: () => void;
  onInsert: (values: [string, Value][]) => void;
}) {
  /** Only the fields someone actually typed into. */
  const [entered, setEntered] = useState<Record<string, string>>({});
  /** Fields explicitly set to NULL, which is different from left blank. */
  const [nulled, setNulled] = useState<ReadonlySet<string>>(new Set());

  const required = useMemo(
    () =>
      columns.filter(
        // A column the server cannot fill in for itself: not nullable, no
        // default, and not generated.
        (c) => !c.nullable && c.default == null && !c.auto_increment,
      ),
    [columns],
  );

  const missing = required.filter((c) => !entered[c.name]?.trim() && !nulled.has(c.name));

  const submit = () => {
    const values: [string, Value][] = [];
    for (const column of columns) {
      if (nulled.has(column.name)) {
        values.push([column.name, { kind: "null" }]);
        continue;
      }
      const text = entered[column.name];
      if (text === undefined) continue;
      // Sent as text and cast by the server, the same way an edited cell is —
      // so an exact decimal reaches the column as its digits.
      values.push([column.name, { kind: "text", value: text }]);
    }
    onInsert(values);
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`New row in ${table}`}
      description="Fields left blank are omitted, so the server applies its own defaults."
      width="wide"
      footer={
        <div className="flex items-center gap-2">
          {missing.length > 0 && (
            <span className="text-[11px] text-warn">
              {missing.map((c) => c.name).join(", ")} {missing.length === 1 ? "has" : "have"} no
              default and cannot be empty
            </span>
          )}
          <div className="flex-1" />
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={missing.length > 0} onClick={submit}>
            Insert
          </Button>
        </div>
      }
    >
      <div className="max-h-[26rem] space-y-1.5 overflow-y-auto">
        {columns.map((column) => {
          const isNull = nulled.has(column.name);
          return (
            <div key={column.name} className="grid grid-cols-[11rem_1fr_auto] items-center gap-2">
              <label
                htmlFor={`field-${column.name}`}
                className="min-w-0 truncate text-right text-[11px]"
                title={`${column.name} ${column.type_name}`}
              >
                <span className="font-medium text-text">{column.name}</span>{" "}
                <span className="text-text-muted/70">{column.type_name}</span>
              </label>

              <Input
                id={`field-${column.name}`}
                value={entered[column.name] ?? ""}
                disabled={isNull}
                spellCheck={false}
                placeholder={
                  column.auto_increment
                    ? "generated"
                    : column.default != null
                      ? `default ${column.default}`
                      : column.nullable
                        ? "null"
                        : "required"
                }
                onChange={(e) => setEntered((was) => ({ ...was, [column.name]: e.target.value }))}
                className={cx(isNull && "opacity-40")}
              />

              {/* Only where NULL is a value the column can hold. Offering it
                  elsewhere would be offering a statement the server refuses. */}
              {column.nullable ? (
                <button
                  type="button"
                  onClick={() =>
                    setNulled((was) => {
                      const next = new Set(was);
                      if (!next.delete(column.name)) next.add(column.name);
                      return next;
                    })
                  }
                  className={cx(
                    "rounded border px-1.5 py-0.5 text-[10px]",
                    isNull
                      ? "border-accent bg-accent/15 text-accent"
                      : "border-border text-text-muted hover:text-text",
                  )}
                  title="Write NULL rather than leaving this out"
                >
                  null
                </button>
              ) : (
                <span className="w-9" />
              )}
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}
