/**
 * Defining a trigger.
 *
 * Four fields and a body, which is the honest division: a name, when it fires,
 * what it fires on, and whether it fires per row are the same everywhere and
 * belong in controls; the body is a program in the engine's own language and
 * belongs in a text box. A form that tried to model the body would be a worse
 * editor than a plain box and would still not cover what people write.
 *
 * What the engine does with all this differs enough to be worth saying on
 * screen rather than discovering in a failure, so each engine's note is shown
 * beside the form.
 */

import { useState } from "react";
import { Button, Checkbox, Field, Input, Select, cx } from "../ui/primitives";
import type { TriggerDef, TriggerEvent, TriggerTiming } from "@/lib/types";

const EVENTS: { value: TriggerEvent; label: string }[] = [
  { value: "insert", label: "Insert" },
  { value: "update", label: "Update" },
  { value: "delete", label: "Delete" },
];

/**
 * What this engine will do with the trigger, in the engine's own terms.
 *
 * Shown while it is being written rather than after it fails. Two of these are
 * not caveats but different behaviour: a T-SQL trigger sees a set of rows, and
 * a PostgreSQL one becomes two objects.
 */
function noteFor(driver: string): string | null {
  switch (driver) {
    case "postgres":
      return "PostgreSQL keeps a trigger's body in a function, so this creates two objects: a function holding the body, and the trigger that calls it.";
    case "mssql":
      return "SQL Server triggers fire once per statement, not per row, and see the affected rows in the inserted and deleted tables. There is no BEFORE — only AFTER and INSTEAD OF.";
    case "mysql":
    case "mariadb":
      return "MySQL allows one trigger per timing and event on a table, and the body refers to the row as NEW and OLD.";
    case "oracle":
      return "The body is PL/SQL, and refers to the row as :NEW and :OLD.";
    case "sqlite":
      return "SQLite triggers are per row, and the body is one or more complete SQL statements.";
    default:
      return null;
  }
}

/** A trigger with nothing decided but a sensible shape. */
function blankTrigger(table: string): TriggerDef {
  return {
    name: `${table}_trigger`,
    timing: "before",
    events: ["insert"],
    for_each_row: true,
    body: "",
  };
}

export function TriggerForm({
  table,
  driver,
  existing,
  onCancel,
  onSave,
}: {
  table: string;
  /** Decides which timings are offered and what the note says. */
  driver: string;
  existing?: TriggerDef | undefined;
  onCancel: () => void;
  onSave: (trigger: TriggerDef) => void;
}) {
  const [draft, setDraft] = useState<TriggerDef>(existing ?? blankTrigger(table));
  const patch = (changes: Partial<TriggerDef>) => setDraft((d) => ({ ...d, ...changes }));

  // T-SQL has no BEFORE at all. Offering it would be offering a statement the
  // server refuses, which is the thing capabilities exist to prevent.
  const timings: { value: TriggerTiming; label: string }[] =
    driver === "mssql"
      ? [
          { value: "after", label: "After" },
          { value: "instead_of", label: "Instead of" },
        ]
      : [
          { value: "before", label: "Before" },
          { value: "after", label: "After" },
          { value: "instead_of", label: "Instead of" },
        ];

  // What the select shows, and what gets saved. A blank trigger starts on
  // BEFORE, which SQL Server does not have: showing "After" while saving
  // "before" would emit the one statement this list exists to prevent, so the
  // fallback is the value, not just the display.
  const timing: TriggerTiming = timings.some((t) => t.value === draft.timing)
    ? draft.timing
    : (timings[0]?.value ?? "after");

  const perRow = driver !== "mssql";
  const note = noteFor(driver);
  const invalid = !draft.name.trim() || draft.events.length === 0 || !draft.body.trim();

  // Saving an edit that changes nothing is not free: on MySQL and SQLite there
  // is no statement that redefines a trigger, so the migration drops it and
  // writes it again — a real risk taken for no difference.
  const unchanged =
    existing != null &&
    existing.timing === timing &&
    existing.for_each_row === (perRow && draft.for_each_row) &&
    existing.body === draft.body &&
    existing.events.length === draft.events.length &&
    existing.events.every((e) => draft.events.includes(e));

  const toggleEvent = (event: TriggerEvent) =>
    patch({
      events: draft.events.includes(event)
        ? draft.events.filter((e) => e !== event)
        : [...draft.events, event],
    });

  return (
    <div className="rounded border border-accent/40 bg-surface-1 p-2">
      <div className="grid grid-cols-[1fr_auto] gap-2">
        {/* Fixed while editing. A trigger is found by name on every engine
            that can redefine one, so a new name is not an edit at all -- it is
            a second trigger beside the first, which the drop button and this
            form already do in the order that works. */}
        <Field label="Name">
          <Input
            autoFocus={!existing}
            value={draft.name}
            spellCheck={false}
            disabled={Boolean(existing)}
            title={
              existing ? "A trigger is renamed by dropping it and writing another." : undefined
            }
            onChange={(e) => patch({ name: e.target.value })}
          />
        </Field>
        <Field label="Fires">
          <Select
            value={timing}
            onChange={(e) => patch({ timing: e.target.value as TriggerTiming })}
          >
            {timings.map((t) => (
              <option key={t.value} value={t.value}>
                {t.label}
              </option>
            ))}
          </Select>
        </Field>
      </div>

      <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1">
        <span className="text-[10.5px] text-text-muted">On</span>
        {EVENTS.map((event) => (
          <Checkbox
            key={event.value}
            label={event.label}
            checked={draft.events.includes(event.value)}
            onChange={() => toggleEvent(event.value)}
          />
        ))}
        {perRow && (
          <Checkbox
            label="For each row"
            checked={draft.for_each_row}
            onChange={(for_each_row) => patch({ for_each_row })}
          />
        )}
      </div>

      <div className="mt-2">
        <label htmlFor="trigger-body" className="block text-[10.5px] text-text-muted">
          Body
        </label>
        <textarea
          id="trigger-body"
          // Where the cursor belongs when editing: the name is fixed, and the
          // body is what somebody opened this to change.
          autoFocus={Boolean(existing)}
          value={draft.body}
          spellCheck={false}
          rows={6}
          placeholder={driver === "oracle" ? ":NEW.updated_at := SYSDATE;" : "-- statements"}
          onChange={(e) => patch({ body: e.target.value })}
          className={cx(
            "mt-0.5 w-full rounded border border-border bg-surface-0 p-1.5",
            "font-mono text-[length:var(--text-data)] outline-none focus:border-accent",
          )}
        />
      </div>

      {note && <p className="mt-1 text-[10.5px] text-text-muted">{note}</p>}

      <div className="mt-2 flex justify-end gap-2">
        <Button variant="ghost" className="h-6" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          variant="primary"
          className="h-6"
          disabled={invalid || unchanged}
          onClick={() =>
            onSave({
              ...draft,
              name: draft.name.trim(),
              timing,
              // Not shown where the engine has no per-row form, so not claimed
              // either: a T-SQL trigger fires once per statement.
              for_each_row: perRow && draft.for_each_row,
            })
          }
        >
          {existing ? "Save" : "Add"}
        </Button>
      </div>
    </div>
  );
}
