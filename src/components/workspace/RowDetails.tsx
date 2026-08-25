/**
 * One row read down the page instead of across it.
 *
 * A grid is the right shape for comparing rows and the wrong one for reading a
 * single wide row: forty columns means scrolling sideways and losing which
 * column you are in. This panel takes the row under the cursor and lays it out
 * as labelled fields, which is how a form reads and how a row is usually
 * checked.
 *
 * The fields are inputs rather than text, and they write through the same
 * `onEdit` the grid's cells do, so a value corrected here goes through the same
 * statement, the same undo stack, and the same guarantees about which row it
 * matched.
 */

import { useMemo, useState } from "react";
import { cx } from "../ui/primitives";
import { byteSize } from "@/lib/editors";
import { editText, formatValue, parseEdit } from "@/lib/value";
import type { Column, Value } from "@/lib/types";

/**
 * How many fields it takes before a search box earns its place.
 *
 * Under this, the search costs a row of chrome to filter a list already visible
 * in one glance.
 */
const SEARCH_FROM = 10;

export function RowDetails({
  columns,
  row,
  rowNumber,
  editable,
  onEdit,
  onClose,
}: {
  columns: Column[];
  /** The row being shown, or null when the result is empty. */
  row: Value[] | null;
  /** The row's place in the result, 1-based, as the grid's gutter counts it. */
  rowNumber: number | null;
  editable: boolean;
  onEdit: (columnIndex: number, next: Value) => Promise<void>;
  onClose: () => void;
}) {
  const [search, setSearch] = useState("");

  const matches = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const indexes = columns.map((_, i) => i);
    if (!needle) return indexes;
    return indexes.filter((i) => columns[i]?.name.toLowerCase().includes(needle));
  }, [columns, search]);

  return (
    <aside
      aria-label="Row details"
      className="flex w-72 shrink-0 flex-col border-l border-border bg-surface-1"
    >
      <header className="flex h-7 shrink-0 items-center gap-2 border-b border-border px-2">
        <span className="text-[11px] font-semibold tracking-wide text-text-muted uppercase">
          Details
        </span>
        {rowNumber !== null && (
          <span className="font-mono text-[10px] text-text-muted/70">Row {rowNumber}</span>
        )}
        <div className="flex-1" />
        <button
          onClick={onClose}
          title="Hide details"
          aria-label="Hide details"
          className="flex size-5 items-center justify-center rounded text-[10px] text-text-muted hover:bg-surface-2 hover:text-text"
        >
          ✕
        </button>
      </header>

      {columns.length >= SEARCH_FROM && (
        <div className="shrink-0 border-b border-border px-2 py-1.5">
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search for field..."
            aria-label="Search for field"
            className={cx(
              "h-6 w-full rounded border border-border bg-surface-0 px-1.5 text-[11px]",
              "placeholder:text-text-muted/50 focus:border-accent focus:outline-none",
            )}
          />
        </div>
      )}

      {row === null ? (
        <p className="px-2 py-6 text-center text-[11px] text-text-muted">
          No row to show. Click a row to see its fields here.
        </p>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto px-2 py-1.5">
          {matches.length === 0 ? (
            <p className="py-6 text-center text-[11px] text-text-muted">No field matches.</p>
          ) : (
            matches.map((index) => {
              const column = columns[index];
              const value = row[index];
              if (!column || !value) return null;
              return (
                <Field
                  // Keyed by column rather than by position in the filtered
                  // list, so narrowing the search does not hand one field's
                  // draft to another — and by the value as well, so moving the
                  // cursor to another row builds fresh boxes rather than
                  // leaving one row's typing in front of another row's data.
                  // A remount is how React is meant to reset state on a change
                  // of subject; syncing it in an effect costs a second render
                  // and an opportunity to get the two out of step.
                  key={`${index}-${column.name}-${formatValue(value)}`}
                  column={column}
                  value={value}
                  editable={editable}
                  onCommit={(next) => onEdit(index, next)}
                />
              );
            })
          )}
        </div>
      )}
    </aside>
  );
}

/**
 * One column's label and value.
 *
 * The draft is seeded from the value once. Keeping the two in step afterwards is
 * the key's job rather than this component's: a value that changes underneath —
 * because the cursor moved, or because the edit landed — arrives as a new field
 * with a fresh box, and there is no second copy of the truth to go stale.
 */
function Field({
  column,
  value,
  editable,
  onCommit,
}: {
  column: Column;
  value: Value;
  editable: boolean;
  onCommit: (next: Value) => Promise<void>;
}) {
  const [draft, setDraft] = useState(() => editText(value));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /** What is in the database now, to tell a real edit from a no-op. */
  const settled = formatValue(value);

  // Binary has no text form to edit: the grid opens a viewer for it, and a box
  // holding "12.4 KB" that accepted typing would be a way to destroy a blob.
  const binary = value.kind === "bytes";
  const readOnly = !editable || binary;

  const commit = async () => {
    if (readOnly) return;
    const next = parseEdit(draft, value);
    // Nothing changed: no statement, and nothing on the undo stack to take back.
    if (formatValue(next) === settled && next.kind === value.kind) return;

    setSaving(true);
    setError(null);
    try {
      await onCommit(next);
    } catch (e) {
      // The typed value stays in the box. Putting the old one back would lose
      // the correction along with the reason it failed.
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mb-1.5">
      <div className="flex items-baseline gap-2">
        <label
          htmlFor={`detail-${column.name}`}
          className="min-w-0 flex-1 truncate text-[11px] text-text"
          title={column.name}
        >
          {column.name}
        </label>
        <span
          className="shrink-0 font-mono text-[10px] text-text-muted/70"
          title={column.nullable === false ? `${column.type_name}, not null` : column.type_name}
        >
          {column.type_name}
        </span>
      </div>

      <input
        id={`detail-${column.name}`}
        value={binary ? byteSize(value.value.length) : draft}
        readOnly={readOnly}
        disabled={saving}
        // NULL is an empty box with the word in the placeholder, the same
        // distinction the grid's editor makes: an empty string and NULL are
        // different values, and a box reading "NULL" could be either.
        placeholder={value.kind === "null" ? "NULL" : ""}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void commit();
          }
          if (e.key === "Escape") {
            e.preventDefault();
            setDraft(editText(value));
            setError(null);
          }
        }}
        className={cx(
          "mt-0.5 h-6 w-full rounded border bg-surface-0 px-1.5",
          "font-mono text-[calc(var(--text-data)*0.95)] outline-none",
          "placeholder:text-text-muted/50 placeholder:italic",
          error ? "border-danger" : "border-border focus:border-accent",
          readOnly && "text-text-muted",
        )}
      />

      {error && <p className="mt-0.5 text-[10px] text-danger">{error}</p>}
    </div>
  );
}
