/**
 * The working area for one connected connection: schema tree on the left, SQL
 * editor above, results below.
 */

import { useEffect, useMemo } from "react";
import { SchemaTree, selectFor } from "./SchemaTree";
import { SqlEditor } from "./SqlEditor";
import { ResultGrid } from "./ResultGrid";
import { HistoryPanel } from "./HistoryPanel";
import { Button, Spinner, cx } from "../ui/primitives";
import { useHistory } from "@/store/history";
import { useWorkspace } from "@/store/workspace";
import type { ConnectionConfig, DriverInfo, StatementResult } from "@/lib/types";

export function Workspace({
  connection,
  driver,
}: {
  connection: ConnectionConfig;
  driver: DriverInfo | undefined;
}) {
  const {
    pane,
    setSql,
    setActiveStatement,
    run,
    loadCompletion,
    applyEdit,
    undo,
    redo,
  } = useWorkspace();

  const historyOpen = useHistory((s) => s.open);
  const setHistoryOpen = useHistory((s) => s.setOpen);

  const state = pane(connection.id);
  const quote = driver?.capabilities.identifier_quote ?? '"';

  // Autocomplete data is fetched once per connection, not per keystroke.
  useEffect(() => {
    void loadCompletion(connection.id);
  }, [connection.id, loadCompletion]);

  // Undo/redo are global shortcuts while this pane is mounted. Bound on window
  // rather than the grid so they work regardless of which element has focus,
  // except inside the editor, which has its own text history.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const inEditor = (e.target as HTMLElement)?.closest?.(".cm-editor");
      if (inEditor) return;
      const mod = e.ctrlKey || e.metaKey;
      if (!mod || e.key.toLowerCase() !== "z") return;
      e.preventDefault();
      void (e.shiftKey ? redo(connection.id) : undo(connection.id));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [connection.id, undo, redo]);

  // History gets its own handler rather than joining the one above, because it
  // must work while the caret is in the editor — that is where you are when you
  // want a previous statement back.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "h") return;
      e.preventDefault();
      setHistoryOpen(!useHistory.getState().open);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setHistoryOpen]);

  const active = state.outcome?.statements[state.activeStatement];

  const readOnlyReason = useMemo(() => {
    if (connection.read_only) return "This connection is marked read-only.";
    if (!driver?.capabilities.column_provenance) {
      return `${driver?.name ?? "This driver"} does not report which table each column came from, so results cannot be edited in place.`;
    }
    return "This result has no single source table with a usable key — joins and aggregates are read-only.";
  }, [connection.read_only, driver]);

  return (
    <div className="flex min-h-0 flex-1">
      <aside className="flex w-56 shrink-0 flex-col overflow-y-auto border-r border-border bg-surface-1">
        <SchemaTree
          connectionId={connection.id}
          onOpenTable={(node) => {
            const sql = selectFor(node, quote);
            setSql(connection.id, sql);
            void run(connection.id, sql);
          }}
        />
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
          <Button
            variant="primary"
            onClick={() => void run(connection.id)}
            busy={state.running}
            disabled={!state.sql.trim()}
            className="h-6"
          >
            Run
          </Button>
          <span className="text-[10.5px] text-text-muted">Ctrl+Enter</span>

          {connection.read_only && (
            <span className="rounded bg-warn/15 px-1.5 py-0.5 text-[10px] font-medium text-warn">
              READ-ONLY
            </span>
          )}

          <div className="flex-1" />

          <Button
            variant="ghost"
            className={cx("h-6", historyOpen && "bg-surface-3 text-text")}
            onClick={() => setHistoryOpen(!historyOpen)}
            aria-pressed={historyOpen}
            title="Query history (Ctrl+H)"
          >
            History
          </Button>
          <Button
            variant="ghost"
            className="h-6"
            disabled={state.undo.length === 0}
            onClick={() => void undo(connection.id)}
            title="Undo last cell edit (Ctrl+Z)"
          >
            Undo{state.undo.length > 0 && ` (${state.undo.length})`}
          </Button>
          <Button
            variant="ghost"
            className="h-6"
            disabled={state.redo.length === 0}
            onClick={() => void redo(connection.id)}
            title="Redo (Ctrl+Shift+Z)"
          >
            Redo
          </Button>
        </div>

        {/* The editor gets a fixed share and the results take the rest, which is
            the proportion people actually want once a result is on screen. */}
        <div className="h-[38%] min-h-24 shrink-0 border-b border-border">
          <SqlEditor
            value={state.sql}
            onChange={(sql) => setSql(connection.id, sql)}
            onRun={(text) => void run(connection.id, text)}
            driver={connection.driver}
            completion={state.completion}
            errorPosition={state.error?.position}
          />
        </div>

        <div className="flex min-h-0 flex-1 flex-col">
          {state.error && (
            <div role="alert" className="shrink-0 border-b border-danger/30 bg-danger/10 px-2 py-1.5">
              <p className="font-mono text-[11px] text-danger" data-selectable>
                {state.error.message}
              </p>
              {state.error.code && (
                <p className="mt-0.5 text-[10px] text-danger/70">
                  SQLSTATE {state.error.code}
                  {state.error.position !== undefined && ` · position ${state.error.position}`}
                </p>
              )}
            </div>
          )}

          {state.outcome && state.outcome.statements.length > 1 && (
            <StatementTabs
              statements={state.outcome.statements}
              active={state.activeStatement}
              onSelect={(i) => setActiveStatement(connection.id, i)}
            />
          )}

          {state.running && !state.outcome ? (
            <div className="flex flex-1 items-center justify-center">
              <Spinner className="text-text-muted" />
            </div>
          ) : active?.type === "rows" ? (
            <ResultGrid
              result={active}
              readOnlyReason={readOnlyReason}
              onEdit={(row, col, next) => applyEdit(connection.id, row, col, next)}
            />
          ) : active?.type === "affected" ? (
            <div className="flex flex-1 items-center justify-center text-[12px] text-text-muted">
              {active.rows_affected} row{active.rows_affected === 1 ? "" : "s"} affected
              {active.last_insert_id != null && ` · last insert id ${active.last_insert_id}`}
            </div>
          ) : (
            <div className="flex flex-1 items-center justify-center px-6 text-center text-[12px] text-text-muted">
              {state.error
                ? "Fix the statement and run again."
                : "Write a query and press Ctrl+Enter, or click a table in the sidebar."}
            </div>
          )}

          {state.outcome && (
            <div className="flex h-5 shrink-0 items-center gap-3 border-t border-border bg-surface-1 px-2 text-[10.5px] text-text-muted">
              <span>{state.outcome.elapsed_ms} ms</span>
              {state.outcome.statements.length > 1 && (
                <span>{state.outcome.statements.length} statements</span>
              )}
              {state.outcome.notices.map((n, i) => (
                <span key={i} className="truncate text-warn">
                  {n}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>

      <HistoryPanel
        connectionId={connection.id}
        onPick={(sql) => setSql(connection.id, sql)}
        onRun={(sql) => {
          setSql(connection.id, sql);
          void run(connection.id, sql);
        }}
      />
    </div>
  );
}

function StatementTabs({
  statements,
  active,
  onSelect,
}: {
  statements: StatementResult[];
  active: number;
  onSelect: (index: number) => void;
}) {
  return (
    <div className="flex h-6 shrink-0 items-center gap-px overflow-x-auto border-b border-border bg-surface-1 px-1">
      {statements.map((s, i) => (
        <button
          key={i}
          onClick={() => onSelect(i)}
          className={cx(
            "shrink-0 rounded px-2 py-0.5 text-[10.5px] whitespace-nowrap",
            i === active ? "bg-surface-3 text-text" : "text-text-muted hover:bg-surface-2",
          )}
        >
          {i + 1}. {s.type === "rows" ? `${s.rows.length} rows` : `${s.rows_affected} affected`}
        </button>
      ))}
    </div>
  );
}
