/**
 * A table's shape, beside its rows.
 *
 * The backend has returned all of this since the driver contract was written —
 * columns, types, nullability, defaults, indexes, foreign keys — and nothing
 * rendered it. It is not a separate place to go: looking at a table means
 * moving between what is in it and how it is built, several times, so this
 * lives behind a toggle in the same tab rather than in a tab of its own.
 *
 * Read-only. Editing a column is a migration, and a migration belongs in a
 * statement somebody can read before it runs — the schema diff already writes
 * those.
 */

import { useEffect, useState } from "react";
import { Banner, Spinner, cx } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { TableDetail } from "@/lib/types";

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

  useEffect(() => {
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

  const keyed = new Set(detail.primary_key);
  // A column can be referenced by more than one key; the set is what the badge
  // asks, not how many.
  const referencing = new Set(detail.foreign_keys.flatMap((k) => k.columns));

  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <div className="space-y-4 p-3">
        <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h2 className="font-mono text-[13px] font-semibold text-text">
            {detail.schema ? `${detail.schema}.${detail.name}` : detail.name}
          </h2>
          <span className="text-[11px] text-text-muted">
            {detail.columns.length} column{detail.columns.length === 1 ? "" : "s"}
          </span>
          {detail.estimated_rows != null && (
            // Named as an estimate every time it is shown. An exact count on a
            // large table is a table scan, and a number that looks exact and is
            // not is worse than one that admits it.
            <span className="text-[11px] text-text-muted">
              ~{detail.estimated_rows.toLocaleString()} rows (estimated)
            </span>
          )}
          {detail.comment && (
            <span className="w-full text-[11px] text-text-muted italic">{detail.comment}</span>
          )}
        </header>

        <Section title="Columns">
          <table className="w-full border-collapse text-[length:var(--text-data)]">
            <thead>
              <tr className="text-left text-[10.5px] text-text-muted">
                <Th className="w-8 text-right">#</Th>
                <Th>Name</Th>
                <Th>Type</Th>
                <Th className="w-16">Null</Th>
                <Th>Default</Th>
                <Th>Notes</Th>
              </tr>
            </thead>
            <tbody>
              {detail.columns.map((column) => (
                <tr key={column.name} className="border-t border-border/50">
                  <Td className="text-right tabular-nums text-text-muted/60">{column.ordinal}</Td>
                  <Td>
                    <span className="flex items-center gap-1">
                      {keyed.has(column.name) && (
                        <span title="Primary key" className="text-accent">
                          ⚿
                        </span>
                      )}
                      <span className="font-medium text-text">{column.name}</span>
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
                </tr>
              ))}
            </tbody>
          </table>
        </Section>

        <Section title="Indexes" empty="No indexes.">
          {detail.indexes.length > 0 && (
            <ul className="space-y-1">
              {detail.indexes.map((index) => (
                <li key={index.name} className="flex flex-wrap items-baseline gap-2 text-[11.5px]">
                  <span className="font-mono text-text">{index.name}</span>
                  <span className="font-mono text-text-muted">({index.columns.join(", ")})</span>
                  {index.primary && <Tag tone="accent">primary</Tag>}
                  {index.unique && !index.primary && <Tag>unique</Tag>}
                  {index.method && <Tag>{index.method}</Tag>}
                </li>
              ))}
            </ul>
          )}
        </Section>

        <Section title="Foreign keys" empty="No foreign keys.">
          {detail.foreign_keys.length > 0 && (
            <ul className="space-y-1">
              {detail.foreign_keys.map((key) => (
                <li key={key.name} className="flex flex-wrap items-baseline gap-2 text-[11.5px]">
                  <span className="font-mono text-text">{key.columns.join(", ")}</span>
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
                </li>
              ))}
            </ul>
          )}
        </Section>
      </div>
    </div>
  );
}

function Section({
  title,
  empty,
  children,
}: {
  title: string;
  /** Shown when there is nothing, so an absence reads as an answer. */
  empty?: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <h3 className="mb-1.5 text-[10.5px] font-medium tracking-wide text-text-muted uppercase">
        {title}
      </h3>
      {children ?? null}
      {!children && empty && <p className="text-[11px] text-text-muted/60">{empty}</p>}
    </section>
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
