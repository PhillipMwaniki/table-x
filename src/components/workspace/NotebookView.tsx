/**
 * Prose and queries in one document.
 *
 * A snippet is a statement worth keeping; a notebook is the reasoning around
 * several of them — why this query, what its result showed, what to run next.
 * That reasoning is the part that gets lost, and a tool that stores only the
 * SQL stores the least valuable half.
 *
 * Results are held in memory and never saved. A notebook records what to run
 * and why; a stored result would be a claim about a database that may have been
 * true a month ago, presented as though it were current.
 *
 * Every run goes through the same guard the query tabs use, so a `DELETE`
 * inside a notebook cell is confirmed exactly as it would be anywhere else.
 */

import { useCallback, useState } from "react";
import { Button, Spinner } from "../ui/primitives";
import { Markdown } from "./Markdown";
import { ResultGrid } from "./ResultGrid";
import type { IpcError } from "@/lib/ipc";
import type { NotebookCell, StatementResult } from "@/lib/types";

/** What a cell knows after it has been run. Never persisted. */
interface CellState {
  running: boolean;
  result: StatementResult | null;
  error: string | null;
  elapsedMs: number | null;
}

const BLANK: CellState = { running: false, result: null, error: null, elapsedMs: null };

let counter = 0;
function nextId(): string {
  counter += 1;
  return `cell-${Date.now().toString(36)}-${counter}`;
}

