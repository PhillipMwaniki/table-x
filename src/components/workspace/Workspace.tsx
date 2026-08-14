/**
 * The working area for one connected connection: schema tree on the left, tabs
 * across the top, and the active tab's editor and results below.
 *
 * A table tab shows rows with no editor — you opened an object, not a question —
 * while a query tab is an editor over a result. Both carry the database they
 * belong to, which is what makes them safe to switch between on a server that
 * has more than one.
 */

import { useEffect, useMemo } from "react";
import { SchemaTree } from "./SchemaTree";
import { SqlEditor } from "./SqlEditor";
import { ResultGrid } from "./ResultGrid";
import { HistoryPanel } from "./HistoryPanel";
import { TabBar } from "./TabBar";
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
    activeTab,
    openQuery,
    openTable,
    setSql,
    setActiveStatement,
    run,
    loadSession,
    loadCompletionFor,
    useDatabase,
    applyEdit,
    undo,
    redo,
  } = useWorkspace();

  const historyOpen = useHistory((s) => s.open);
  const setHistoryOpen = useHistory((s) => s.setOpen);

  const tab = activeTab(connection.id);
  const completion = useWorkspace((s) => s.completion[connection.id] ?? null);
  const database = useWorkspace((s) => s.database[connection.id] ?? null);

  // The session's database and the first tab are established once per
  // connection; autocomplete is fetched once rather than per keystroke.
  useEffect(() => {
    void loadSession(connection.id);
    void loadCompletionFor(connection.id);
  }, [connection.id, loadSession, loadCompletionFor]);

  // Undo/redo are global shortcuts while this pane is mounted. Bound on window
  // rather than the grid so they work regardless of which element has focus,
  // except inside the editor, which has its own text history.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const inEditor = (e.target as HTMLElement)?.closest?.(".cm-editor");
      if (inEditor) return;
      const mod = e.ctrlKey || e.metaKey;
      if (!mod || e.key.toLowerCase() !== "z" || !tab) return;
      e.preventDefault();
      void (e.shiftKey ? redo(connection.id, tab.id) : undo(connection.id, tab.id));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [connection.id, tab, undo, redo]);

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

  // A new query tab is Ctrl+T, as it is in every tabbed application.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "t") return;
      e.preventDefault();
      openQuery(connection.id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [connection.id, openQuery]);

  const active = tab?.outcome?.statements[tab.activeStatement];

  const readOnlyReason = useMemo(() => {
    if (connection.read_only) return "This connection is marked read-only.";
    if (!driver?.capabilities.column_provenance) {
      return `${driver?.name ?? "This driver"} does not report which table each column came from, so results cannot be edited in place.`;
    }
    return "This result has no single source table with a usable key — joins and aggregates are read-only.";
  }, [connection.read_only, driver]);

  return (
    <div className="flex min-h-0 flex-1">
      <aside className="flex w-60 shrink-0 flex-col overflow-y-auto border-r border-border bg-surface-1">
        <SchemaTree
          connectionId={connection.id}
          activeDatabase={database}
          onOpenTable={(node) =>
            openTable(connection.id, {
              title: node.name,
              // The driver already quoted and qualified this for its own
              // engine, so the UI never assembles a name itself.
              qualified: node.qualified ?? node.name,
              schema: node.schema,
            })
          }
          onSelectDatabase={(name) => void useDatabase(connection.id, name)}
        />
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <TabBar connectionId={connection.id} />

        {tab ? (
          <>
            <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
              <Button
                variant="primary"
                onClick={() => void run(connection.id, tab.id)}
                busy={tab.running}
                disabled={!tab.sql.trim()}
                className="h-6"
              >
                {tab.kind === "table" ? "Refresh" : "Run"}
              </Button>
              {tab.kind === "query" && (
                <span className="text-[10.5px] text-text-muted">Ctrl+Enter</span>
              )}

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
                disabled={tab.undo.length === 0}
                onClick={() => void undo(connection.id, tab.id)}
                title="Undo last cell edit (Ctrl+Z)"
              >
                Undo{tab.undo.length > 0 && ` (${tab.undo.length})`}
              </Button>
              <Button
                variant="ghost"
                className="h-6"
                disabled={tab.redo.length === 0}
                onClick={() => void redo(connection.id, tab.id)}
                title="Redo (Ctrl+Shift+Z)"
              >
                Redo
              </Button>
            </div>

            {/* A table tab gives its whole height to the rows: there is no
                statement to edit, and the SQL that produced them is one line
                the tab's own context already describes. */}
            {tab.kind === "query" && (
              <div className="h-[38%] min-h-24 shrink-0 border-b border-border">
                <SqlEditor
                  value={tab.sql}
                  onChange={(sql) => setSql(connection.id, tab.id, sql)}
                  onRun={(text) => void run(connection.id, tab.id, text)}
                  driver={connection.driver}
                  completion={completion}
                  errorPosition={tab.error?.position}
                />
              </div>
            )}

            <div className="flex min-h-0 flex-1 flex-col">
              {tab.error && (
                <div
                  role="alert"
                  className="shrink-0 border-b border-danger/30 bg-danger/10 px-2 py-1.5"
                >
                  <p className="font-mono text-[11px] text-danger" data-selectable>
                    {tab.error.message}
                  </p>
                  {tab.error.code && (
                    <p className="mt-0.5 text-[10px] text-danger/70">
                      SQLSTATE {tab.error.code}
                      {tab.error.position !== undefined && ` · position ${tab.error.position}`}
                    </p>
                  )}
                </div>
              )}

              {tab.outcome && tab.outcome.statements.length > 1 && (
                <StatementTabs
                  statements={tab.outcome.statements}
                  active={tab.activeStatement}
                  onSelect={(i) => setActiveStatement(connection.id, tab.id, i)}
                />
              )}

              {tab.running && !tab.outcome ? (
                <div className="flex flex-1 items-center justify-center">
                  <Spinner className="text-text-muted" />
                </div>
              ) : active?.type === "rows" ? (
                <ResultGrid
                  result={active}
                  readOnlyReason={readOnlyReason}
                  onEdit={(row, col, next) => applyEdit(connection.id, tab.id, row, col, next)}
                />
              ) : active?.type === "affected" ? (
                <div className="flex flex-1 items-center justify-center text-[12px] text-text-muted">
                  {active.rows_affected} row{active.rows_affected === 1 ? "" : "s"} affected
                  {active.last_insert_id != null && ` · last insert id ${active.last_insert_id}`}
                </div>
              ) : (
                <div className="flex flex-1 items-center justify-center px-6 text-center text-[12px] text-text-muted">
                  {tab.error
                    ? "Fix the statement and run again."
                    : "Write a query and press Ctrl+Enter, or click a table in the sidebar."}
                </div>
              )}

              {tab.outcome && (
                <div className="flex h-5 shrink-0 items-center gap-3 border-t border-border bg-surface-1 px-2 text-[10.5px] text-text-muted">
                  <span>{tab.outcome.elapsed_ms} ms</span>
                  {tab.outcome.statements.length > 1 && (
                    <span>{tab.outcome.statements.length} statements</span>
                  )}
                  {tab.outcome.notices.map((n, i) => (
                    <span key={i} className="truncate text-warn">
                      {n}
                    </span>
                  ))}
                </div>
              )}
            </div>
          </>
        ) : (
          <div className="flex flex-1 items-center justify-center">
            <Button variant="primary" onClick={() => openQuery(connection.id)}>
              New query
            </Button>
          </div>
        )}
      </div>

      <HistoryPanel
        connectionId={connection.id}
        onPick={(sql) => tab && setSql(connection.id, tab.id, sql)}
        onRun={(sql) => {
          if (!tab) return;
          setSql(connection.id, tab.id, sql);
          void run(connection.id, tab.id, sql);
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
