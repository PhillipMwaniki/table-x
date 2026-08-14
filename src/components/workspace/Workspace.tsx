/**
 * The working area for one connected connection: schema tree on the left, tabs
 * across the top, and the active tab's editor and results below.
 *
 * A table tab shows rows with no editor — you opened an object, not a question —
 * while a query tab is an editor over a result. Both carry the database they
 * belong to, which is what makes them safe to switch between on a server that
 * has more than one.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { SchemaTree } from "./SchemaTree";
import { SqlEditor } from "./SqlEditor";
import { ResultGrid } from "./ResultGrid";
import { HistoryPanel } from "./HistoryPanel";
import { TabBar } from "./TabBar";
import { SplitHandle } from "./SplitHandle";
import { Button, Spinner, cx } from "../ui/primitives";
import { ContextMenu } from "../ui/ContextMenu";
import type { MenuItem } from "../ui/ContextMenu";
import { ipc } from "@/lib/ipc";
import { drop, selectFrom, truncate } from "@/lib/statements";
import { useHistory } from "@/store/history";
import { useSettings } from "@/store/settings";
import { useWorkspace } from "@/store/workspace";
import type { ConnectionConfig, DriverInfo, NodeKind, SchemaNode, StatementResult } from "@/lib/types";

/**
 * Object kinds whose source is a statement worth editing.
 *
 * A table's "definition" is its columns, which the structure view shows far
 * better than a CREATE statement would; these are the ones where the script
 * *is* the object.
 */
const SCRIPTED: NodeKind[] = ["function", "procedure", "trigger"];

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
    openScript: openScriptTab,
    setTabError,
    useDatabase,
    applyEdit,
    undo,
    redo,
  } = useWorkspace();

  const historyOpen = useHistory((s) => s.open);
  const setHistoryOpen = useHistory((s) => s.setOpen);

  // The stored split, and the live one while the divider is being dragged.
  // Dragging keeps its position here rather than in the store so a drag does
  // not write a preferences file sixty times a second.
  const storedRatio = useSettings((s) => s.editorRatio);
  const setEditorRatio = useSettings((s) => s.setEditorRatio);
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  const [menu, setMenu] = useState<{
    node: SchemaNode;
    x: number;
    y: number;
    /** Supplied by the tree, which owns the cache being refreshed. */
    refresh: (() => void) | null;
  } | null>(null);
  const ratio = dragRatio ?? storedRatio;
  const splitRef = useRef<HTMLDivElement>(null);

  const tab = activeTab(connection.id);
  const completion = useWorkspace((s) => s.completion[connection.id] ?? null);
  const database = useWorkspace((s) => s.database[connection.id] ?? null);

  // The session's database and the first tab are established once per
  // connection; autocomplete is fetched once rather than per keystroke.
  //
  // Completion is chained rather than fired alongside, because everything on a
  // connection is serialized behind one session lock: asking for it at the same
  // moment as the tree's first query puts the catalogue scan in front of what
  // the user is looking at.
  useEffect(() => {
    void loadSession(connection.id).then(() => loadCompletionFor(connection.id));
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

  /**
   * Fetch an object's source and open it in its own tab.
   *
   * The failure lands on the tab the user is looking at rather than in a dialog:
   * "this procedure is encrypted" is information about the object, and it
   * belongs where the rest of the query errors go.
   */
  const openScript = async (node: SchemaNode) => {
    try {
      const sql = await ipc.objectDefinition(connection.id, node.id);
      openScriptTab(connection.id, { title: node.name, sql });
    } catch (e) {
      const current = activeTab(connection.id);
      if (current) {
        setTabError(connection.id, current.id, (e as Error).message);
      }
    }
  };

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
          onOpenScript={(node) => void openScript(node)}
          onContextMenu={(node, at, refresh) =>
            setMenu({ node, x: at.x, y: at.y, refresh })
          }
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
            <div ref={splitRef} className="flex min-h-0 flex-1 flex-col">
              {tab.kind === "query" && (
                <>
                  <div
                    style={{ height: `${ratio * 100}%` }}
                    className="min-h-0 shrink-0 overflow-hidden"
                  >
                    <SqlEditor
                      value={tab.sql}
                      onChange={(sql) => setSql(connection.id, tab.id, sql)}
                      onRun={(text) => void run(connection.id, tab.id, text)}
                      driver={connection.driver}
                      completion={completion}
                      errorPosition={tab.error?.position}
                    />
                  </div>

                  <SplitHandle
                    containerRef={splitRef}
                    ratio={ratio}
                    onPreview={setDragRatio}
                    onCommit={(next) => {
                      setEditorRatio(next);
                      setDragRatio(null);
                    }}
                  />
                </>
              )}

              <div className="flex min-h-0 flex-1 flex-col border-t border-border">
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

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuFor(
            menu.node,
            {
              driver: connection.driver,
              tableScripts: driver?.capabilities.table_scripts ?? false,
            },
            {
              onOpen: () =>
                openTable(connection.id, {
                  title: menu.node.name,
                  qualified: menu.node.qualified ?? menu.node.name,
                }),
              onScript: () => void openScript(menu.node),
              onNewTab: (title, sql) => openScriptTab(connection.id, { title, sql }),
              onCopy: (text) => void navigator.clipboard?.writeText(text),
              onRefresh: menu.refresh,
            },
          )}
          onClose={() => setMenu(null)}
        />
      )}

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