export function NotebookView({
  cells,
  saved,
  onChange,
  onRunGuarded,
  onSave,
}: {
  cells: NotebookCell[];
  /** Whether this has been kept before, which decides what Save means. */
  saved: boolean;
  onChange: (cells: NotebookCell[]) => void;
  onSave: () => void;
  /**
   * Run one statement through the workspace's own gate.
   *
   * Passed in rather than called directly so a notebook cannot become the one
   * route that skips the destructive-statement confirmation.
   */
  onRunGuarded: (sql: string) => Promise<StatementResult | null>;
}) {
  const [states, setStates] = useState<Record<string, CellState>>({});
  /** Cells being edited. A markdown cell renders until you ask to change it. */
  const [editing, setEditing] = useState<Set<string>>(new Set());

  const stateOf = (id: string) => states[id] ?? BLANK;

  const patch = (id: string, changes: Partial<CellState>) =>
    setStates((was) => ({ ...was, [id]: { ...(was[id] ?? BLANK), ...changes } }));

  const setSource = (id: string, source: string) =>
    onChange(cells.map((cell) => (cell.id === id ? { ...cell, source } : cell)));

  const addCell = (kind: NotebookCell["kind"], after?: number) => {
    const cell: NotebookCell = { id: nextId(), kind, source: "" };
    const next = [...cells];
    next.splice(after === undefined ? cells.length : after + 1, 0, cell);
    onChange(next);
    // A new cell opens in edit mode, since an empty rendered note shows nothing
    // to click on.
    setEditing((was) => new Set(was).add(cell.id));
  };

  const removeCell = (id: string) => onChange(cells.filter((cell) => cell.id !== id));

  const move = (index: number, by: number) => {
    const target = index + by;
    if (target < 0 || target >= cells.length) return;
    const next = [...cells];
    const [cell] = next.splice(index, 1);
    if (cell) next.splice(target, 0, cell);
    onChange(next);
  };

  const runCell = useCallback(
    async (cell: NotebookCell) => {
      if (!cell.source.trim()) return;
      patch(cell.id, { running: true, error: null });
      const started = performance.now();
      try {
        const result = await onRunGuarded(cell.source);
        patch(cell.id, {
          running: false,
          result,
          elapsedMs: Math.round(performance.now() - started),
        });
      } catch (e) {
        patch(cell.id, {
          running: false,
          result: null,
          error: (e as IpcError).message,
          elapsedMs: null,
        });
      }
    },
    [onRunGuarded],
  );

  const runAll = async () => {
    // Sequentially, and stopping at the first failure: the cells after a broken
    // one usually depend on it, and running them produces a screen of errors
    // that all have the same cause.
    for (const cell of cells) {
      if (cell.kind !== "sql") continue;
      await runCell(cell);
      if (stateOf(cell.id).error) break;
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 text-[11px]">
        <Button variant="ghost" className="h-5" onClick={() => void runAll()}>
          Run all
        </Button>
        <span className="text-text-muted">
          {cells.filter((c) => c.kind === "sql").length} queries ·{" "}
          {cells.filter((c) => c.kind === "markdown").length} notes
        </span>
        <div className="flex-1" />
        <span className="text-text-muted/70">Results are not saved with the notebook.</span>
        <Button variant="ghost" className="h-5" onClick={onSave}>
          {saved ? "Save" : "Save as…"}
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-3">
        <div className="mx-auto max-w-4xl space-y-2">
          {cells.length === 0 && (
            <p className="py-8 text-center text-[12px] text-text-muted">
              An empty notebook. Add a note to say what you are looking into, or a query to
              start looking.
            </p>
          )}

          {cells.map((cell, index) => {
            const state = stateOf(cell.id);
            const isEditing = editing.has(cell.id) || cell.kind === "sql";

            return (
              <div
                key={cell.id}
                className="group rounded-md border border-transparent p-1 hover:border-border"
              >
                <div className="mb-1 flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <span className="font-mono text-[10px] text-text-muted">
                    {cell.kind === "sql" ? "SQL" : "note"}
                  </span>
                  <div className="flex-1" />
                  {cell.kind === "markdown" && (
                    <CellButton
                      onClick={() =>
                        setEditing((was) => {
                          const next = new Set(was);
                          if (!next.delete(cell.id)) next.add(cell.id);
                          return next;
                        })
                      }
                    >
                      {editing.has(cell.id) ? "Done" : "Edit"}
                    </CellButton>
                  )}
                  <CellButton onClick={() => move(index, -1)}>↑</CellButton>
                  <CellButton onClick={() => move(index, 1)}>↓</CellButton>
                  <CellButton onClick={() => removeCell(cell.id)}>Remove</CellButton>
                </div>

                {cell.kind === "markdown" ? (
                  isEditing ? (
                    <textarea
                      value={cell.source}
                      onChange={(e) => setSource(cell.id, e.target.value)}
                      onBlur={() =>
                        setEditing((was) => {
                          const next = new Set(was);
                          next.delete(cell.id);
                          return next;
                        })
                      }
                      autoFocus
                      rows={Math.max(3, cell.source.split("\n").length + 1)}
                      placeholder="Notes. Markdown works: # heading, **bold**, `code`, - lists."
                      className="w-full resize-y rounded border border-border bg-surface-0 p-2 font-mono text-[12px] outline-none focus:border-accent"
                    />
                  ) : (
                    <div
                      onDoubleClick={() => setEditing((was) => new Set(was).add(cell.id))}
                      className="cursor-text px-1"
                    >
                      <Markdown source={cell.source} />
                    </div>
                  )
                ) : (
                  <div className="overflow-hidden rounded border border-border">
                    <div className="flex items-start">
                      <textarea
                        value={cell.source}
                        onChange={(e) => setSource(cell.id, e.target.value)}
                        onKeyDown={(e) => {
                          // The same chord as the editor, so the muscle memory
                          // carries over.
                          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                            e.preventDefault();
                            void runCell(cell);
                          }
                        }}
                        rows={Math.max(2, cell.source.split("\n").length)}
                        placeholder="SELECT …"
                        spellCheck={false}
                        className="flex-1 resize-y bg-surface-0 p-2 font-mono text-[length:var(--text-data)] outline-none"
                      />
                      <button
                        onClick={() => void runCell(cell)}
                        disabled={state.running || !cell.source.trim()}
                        title="Run this cell (Ctrl+Enter)"
                        className="flex h-8 w-10 shrink-0 items-center justify-center border-l border-border text-accent hover:bg-surface-2 disabled:opacity-30"
                      >
                        {state.running ? <Spinner /> : "▶"}
                      </button>
                    </div>

                    {state.error && (
                      <p
                        role="alert"
                        className="border-t border-danger/30 bg-danger/10 px-2 py-1 font-mono text-[11px] text-danger"
                        data-selectable
                      >
                        {state.error}
                      </p>
                    )}

                    {state.result?.type === "rows" && (
                      <div className="flex max-h-80 min-h-0 flex-col border-t border-border">
                        <ResultGrid
                          result={state.result}
                          onEdit={async () => {
                            // Editing a cell of a notebook result would need the
                            // provenance and undo machinery a query tab has; a
                            // notebook is for reading, so it says so instead of
                            // half-doing it.
                            throw new Error(
                              "Notebook results are read-only — open the table in a tab to edit rows.",
                            );
                          }}
                          readOnlyDetail={{
                            reason: "Notebook results are read-only.",
                            remedy: "Open the table in its own tab to edit its rows.",
                          }}
                        />
                      </div>
                    )}

                    {state.result?.type === "affected" && (
                      <p className="border-t border-border px-2 py-1 text-[11px] text-text-muted">
                        {state.result.rows_affected} row
                        {state.result.rows_affected === 1 ? "" : "s"} affected
                      </p>
                    )}

                    {state.elapsedMs !== null && !state.error && (
                      <p className="border-t border-border bg-surface-1 px-2 py-0.5 text-[10.5px] text-text-muted">
                        {state.elapsedMs} ms
                      </p>
                    )}
                  </div>
                )}

                <div className="mt-1 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <CellButton onClick={() => addCell("markdown", index)}>+ note</CellButton>
                  <CellButton onClick={() => addCell("sql", index)}>+ query</CellButton>
                </div>
              </div>
            );
          })}

          <div className="flex justify-center gap-2 pt-2">
            <Button variant="ghost" className="h-6" onClick={() => addCell("markdown")}>
              Add a note
            </Button>
            <Button variant="ghost" className="h-6" onClick={() => addCell("sql")}>
              Add a query
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

function CellButton({
  onClick,
  children,
}: {
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className="rounded px-1 py-0.5 text-[10.5px] text-text-muted hover:bg-surface-2 hover:text-text"
    >
      {children}
    </button>
  );
}
