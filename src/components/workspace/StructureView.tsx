/**
 * A table's shape, beside its rows.
 *
 * The backend has returned all of this since the driver contract was written —
 * columns, types, nullability, defaults, indexes, foreign keys — and nothing
 * rendered it. It is not a separate place to go: looking at a table means
 * moving between what is in it and how it is built, several times, so this
 * lives behind a toggle in the same tab rather than in a tab of its own.
 *
 * Editable, on the engines that can honestly manage it, and on the terms this
 * file used to state as a reason not to be: editing a column is a migration, and
 * a migration belongs in a statement somebody can read before it runs. So edits
 * stage rather than fire. They collect as `Change` values — the same union the
 * schema diff produces — and Review turns them into the statements the backend
 * would run, which is the click that actually runs them.
 *
 * What is offered comes from `capabilities.ddl` rather than from a list of
 * driver ids here. SQLite adds and drops columns and has no ALTER COLUMN at all;
 * ClickHouse has no foreign keys and means something else by "index". Both are
 * facts about the engine, so the driver states them and this file asks.
 */

import { useCallback, useEffect, useState } from "react";
import { Banner, Button, Spinner, cx } from "../ui/primitives";
import { ColumnForm, ForeignKeyForm, IndexForm } from "./StructureForms";
import { ReviewChangesDialog } from "./ReviewChangesDialog";
import { ipc, IpcError } from "@/lib/ipc";
import { columnDifferences, describeChange, discard, withPending } from "@/lib/structure";
import type { RowState } from "@/lib/structure";
import { useConnections } from "@/store/connections";
import type { Change, DdlSupport, TableDetail } from "@/lib/types";

/** Nothing offered, for a connection whose driver has not loaded yet. */
const NO_DDL: DdlSupport = {
  add_column: false,
  drop_column: false,
  alter_column: false,
  indexes: false,
  foreign_keys: false,
  transactional_ddl: false,
};