/**
 * What right-clicking this node offers.
 *
 * Only actions that would actually work: an item that reports "not supported"
 * when clicked is an item that should not have been drawn. That is why the
 * script entries are gated on the driver rather than shown everywhere and
 * allowed to fail — PostgreSQL cannot render a table as a CREATE statement, and
 * SQL Server's OBJECT_DEFINITION returns nothing for one.
 *
 * Destructive statements open in a tab rather than running. A confirmation
 * dialog asks whether you meant it; showing you the statement asks the better
 * question, which is whether it says what you meant.
 */
export function menuFor(
  node: SchemaNode,
  options: { driver: string; tableScripts: boolean },
  actions: {
    onOpen: () => void;
    onScript: () => void;
    onNewTab: (title: string, sql: string) => void;
    onCopy: (text: string) => void;
    onRefresh: (() => void) | null;
  },
): MenuItem[] {
  const items: MenuItem[] = [];
  const qualified = node.qualified ?? node.name;
  const isRelation =
    node.kind === "table" || node.kind === "view" || node.kind === "materialized_view";

  if (isRelation) {
    items.push({ label: "Open rows", onSelect: actions.onOpen });
    items.push({
      label: "New tab: SELECT",
      onSelect: () => actions.onNewTab(node.name, selectFrom(qualified, options.driver)),
    });
    if (options.tableScripts) {
      items.push({ label: "Show CREATE statement", onSelect: actions.onScript });
    }
  }

  if (SCRIPTED.includes(node.kind)) {
    items.push({ label: "Edit script", onSelect: actions.onScript });
  }

  if (node.qualified) {
    items.push({
      label: "Copy qualified name",
      separated: items.length > 0,
      onSelect: () => actions.onCopy(qualified),
    });
  }
  if (node.kind !== "folder") {
    items.push({ label: "Copy name", onSelect: () => actions.onCopy(node.name) });
  }

  if (actions.onRefresh) {
    items.push({ label: "Refresh", separated: true, onSelect: actions.onRefresh });
  }

  // Emptying and dropping are last, separated, and phrased with an ellipsis
  // because neither happens on click — both open the statement for review.
  if (node.kind === "table") {
    items.push({
      label: "Truncate table…",
      separated: true,
      onSelect: () =>
        actions.onNewTab(`Truncate ${node.name}`, truncate(qualified, options.driver)),
    });
  }
  if (isRelation) {
    items.push({
      label: node.kind === "view" ? "Drop view…" : "Drop table…",
      separated: node.kind !== "table",
      onSelect: () =>
        actions.onNewTab(
          `Drop ${node.name}`,
          drop(qualified, options.driver, node.kind === "view" ? "view" : "table"),
        ),
    });
  }

  return items;
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
