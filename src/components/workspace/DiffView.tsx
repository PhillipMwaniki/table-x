/**
 * What two schemas disagree about, and the statements that would settle it.
 *
 * Nothing here runs. The script opens in a query tab for review, because the
 * question worth asking about generated DDL is not "are you sure" but "does
 * this say what you meant" — and only the statements themselves can answer it.
 *
 * Destructive statements are marked in both panes and counted at the top, since
 * the difference between a migration that adds a column and one that drops a
 * table is the entire difference, and it is one line in a hundred.
 */

import { useMemo, useState } from "react";
import { Banner, Button, cx } from "../ui/primitives";
import type { Change, DiffReport } from "@/lib/types";

export function DiffView({
  report,
  onOpenScript,
}: {
  report: DiffReport;
  /** Send the whole script to a query tab, where it can be read and run. */
  onOpenScript: (sql: string) => void;
}) {
  const [pane, setPane] = useState<"changes" | "script">("changes");

  const destructive = report.statements.filter((s) => s.destructive).length;

  // Grouped by table, in the order the diff produced — which is by table name,
  // so the report and the script read in the same sequence.
  const grouped = useMemo(() => {
    const out: { table: string; changes: Change[] }[] = [];
    for (const change of report.changes) {
      const last = out[out.length - 1];
      if (last && last.table === change.table) last.changes.push(change);
      else out.push({ table: change.table, changes: [change] });
    }
    return out;
  }, [report.changes]);

  const script = report.statements
    .map((s) => (s.note ? `-- ${s.note}\n${s.sql}` : s.sql))
    .join("\n\n");

  if (report.changes.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-1 px-6 text-center">
        <p className="text-[13px] text-text">No differences.</p>
        <p className="text-[11px] text-text-muted">
          {report.from} and {report.to} have the same structure.
        </p>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 py-1">
        <span className="text-[11px] text-text-muted">
          <span className="font-medium text-text">{report.from}</span> →{" "}
          <span className="font-medium text-text">{report.to}</span> ·{" "}
          {report.changes.length} change{report.changes.length === 1 ? "" : "s"}
        </span>
        {destructive > 0 && (
          <span className="rounded bg-danger/15 px-1.5 py-0.5 text-[10px] font-medium text-danger">
            {destructive} DESTRUCTIVE
          </span>
        )}

        <div className="flex-1" />

        <Button
          variant="ghost"
          className={cx("h-5", pane === "changes" && "bg-surface-3 text-text")}
          onClick={() => setPane("changes")}
        >
          Changes
        </Button>
        <Button
          variant="ghost"
          className={cx("h-5", pane === "script" && "bg-surface-3 text-text")}
          onClick={() => setPane("script")}
        >
          Script
        </Button>
        <Button variant="secondary" className="h-5" onClick={() => onOpenScript(script)}>
          Open in editor
        </Button>
      </div>

      {destructive > 0 && (
        <div className="shrink-0 px-2 pt-2">
          <Banner tone="error">
            This script drops {destructive === 1 ? "something" : `${destructive} things`}. Read it
            before running it — nothing here runs on its own.
          </Banner>
        </div>
      )}

      {pane === "changes" ? (
        <div className="min-h-0 flex-1 overflow-auto p-2">
          {grouped.map((group) => (
            <div key={group.table} className="mb-3">
              <h3 className="mb-1 font-mono text-[12px] font-medium text-text">{group.table}</h3>
              <ul className="space-y-0.5">
                {group.changes.map((change, i) => (
                  <li key={i} className="flex items-baseline gap-2 text-[11px]">
                    <Marker change={change} />
                    <span className="text-text-muted">{describe(change)}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          {report.statements.map((statement, i) => (
            <div
              key={i}
              className={cx(
                "border-b border-border/50 px-2 py-1.5",
                statement.destructive && "bg-danger/5",
              )}
            >
              {statement.note && (
                <p
                  className={cx(
                    "mb-0.5 text-[10.5px]",
                    statement.destructive ? "text-danger" : "text-text-muted",
                  )}
                >
                  {statement.note}
                </p>
              )}
              <pre
                className="whitespace-pre-wrap font-mono text-[length:var(--text-data)] text-text"
                data-selectable
              >
                {statement.sql}
              </pre>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** A one-character sign of what kind of change this is. */
function Marker({ change }: { change: Change }) {
  const [glyph, tone] = signOf(change);
  return (
    <span className={cx("w-3 shrink-0 text-center font-mono font-medium", tone)}>{glyph}</span>
  );
}

function signOf(change: Change): [string, string] {
  switch (change.kind) {
    case "table_added":
    case "column_added":
    case "index_added":
    case "foreign_key_added":
      return ["+", "text-ok"];
    case "table_removed":
    case "column_removed":
      return ["−", "text-danger"];
    case "index_removed":
    case "foreign_key_removed":
      return ["−", "text-warn"];
    default:
      return ["~", "text-warn"];
  }
}

function describe(change: Change): string {
  switch (change.kind) {
    case "table_added":
      return `new table, ${change.columns.length} column${change.columns.length === 1 ? "" : "s"}`;
    case "table_removed":
      return "table does not exist on the other side";
    case "column_added":
      return `column ${change.column.name} ${change.column.type_name}`;
    case "column_removed":
      return `column ${change.column}`;
    case "column_changed":
      return `${change.column} — ${change.differences
        .map((d) => `${d.field}: ${d.from} → ${d.to}`)
        .join(", ")}`;
    case "index_added":
      return `${change.index.unique ? "unique index" : "index"} ${change.index.name} on ${change.index.columns.join(", ")}`;
    case "index_removed":
      return `index ${change.index}`;
    case "foreign_key_added":
      return `key ${change.key.columns.join(", ")} → ${change.key.referenced_table}`;
    case "foreign_key_removed":
      return `key ${change.key}`;
    case "primary_key_changed":
      return `primary key ${change.from.join(", ") || "none"} → ${change.to.join(", ") || "none"}`;
  }
}
