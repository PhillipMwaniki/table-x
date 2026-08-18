/**
 * The three small forms the structure editor stages changes with.
 *
 * Each one hands back a finished value and knows nothing about pending state or
 * DDL — staging is the view's job, rendering the statement is the backend's.
 *
 * Column names are chosen from the table's own columns wherever a name is
 * required rather than typed. A foreign key naming a column that does not exist
 * is a mistake the form can prevent outright, and preventing it is better than
 * reporting it after a round trip to the server.
 */

import { useState } from "react";
import { Button, Checkbox, Field, Input, cx } from "../ui/primitives";
import type { ColumnDef, ForeignKeyDef, IndexDef, TableDetail } from "@/lib/types";
import { blankColumn } from "@/lib/structure";

/** Shared shell: a bordered panel with a save/cancel pair. */
function FormPanel({
  title,
  onCancel,
  onSave,
  saveLabel,
  disabled,
  children,
}: {
  title: string;
  onCancel: () => void;
  onSave: () => void;
  saveLabel: string;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded border border-accent/40 bg-surface-2 p-2.5">
      <h4 className="mb-2 text-[10.5px] font-medium tracking-wide text-text-muted uppercase">
        {title}
      </h4>
      <div className="space-y-2">{children}</div>
      <div className="mt-2.5 flex items-center gap-2">
        <div className="flex-1" />
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="primary" onClick={onSave} disabled={disabled}>
          {saveLabel}
        </Button>
      </div>
    </div>
  );
}

/**
 * Add a column, or change one that exists.
 *
 * The name is fixed when editing. Renaming is a different operation with
 * different risks — the backend's diff deliberately reports a rename as a drop
 * and an add because a catalog cannot tell them apart — so offering it in the
 * field that also changes a type would quietly turn one into the other.
 */
export function ColumnForm({
  existing,
  ordinal,
  onCancel,
  onSave,
}: {
  existing?: ColumnDef | undefined;
  ordinal: number;
  onCancel: () => void;
  onSave: (column: ColumnDef) => void;
}) {
  const [draft, setDraft] = useState<ColumnDef>(existing ?? blankColumn(ordinal));
  const patch = (changes: Partial<ColumnDef>) => setDraft((d) => ({ ...d, ...changes }));

  const invalid = !draft.name.trim() || !draft.type_name.trim();

  return (
    <FormPanel
      title={existing ? `Edit ${existing.name}` : "Add column"}
      onCancel={onCancel}
      onSave={() =>
        onSave({ ...draft, name: draft.name.trim(), type_name: draft.type_name.trim() })
      }
      saveLabel={existing ? "Stage change" : "Stage column"}
      disabled={invalid}
    >
      <div className="grid grid-cols-2 gap-2">
        <Field label="Name">
          <Input
            value={draft.name}
            onChange={(e) => patch({ name: e.target.value })}
            // Renaming is a drop and an add to every catalog, so it is not this
            // form's job -- see the note above.
            disabled={Boolean(existing)}
            placeholder="note"
            spellCheck={false}
            autoFocus={!existing}
          />
        </Field>
        <Field label="Type" hint="Written through as typed, in this engine's own spelling.">
          <Input
            value={draft.type_name}
            onChange={(e) => patch({ type_name: e.target.value })}
            placeholder="text"
            spellCheck={false}
            autoFocus={Boolean(existing)}
          />
        </Field>
      </div>

      <Field
        label="Default"
        hint="An SQL expression, quoted as SQL needs it. Leave blank for none."
      >
        <Input
          value={draft.default ?? ""}
          onChange={(e) => patch({ default: e.target.value || undefined })}
          placeholder="'' or 0 or now()"
          spellCheck={false}
        />
      </Field>

      <Checkbox
        label="Nullable"
        hint="Making an existing column NOT NULL fails if any row holds a null."
        checked={draft.nullable}
        onChange={(nullable) => patch({ nullable })}
      />
    </FormPanel>
  );
}

