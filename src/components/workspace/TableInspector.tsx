/**
 * The table selected on a design canvas, as a form.
 *
 * A box on a canvas is the right place to see a table and the wrong place to
 * type into one: the rows are sixteen pixels tall and three of the four things
 * worth changing about a column do not fit on a line that also has to be read
 * from across the diagram. So the canvas shows, and this edits.
 *
 * Everything here changes the document and nothing runs. A design is turned
 * into SQL when somebody asks for it, and not before — which is what makes it
 * safe to leave a table half-defined while thinking about the next one.
 */

import { Button, Checkbox, Input, Select, cx } from "../ui/primitives";
import {
  addColumn,
  addForeignKey,
  removeColumn,
  removeForeignKey,
  togglePrimaryKey,
  updateColumn,
} from "@/lib/design";
import type { TableDetail } from "@/lib/types";

/** Types offered first, being the ones most columns turn out to be. */
const COMMON_TYPES = [
  "int",
  "bigint",
  "varchar(255)",
  "text",
  "boolean",
  "date",
  "timestamp",
  "decimal(10,2)",
  "json",
  "uuid",
];

export function TableInspector({
  table,
  tables,
  onChange,
  onRename,
  onRemove,
  onClose,
}: {
  table: TableDetail;
  /** Every table in the design, as the targets a relation can point at. */
  tables: TableDetail[];
  onChange: (next: TableDetail) => void;
  /** Renaming reaches wider than the table, so the design handles it. */
  onRename: (name: string) => void;
  onRemove: () => void;
  onClose: () => void;
}) {
  /** Where a column points, if anywhere. */
  const referenceOf = (column: string) =>
    table.foreign_keys.find((k) => k.columns.includes(column));

  return (
    <aside
      aria-label={`Table ${table.name}`}
      className="flex w-80 shrink-0 flex-col border-l border-border bg-surface-1"
    >
      <header className="flex h-7 shrink-0 items-center gap-2 border-b border-border px-2">
        <span className="text-[11px] font-semibold tracking-wide text-text-muted uppercase">
          Table
        </span>
        <div className="flex-1" />
        <button
          onClick={onRemove}
          className="rounded px-1 text-[10.5px] text-text-muted hover:text-danger"
          title="Remove this table from the design"
        >
          Delete
        </button>
        <button
          onClick={onClose}
          aria-label="Close"
          className="flex size-5 items-center justify-center rounded text-[10px] text-text-muted hover:bg-surface-2 hover:text-text"
        >
          ✕
        </button>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        <label className="block text-[10.5px] text-text-muted">Name</label>
        <Input
          value={table.name}
          spellCheck={false}
          onChange={(e) => onRename(e.target.value)}
          className="mb-3"
        />

        <div className="mb-1 flex items-center gap-2">
          <span className="text-[10.5px] text-text-muted">Columns</span>
          <div className="flex-1" />
          <Button variant="ghost" className="h-5" onClick={() => onChange(addColumn(table))}>
            + Column
          </Button>
        </div>

        {table.columns.length === 0 && (
          <p className="py-3 text-center text-[11px] text-text-muted">
            No columns yet. A table needs at least one before it can be created.
          </p>
        )}

        <ul className="space-y-2">
          {table.columns.map((column, index) => {
            const reference = referenceOf(column.name);
            return (
              <li
                key={index}
                className={cx(
                  "rounded border p-1.5",
                  table.primary_key.includes(column.name)
                    ? "border-accent/40 bg-accent/5"
                    : "border-border",
                )}
              >
                <div className="flex items-center gap-1.5">
                  <Input
                    value={column.name}
                    spellCheck={false}
                    aria-label="Column name"
                    onChange={(e) =>
                      onChange(updateColumn(table, index, { ...column, name: e.target.value }))
                    }
                    className="h-6 min-w-0 flex-1"
                  />
                  <button
                    onClick={() => onChange(removeColumn(table, index))}
                    aria-label={`Remove ${column.name}`}
                    className="rounded px-1 text-[11px] text-text-muted hover:text-danger"
                  >
                    ✕
                  </button>
                </div>

                <div className="mt-1 flex items-center gap-1.5">
                  {/* A list of the usual types and a box to type any other:
                      offering only a list would make every engine's own types
                      unreachable, and offering only a box makes the common case
                      typing. */}
                  <Input
                    value={column.type_name}
                    spellCheck={false}
                    aria-label="Type"
                    list="design-types"
                    onChange={(e) =>
                      onChange(updateColumn(table, index, { ...column, type_name: e.target.value }))
                    }
                    className="h-6 min-w-0 flex-1 font-mono"
                  />
                </div>

                <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
                  <Checkbox
                    label="Key"
                    checked={table.primary_key.includes(column.name)}
                    onChange={() => onChange(togglePrimaryKey(table, column.name))}
                  />
                  <Checkbox
                    label="Null"
                    checked={column.nullable}
                    onChange={(nullable) =>
                      onChange(updateColumn(table, index, { ...column, nullable }))
                    }
                  />
                  <Checkbox
                    label="Generated"
                    checked={column.auto_increment}
                    onChange={(auto_increment) =>
                      onChange(updateColumn(table, index, { ...column, auto_increment }))
                    }
                  />
                </div>

                {/* The relationship, chosen rather than drawn. Dragging a line
                    between two boxes is the gesture people know from Workbench
                    and it is coming; picking the table is the same statement in
                    the meantime, and it is reachable from a keyboard. */}
                <div className="mt-1 flex items-center gap-1.5">
                  <span className="text-[10.5px] text-text-muted">→</span>
                  <Select
                    value={reference?.referenced_table ?? ""}
                    aria-label={`What ${column.name} references`}
                    onChange={(e) => {
                      const target = tables.find((t) => t.name === e.target.value);
                      onChange(
                        target
                          ? addForeignKey(table, column.name, target)
                          : removeForeignKey(table, column.name),
                      );
                    }}
                    className="h-6 min-w-0 flex-1 text-[11px]"
                  >
                    <option value="">references nothing</option>
                    {tables.map((t) => (
                      <option key={t.name} value={t.name}>
                        {t.name}
                        {t.primary_key[0] ? `.${t.primary_key[0]}` : ""}
                      </option>
                    ))}
                  </Select>
                </div>
              </li>
            );
          })}
        </ul>

        <datalist id="design-types">
          {COMMON_TYPES.map((t) => (
            <option key={t} value={t} />
          ))}
        </datalist>
      </div>
    </aside>
  );
}
