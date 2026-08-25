/**
 * Defining a table before it exists.
 *
 * The same editor a design uses, pointed at a live schema. A table being drawn
 * on a canvas and a table about to be created are the same thing described the
 * same way — columns, types, a key — so they get the same form rather than two
 * that drift apart on what a column is.
 *
 * What happens to it afterwards is the difference. A design keeps the table as
 * a document; here it becomes a `table_added` change and goes through the
 * review the structure editor already uses: the statement is shown, and Apply is
 * the second click rather than the first.
 */

import { useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Banner, Button } from "../ui/primitives";
import { TableInspector } from "./TableInspector";
import { ReviewChangesDialog } from "./ReviewChangesDialog";
import { addColumn } from "@/lib/design";
import type { Change, TableDetail } from "@/lib/types";

/** A table with the key most tables turn out to want. */
function starter(schema: string | undefined, keyType: string): TableDetail {
  return {
    schema,
    name: "",
    columns: [
      {
        name: "id",
        type_name: keyType,
        nullable: false,
        auto_increment: true,
        ordinal: 0,
      },
    ],
    indexes: [],
    foreign_keys: [],
    primary_key: ["id"],
  };
}

export function CreateTableDialog({
  open,
  connectionId,
  schema,
  types,
  keyType,
  onClose,
  onCreated,
}: {
  open: boolean;
  connectionId: string;
  /** The schema the table is being created in, where the engine has them. */
  schema?: string | undefined;
  /** The engine's column types, offered as suggestions. */
  types: string[];
  /** How this engine spells a generated integer key. */
  keyType: string;
  onClose: () => void;
  /** Created — the caller refreshes the tree and says so. */
  onCreated: (name: string) => void;
}) {
  const [table, setTable] = useState<TableDetail>(() => starter(schema, keyType));
  const [reviewing, setReviewing] = useState(false);

  const name = table.name.trim();
  const problem =
    !name
      ? "A table needs a name."
      : table.columns.length === 0
        ? "A table needs at least one column."
        : table.columns.some((c) => !c.name.trim())
          ? "Every column needs a name."
          : table.columns.some((c) => !c.type_name.trim())
            ? "Every column needs a type."
            : null;

  // The change the migration writer turns into a CREATE TABLE. Built here and
  // written there, so the statement that runs is produced by the same code that
  // produces every other statement this app runs.
  const changes: Change[] = [
    {
      kind: "table_added",
      table: name,
      columns: table.columns.map((column, ordinal) => ({ ...column, ordinal })),
      primary_key: table.primary_key,
    },
  ];

  return (
    <>
      <Dialog
        open={open && !reviewing}
        onClose={onClose}
        title={schema ? `New table in ${schema}` : "New table"}
        description="Nothing runs until you have read the statement."
        width="wide"
        footer={
          <div className="flex items-center gap-2">
            {problem && <span className="text-[11px] text-warn">{problem}</span>}
            <div className="flex-1" />
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={Boolean(problem)}
              onClick={() => setReviewing(true)}
            >
              Review SQL…
            </Button>
          </div>
        }
      >
        <div className="space-y-2">
          <Banner tone="info">
            Foreign keys are added after the table exists, from its structure view — a
            constraint cannot be written against a table that is not there yet.
          </Banner>

          {/* No relation targets: see the note above. The inspector hides that
              control when there is nothing to point at. */}
          <div className="-mx-2 flex max-h-[24rem] overflow-hidden rounded border border-border">
            <TableInspector
              table={table}
              tables={[]}
              types={types}
              onChange={setTable}
              onRename={(next) => setTable((was) => ({ ...was, name: next }))}
              onRemove={onClose}
              onClose={onClose}
            />
          </div>

          <button
            onClick={() => setTable(addColumn(table))}
            className="text-[11px] text-accent hover:underline"
          >
            Add another column
          </button>
        </div>
      </Dialog>

      {reviewing && (
        <ReviewChangesDialog
          open
          connectionId={connectionId}
          changes={changes}
          onClose={() => setReviewing(false)}
          onApplied={() => {
            setReviewing(false);
            onCreated(name);
            // Reset, so opening the dialog again starts a table rather than
            // the one just created.
            setTable(starter(schema, keyType));
          }}
        />
      )}
    </>
  );
}