/** Pick columns of this table, in the order they are ticked. */
function ColumnPicker({
  columns,
  chosen,
  onChange,
}: {
  columns: ColumnDef[];
  chosen: string[];
  onChange: (next: string[]) => void;
}) {
  return (
    <div className="flex flex-wrap gap-1">
      {columns.map((column) => {
        const at = chosen.indexOf(column.name);
        const on = at >= 0;
        return (
          <button
            key={column.name}
            type="button"
            onClick={() =>
              onChange(on ? chosen.filter((c) => c !== column.name) : [...chosen, column.name])
            }
            className={cx(
              "rounded border px-1.5 py-0.5 font-mono text-[11px]",
              on ? "border-accent bg-accent/10 text-accent" : "border-border text-text-muted",
            )}
          >
            {/* The order is the index's order and it matters, so a multi-column
                pick shows its position rather than just that it is on. */}
            {on && chosen.length > 1 && <span className="mr-1 tabular-nums">{at + 1}</span>}
            {column.name}
          </button>
        );
      })}
    </div>
  );
}

export function IndexForm({
  detail,
  onCancel,
  onSave,
}: {
  detail: TableDetail;
  onCancel: () => void;
  onSave: (index: IndexDef) => void;
}) {
  const [columns, setColumns] = useState<string[]>([]);
  const [unique, setUnique] = useState(false);
  // Suggested from the columns, the way most engines name one themselves, but
  // editable because plenty of schemas have their own convention.
  const [name, setName] = useState("");
  const suggested = columns.length ? `${detail.name}_${columns.join("_")}_idx` : "";
  const effective = name.trim() || suggested;

  return (
    <FormPanel
      title="Add index"
      onCancel={onCancel}
      onSave={() => onSave({ name: effective, columns, unique, primary: false })}
      saveLabel="Stage index"
      disabled={columns.length === 0 || !effective}
    >
      <Field label="Columns" hint="Ticked in the order the index uses them.">
        <ColumnPicker columns={detail.columns} chosen={columns} onChange={setColumns} />
      </Field>
      <Field label="Name">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={suggested || "index name"}
          spellCheck={false}
        />
      </Field>
      <Checkbox
        label="Unique"
        hint="Fails to build if the existing rows already hold a duplicate."
        checked={unique}
        onChange={setUnique}
      />
    </FormPanel>
  );
}

export function ForeignKeyForm({
  detail,
  onCancel,
  onSave,
}: {
  detail: TableDetail;
  onCancel: () => void;
  onSave: (key: ForeignKeyDef) => void;
}) {
  const [columns, setColumns] = useState<string[]>([]);
  const [referencedTable, setReferencedTable] = useState("");
  const [referencedColumns, setReferencedColumns] = useState("");
  const [name, setName] = useState("");

  const target = referencedColumns
    .split(",")
    .map((c) => c.trim())
    .filter(Boolean);

  const suggested = columns.length ? `${detail.name}_${columns.join("_")}_fkey` : "";
  const effective = name.trim() || suggested;
  // A foreign key pairs its columns positionally, so a mismatch is not a
  // preference -- the statement would be rejected.
  const mismatched = columns.length > 0 && target.length !== columns.length;

  return (
    <FormPanel
      title="Add foreign key"
      onCancel={onCancel}
      onSave={() =>
        onSave({
          name: effective,
          columns,
          ...(detail.schema ? { referenced_schema: detail.schema } : {}),
          referenced_table: referencedTable.trim(),
          referenced_columns: target,
        })
      }
      saveLabel="Stage foreign key"
      disabled={columns.length === 0 || !referencedTable.trim() || mismatched || !effective}
    >
      <Field label="Columns in this table">
        <ColumnPicker columns={detail.columns} chosen={columns} onChange={setColumns} />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="References table">
          <Input
            value={referencedTable}
            onChange={(e) => setReferencedTable(e.target.value)}
            placeholder="users"
            spellCheck={false}
          />
        </Field>
        <Field
          label="Its columns"
          hint="Comma separated, paired in order with the columns above."
          error={mismatched ? `Needs ${columns.length} to match` : undefined}
        >
          <Input
            value={referencedColumns}
            onChange={(e) => setReferencedColumns(e.target.value)}
            placeholder="id"
            spellCheck={false}
          />
        </Field>
      </div>
      <Field label="Name">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={suggested || "constraint name"}
          spellCheck={false}
        />
      </Field>
    </FormPanel>
  );
}