export function StructureView({
  connectionId,
  table,
  schema,
}: {
  connectionId: string;
  table: string;
  schema?: string | undefined;
}) {
  const [detail, setDetail] = useState<TableDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  /** Staged, not applied. Cleared by a successful apply or by discarding. */
  const [pending, setPending] = useState<Change[]>([]);
  const [adding, setAdding] = useState<null | "column" | "index" | "foreign_key">(null);
  const [editingColumn, setEditingColumn] = useState<string | null>(null);
  const [reviewing, setReviewing] = useState(false);
  const [applied, setApplied] = useState<string | null>(null);

  const connections = useConnections((s) => s.connections);
  const drivers = useConnections((s) => s.drivers);
  const connection = connections.find((c) => c.id === connectionId);
  const ddl = drivers.find((d) => d.id === connection?.driver)?.capabilities.ddl ?? NO_DDL;
  // The same flag that stops a write from the grid. A structure edit is the
  // largest write there is, so it is the last place to make an exception.
  const readOnly = connection?.read_only ?? false;
  const editable = !readOnly && Object.values(ddl).some(Boolean);

  const load = useCallback(() => {
    let cancelled = false;
    setLoading(true);

    ipc
      .tableDetail(connectionId, table, schema)
      .then((next) => {
        if (cancelled) return;
        setDetail(next);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setError((e as IpcError).message);
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [connectionId, table, schema]);

  useEffect(() => load(), [load]);

  // Staged edits belong to the table they were made against. Rather than
  // resetting them in an effect when the props change -- which costs a render
  // and a lint warning for synchronising state to state -- the call site keys
  // this component by the table, so switching to another one mounts a fresh
  // editor with nothing staged.

  const stage = (change: Change) => {
    setPending((was) => [...was, change]);
    setAdding(null);
    setEditingColumn(null);
    setApplied(null);
  };

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Spinner className="text-text-muted" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex-1 p-3">
        <Banner tone="error">{error}</Banner>
      </div>
    );
  }

  if (!detail) return null;

  // Everything below renders the table as it will be, not as it is: a staged
  // drop stays visible and struck through, a staged addition sits with the rest.
  const { detail: staged, state } = withPending(detail, pending);

  const keyed = new Set(staged.primary_key);
  // A column can be referenced by more than one key; the set is what the badge
  // asks, not how many.
  const referencing = new Set(staged.foreign_keys.flatMap((k) => k.columns));

  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <div className="space-y-4 p-3">
        <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h2 className="font-mono text-[13px] font-semibold text-text">
            {staged.schema ? `${staged.schema}.${staged.name}` : staged.name}
          </h2>
          <span className="text-[11px] text-text-muted">
            {staged.columns.length} column{staged.columns.length === 1 ? "" : "s"}
          </span>
          {staged.estimated_rows != null && (
            // Named as an estimate every time it is shown. An exact count on a
            // large table is a table scan, and a number that looks exact and is
            // not is worse than one that admits it.
            <span className="text-[11px] text-text-muted">
              ~{staged.estimated_rows.toLocaleString()} rows (estimated)
            </span>
          )}
          {staged.comment && (
            <span className="w-full text-[11px] text-text-muted italic">{staged.comment}</span>
          )}
        </header>

        {readOnly && (
          <Banner tone="info">
            This connection is marked read-only, so its structure cannot be edited here.
          </Banner>
        )}
        {applied && <Banner tone="success">{applied}</Banner>}

        {pending.length > 0 && (
          <div className="rounded border border-accent/40 bg-accent/5 p-2.5">
            <div className="flex items-center gap-2">
              <span className="text-[11.5px] font-medium text-text">
                {pending.length} pending change{pending.length === 1 ? "" : "s"}
              </span>
              <div className="flex-1" />
              <Button variant="ghost" onClick={() => setPending([])}>
                Discard all
              </Button>
              <Button variant="primary" onClick={() => setReviewing(true)}>
                Review
              </Button>
            </div>
            {/* Listed as well as shown in place, because the in-place marks say
                what a row becomes and this says what will be run. */}
            <ul className="mt-2 space-y-0.5">
              {pending.map((change, i) => (
                <li key={i} className="flex items-baseline gap-2 text-[11px] text-text-muted">
                  <span className="font-mono">{describeChange(change)}</span>
                  <button
                    type="button"
                    className="text-text-muted/60 hover:text-danger"
                    onClick={() => setPending((was) => discard(was, i))}
                    title="Discard this change"
                  >
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}

        <Section
          title="Columns"
          action={
            editable && ddl.add_column && !adding ? (
              <SectionButton onClick={() => setAdding("column")}>Add column</SectionButton>
            ) : undefined
          }
        >
          <table className="w-full border-collapse text-[length:var(--text-data)]">
            <thead>
              <tr className="text-left text-[10.5px] text-text-muted">
                <Th className="w-8 text-right">#</Th>
                <Th>Name</Th>
                <Th>Type</Th>
                <Th className="w-16">Null</Th>
                <Th>Default</Th>
                <Th>Notes</Th>
                <Th className="w-16" />
              </tr>
            </thead>
            <tbody>
              {staged.columns.map((column) => {
                const mark = state.get(`column:${column.name}`);
                return (
                  <tr key={column.name} className={cx("border-t border-border/50", rowTint(mark))}>
                    <Td className="text-right tabular-nums text-text-muted/60">{column.ordinal}</Td>
                    <Td>
                      <span className="flex items-center gap-1">
                        {keyed.has(column.name) && (
                          <span title="Primary key" className="text-accent">
                            ⚿
                          </span>
                        )}
                        <span
                          className={cx(
                            "font-medium text-text",
                            mark === "removed" && "line-through opacity-60",
                          )}
                        >
                          {column.name}
                        </span>
                        <StateMark state={mark} />
                      </span>
                    </Td>
                    <Td className="text-text-muted">{column.type_name}</Td>
                    <Td>
                      {/* The one that gets misread most, so it says the word
                        rather than showing a tick that could mean either. */}
                      <span className={column.nullable ? "text-text-muted" : "text-warn"}>
                        {column.nullable ? "null" : "not null"}
                      </span>
                    </Td>
                    <Td className="text-text-muted">
                      {column.default ?? <span className="text-text-muted/40">—</span>}
                    </Td>
                    <Td className="text-text-muted">
                      <span className="flex flex-wrap gap-1">
                        {column.auto_increment && <Tag>auto</Tag>}
                        {referencing.has(column.name) && <Tag>references</Tag>}
                        {column.comment && <span className="italic">{column.comment}</span>}
                      </span>
                    </Td>
                    <Td className="w-16 text-right">
                      {editable && mark !== "removed" && (
                        <span className="flex justify-end gap-1.5">
                          {ddl.alter_column && (
                            <RowButton
                              onClick={() => {
                                setAdding(null);
                                setEditingColumn(column.name);
                              }}
                              title={`Edit ${column.name}`}
                            >
                              edit
                            </RowButton>
                          )}
                          {ddl.drop_column && (
                            <RowButton
                              tone="error"
                              title={`Drop ${column.name}`}
                              onClick={() =>
                                stage({
                                  kind: "column_removed",
                                  table: staged.name,
                                  column: column.name,
                                })
                              }
                            >
                              drop
                            </RowButton>
                          )}
                        </span>
                      )}
                    </Td>
                  </tr>
                );
              })}
            </tbody>
          </table>

          {editingColumn && (
            <div className="mt-2">
              <ColumnForm
                existing={staged.columns.find((c) => c.name === editingColumn)}
                ordinal={staged.columns.length + 1}
                onCancel={() => setEditingColumn(null)}
                onSave={(next) => {
                  const before = detail.columns.find((c) => c.name === editingColumn);
                  const differences = before ? columnDifferences(before, next) : [];
                  // Nothing moved, so there is nothing to stage. Staging it
                  // anyway would put an ALTER in the script that changes nothing.
                  if (!before || differences.length === 0) {
                    setEditingColumn(null);
                    return;
                  }
                  stage({
                    kind: "column_changed",
                    table: staged.name,
                    column: editingColumn,
                    to: next,
                    differences,
                  });
                }}
              />
            </div>
          )}

          {adding === "column" && (
            <div className="mt-2">
              <ColumnForm
                ordinal={staged.columns.length + 1}
                onCancel={() => setAdding(null)}
                onSave={(column) => stage({ kind: "column_added", table: staged.name, column })}
              />
            </div>
          )}
        </Section>

        <Section
          title="Indexes"
          empty="No indexes."
          action={
            editable && ddl.indexes && !adding ? (
              <SectionButton onClick={() => setAdding("index")}>Add index</SectionButton>
            ) : undefined
          }
        >
          {staged.indexes.length > 0 && (
            <ul className="space-y-1">
              {staged.indexes.map((index) => {
                const mark = state.get(`index:${index.name}`);
                return (
                  <li
                    key={index.name}
                    className="flex flex-wrap items-baseline gap-2 text-[11.5px]"
                  >
                    <span
                      className={cx(
                        "font-mono text-text",
                        mark === "removed" && "line-through opacity-60",
                      )}
                    >
                      {index.name}
                    </span>
                    <StateMark state={mark} />
                    <span className="font-mono text-text-muted">({index.columns.join(", ")})</span>
                    {index.primary && <Tag tone="accent">primary</Tag>}
                    {index.unique && !index.primary && <Tag>unique</Tag>}
                    {index.method && <Tag>{index.method}</Tag>}
                    {/* The primary key's index is not an ordinary index: dropping
                      it means dropping the constraint, which is a different
                      change and not one offered here. */}
                    {editable && ddl.indexes && !index.primary && mark !== "removed" && (
                      <RowButton
                        tone="error"
                        title={`Drop ${index.name}`}
                        onClick={() =>
                          stage({ kind: "index_removed", table: staged.name, index: index.name })
                        }
                      >
                        drop
                      </RowButton>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          {adding === "index" && (
            <div className="mt-2">
              <IndexForm
                detail={staged}
                onCancel={() => setAdding(null)}
                onSave={(index) => stage({ kind: "index_added", table: staged.name, index })}
              />
            </div>
          )}
        </Section>

        <Section
          title="Foreign keys"
          empty="No foreign keys."
          action={
            editable && ddl.foreign_keys && !adding ? (
              <SectionButton onClick={() => setAdding("foreign_key")}>
                Add foreign key
              </SectionButton>
            ) : undefined
          }
        >
          {staged.foreign_keys.length > 0 && (
            <ul className="space-y-1">
              {staged.foreign_keys.map((key) => {
                const mark = state.get(`fk:${key.name}`);
                return (
                  <li key={key.name} className="flex flex-wrap items-baseline gap-2 text-[11.5px]">
                    <span
                      className={cx(
                        "font-mono text-text",
                        mark === "removed" && "line-through opacity-60",
                      )}
                    >
                      {key.columns.join(", ")}
                    </span>
                    <StateMark state={mark} />
                    <span className="text-text-muted">→</span>
                    <span className="font-mono text-text">
                      {key.referenced_schema ? `${key.referenced_schema}.` : ""}
                      {key.referenced_table} ({key.referenced_columns.join(", ")})
                    </span>
                    {/* Only when it is not the default. `NO ACTION` on every row
                      is a column of noise that hides the one row where
                      something actually cascades. */}
                    {key.on_delete && key.on_delete !== "NO ACTION" && (
                      <Tag tone="warn">on delete {key.on_delete.toLowerCase()}</Tag>
                    )}
                    {key.on_update && key.on_update !== "NO ACTION" && (
                      <Tag tone="warn">on update {key.on_update.toLowerCase()}</Tag>
                    )}
                    <span className="font-mono text-[10px] text-text-muted/50">{key.name}</span>
                    {editable && ddl.foreign_keys && mark !== "removed" && (
                      <RowButton
                        tone="error"
                        title={`Drop ${key.name}`}
                        onClick={() =>
                          stage({ kind: "foreign_key_removed", table: staged.name, key: key.name })
                        }
                      >
                        drop
                      </RowButton>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          {adding === "foreign_key" && (
            <div className="mt-2">
              <ForeignKeyForm
                detail={staged}
                onCancel={() => setAdding(null)}
                onSave={(key) => stage({ kind: "foreign_key_added", table: staged.name, key })}
              />
            </div>
          )}
        </Section>

        {/* Not offered where the engine cannot honestly do it, and said rather
            than left as an absence the user has to guess the reason for. */}
        {!readOnly && !editable && (
          <p className="text-[11px] text-text-muted/70">
            This engine's tables cannot be altered from here.
          </p>
        )}
      </div>

      {/* Mounted only while open, so each review starts from no plan rather
          than clearing the last one out of an effect. */}
      {reviewing && (
        <ReviewChangesDialog
          open
          onClose={() => setReviewing(false)}
          connectionId={connectionId}
          changes={pending}
          onApplied={(count) => {
            setApplied(`Applied ${count} statement${count === 1 ? "" : "s"}.`);
            setPending([]);
            // Refetched rather than patched from what was staged: the server is
            // what knows the result, including anything it normalised on the way
            // in -- a type it renamed, a constraint it named itself.
            load();
          }}
        />
      )}
    </div>
  );
}

function Section({
  title,
  empty,
  action,
  children,
}: {
  title: string;
  /** Shown when there is nothing, so an absence reads as an answer. */
  empty?: string;
  /** The section's own control, if the engine and connection allow one. */
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="mb-1.5 flex items-baseline gap-2">
        <h3 className="text-[10.5px] font-medium tracking-wide text-text-muted uppercase">
          {title}
        </h3>
        <div className="flex-1" />
        {action}
      </div>
      {children ?? null}
      {!children && empty && <p className="text-[11px] text-text-muted/60">{empty}</p>}
    </section>
  );
}

function SectionButton({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="rounded border border-border px-1.5 py-0.5 text-[10.5px] text-text-muted hover:border-accent hover:text-accent"
    >
      + {children}
    </button>
  );
}

function RowButton({
  onClick,
  title,
  tone,
  children,
}: {
  onClick: () => void;
  title: string;
  tone?: "error";
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={cx(
        "text-[10.5px] text-text-muted/70",
        tone === "error" ? "hover:text-danger" : "hover:text-accent",
      )}
    >
      {children}
    </button>
  );
}

/** What a staged row is about to become. */
function StateMark({ state }: { state: RowState }) {
  if (!state) return null;
  return (
    <span
      className={cx(
        "rounded px-1 text-[9.5px]",
        state === "removed" && "bg-danger/15 text-danger",
        state === "added" && "bg-accent/15 text-accent",
        state === "changed" && "bg-warn/15 text-warn",
      )}
    >
      {state}
    </span>
  );
}

/** A faint wash so a staged row reads as staged at a glance, not only by badge. */
function rowTint(state: RowState): string | false {
  return (
    (state === "removed" && "bg-danger/5") ||
    (state === "added" && "bg-accent/5") ||
    (state === "changed" && "bg-warn/5")
  );
}

function Th({ children, className }: { children?: React.ReactNode; className?: string }) {
  return <th className={cx("pb-1 pr-3 font-medium", className)}>{children}</th>;
}

function Td({ children, className }: { children?: React.ReactNode; className?: string }) {
  return <td className={cx("py-1 pr-3 align-top", className)}>{children}</td>;
}

function Tag({ children, tone }: { children: React.ReactNode; tone?: "accent" | "warn" }) {
  return (
    <span
      className={cx(
        "rounded border px-1 py-px text-[9.5px]",
        tone === "accent" && "border-accent/40 text-accent",
        tone === "warn" && "border-warn/40 text-warn",
        !tone && "border-border text-text-muted",
      )}
    >
      {children}
    </span>
  );
}
