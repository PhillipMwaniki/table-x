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
import { ExportProgress } from "./ExportProgress";
import { SplitHandle } from "./SplitHandle";
import { CsvImportDialog } from "./CsvImportDialog";
import { ActivityPanel } from "./ActivityPanel";
import { PlanView } from "./PlanView";
import { Button, Spinner, cx } from "../ui/primitives";
import { ContextMenu } from "../ui/ContextMenu";
import type { MenuItem } from "../ui/ContextMenu";
import { ipc, IpcError } from "@/lib/ipc";
import { drop, selectFrom, truncate } from "@/lib/statements";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useHistory } from "@/store/history";
import { useSnippets } from "@/store/snippets";
import { useCommands } from "@/store/commands";
import { useSettings } from "@/store/settings";
import { useExports } from "@/store/exports";
import { useWorkspace } from "@/store/workspace";
import type {
  ColumnDef,
  ConnectionConfig,
  DriverInfo,
  ExportFormat,
  NodeKind,
  SchemaNode,
  StatementResult,
} from "@/lib/types";

/** Formats offered in the object menu, in the order they are listed. */
const EXPORT_FORMATS: { format: ExportFormat; label: string; extension: string }[] = [
  { format: "csv", label: "CSV", extension: "csv" },
  { format: "json", label: "JSON", extension: "json" },
  { format: "sql", label: "SQL inserts", extension: "sql" },
];

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
    closeTab,
    setSql,
    setActiveStatement,
    run,
    loadSession,
    loadCompletionFor,
    openScript: openScriptTab,
    openActivity,
    setTabError,
    setTabNotice,
    useDatabase,
    applyEdit,
    explain,
    clearPlan,
    undo,
    redo,
  } = useWorkspace();

  const historyOpen = useHistory((s) => s.open);
  const setHistoryOpen = useHistory((s) => s.setOpen);
  const setPanelTab = useHistory((s) => s.setTab);
  const saveSnippet = useSnippets((s) => s.save);
  const registerCommands = useCommands((s) => s.register);

  // The stored split, and the live one while the divider is being dragged.
  // Dragging keeps its position here rather than in the store so a drag does
  // not write a preferences file sixty times a second.
  const storedRatio = useSettings((s) => s.editorRatio);
  const setEditorRatio = useSettings((s) => s.setEditorRatio);
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  const watchExports = useExports((s) => s.watch);
  const beginExport = useExports((s) => s.begin);
  const endExport = useExports((s) => s.end);
  /** The file and table a mapping dialog is open for, if any. */
  const [csvImport, setCsvImport] = useState<{
    path: string;
    node: SchemaNode & { schema?: string | undefined };
    columns: ColumnDef[];
  } | null>(null);
  const [menu, setMenu] = useState<{
    node: SchemaNode & { schema?: string | undefined };
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

  // One subscription for the process, established the first time a workspace
  // mounts; the store ignores repeat calls.
  useEffect(() => {
    void watchExports();
  }, [watchExports]);

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

  /**
   * Ask where to write, then export.
   *
   * The dialog is the frontend's, the writing is the backend's: the webview has
   * no filesystem access of its own, so it hands over a path and nothing else.
   */
  const exportTable = async (
    node: SchemaNode & { schema?: string | undefined },
    format: ExportFormat,
    extension: string,
  ) => {
    const path = await save({
      defaultPath: `${node.name}.${extension}`,
      filters: [{ name: format.toUpperCase(), extensions: [extension] }],
    });
    // Cancelling the dialog is a decision, not a failure.
    if (!path) return;

    const id = crypto.randomUUID();
    const current = activeTab(connection.id);
    // Registered before the call so the bar appears immediately: the first
    // query is often the slow part, and it emits nothing until it returns.
    beginExport(id, node.name);
    try {
      const rows = await ipc.exportTable({
        id,
        connection_id: connection.id,
        qualified: node.qualified ?? node.name,
        schema: node.schema,
        table: node.name,
        format,
        path,
      });
      if (current) {
        setTabNotice(
          connection.id,
          current.id,
          `Exported ${rows.toLocaleString()} rows to ${path}`,
        );
      }
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, `Export of ${node.name}`);
    } finally {
      endExport(id);
    }
  };


  /**
   * Dump a whole database to one SQL file.
   *
   * Schema then data, table by table, which is the order a restore needs.
   */
  const exportDatabase = async (database: string) => {
    const path = await save({
      defaultPath: `${database}.sql`,
      filters: [{ name: "SQL", extensions: ["sql"] }],
    });
    if (!path) return;

    const id = crypto.randomUUID();
    const current = activeTab(connection.id);
    beginExport(id, `Exporting ${database}`, "tables");
    try {
      const rows = await ipc.exportDatabase({
        id,
        connection_id: connection.id,
        database,
        path,
      });
      if (current) {
        setTabNotice(
          connection.id,
          current.id,
          `Exported ${database} — ${rows.toLocaleString()} rows to ${path}`,
        );
      }
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, `Export of ${database}`);
    } finally {
      endExport(id);
    }
  };

  /**
   * Load a delimited file into a table.
   *
   * The file is chosen first and the columns are read before anything opens,
   * so the dialog can show the mapping already made rather than an empty form
   * asking the user to describe their own file back to us.
   */
  const importCsv = async (node: SchemaNode & { schema?: string | undefined }) => {
    const path = await open({
      multiple: false,
      filters: [{ name: "Delimited text", extensions: ["csv", "tsv", "txt"] }],
    });
    if (typeof path !== "string") return;

    const current = activeTab(connection.id);
    try {
      const detail = await ipc.tableDetail(connection.id, node.name, node.schema);
      setCsvImport({ path, node, columns: detail.columns });
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, `Import into ${node.name}`);
    }
  };

  /** Run the import the dialog just described. */
  const runCsvImport = async (
    target: { path: string; node: SchemaNode & { schema?: string | undefined } },
    options: {
      delimiter: string;
      hasHeader: boolean;
      mapping: (string | null)[];
      nullAsEmpty: boolean;
    },
  ) => {
    setCsvImport(null);
    const id = crypto.randomUUID();
    const current = activeTab(connection.id);
    beginExport(id, `Importing ${target.path.split(/[\\/]/).pop()}`, "KB");
    try {
      const rows = await ipc.importCsv({
        id,
        connection_id: connection.id,
        path: target.path,
        qualified: target.node.qualified ?? target.node.name,
        schema: target.node.schema,
        table: target.node.name,
        delimiter: options.delimiter,
        has_header: options.hasHeader,
        mapping: options.mapping,
        null_as_empty: options.nullAsEmpty,
      });
      if (current) {
        setTabNotice(
          connection.id,
          current.id,
          `Imported ${rows.toLocaleString()} rows into ${target.node.name}`,
        );
      }
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, `Import into ${target.node.name}`);
    } finally {
      endExport(id);
    }
  };

  /**
   * Run a SQL file against this connection.
   *
   * Nothing is dropped or emptied first: what the file does is what happens,
   * and a restore that silently cleared the target would be a data-loss bug
   * wearing a feature's clothes.
   */
  const importSql = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "SQL", extensions: ["sql"] }],
    });
    if (typeof path !== "string") return;

    const id = crypto.randomUUID();
    const current = activeTab(connection.id);
    beginExport(id, `Importing ${path.split(/[\\/]/).pop()}`, "KB");
    try {
      const applied = await ipc.importSql({ id, connection_id: connection.id, path });
      if (current) {
        setTabNotice(
          connection.id,
          current.id,
          `Applied ${applied.toLocaleString()} statements from ${path}`,
        );
      }
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, "Import");
    } finally {
      endExport(id);
    }
  };

  /** Cancelling is a decision; anything else is a failure worth reading. */
  const reportJobFailure = (err: IpcError, tabId: string | undefined, what: string) => {
    if (!tabId) return;
    if (err.category === "cancelled") {
      setTabNotice(connection.id, tabId, `${what} cancelled.`);
    } else {
      setTabError(connection.id, tabId, err.message);
    }
  };


  /**
   * Keep the current statement under a name.
   *
   * Named through a prompt rather than a dialog: the name is the only thing
   * being asked for, and a modal for one text field is a modal too many.
   */
  const saveCurrentQuery = () => {
    if (!tab?.sql.trim()) return;
    const suggested = tab.kind === "table" ? tab.title : "";
    const name = window.prompt("Save this query as", suggested);
    if (name === null) return;
    if (!name.trim()) {
      setTabError(connection.id, tab.id, "A saved query needs a name.");
      return;
    }
    void saveSnippet(name, tab.sql).then(() => {
      setPanelTab("snippets");
      setHistoryOpen(true);
      setTabNotice(connection.id, tab.id, `Saved as “${name.trim()}”.`);
    });
  };


  /**
   * Reformat the tab's SQL in place.
   *
   * The result replaces the editor's contents, so it goes through the same
   * setSql the editor writes to — the undo history in CodeMirror then treats it
   * as one edit, which is what makes it safe to try.
   */
  const formatCurrentSql = async () => {
    if (!tab?.sql.trim()) return;
    try {
      const formatted = await ipc.formatSql(tab.sql);
      if (formatted.trim()) setSql(connection.id, tab.id, formatted);
    } catch (e) {
      setTabError(connection.id, tab.id, (e as Error).message);
    }
  };

  const active = tab?.outcome?.statements[tab.activeStatement];


  // Ctrl+Shift+F formats, matching every editor people arrive from. Bound on
  // the window so it works with the caret in the editor, where it is used.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.key.toLowerCase() !== "f") return;
      e.preventDefault();
      void formatCurrentSql();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });


  // Registered while this workspace is mounted, so the palette offers running
  // a query only when there is something to run it against.
  useEffect(() => {
    if (!tab) return;
    return registerCommands("workspace", [
      {
        id: "ws.run",
        title: tab.kind === "table" ? "Refresh rows" : "Run query",
        group: "Query",
        shortcut: "Ctrl+Enter",
        run: () => void run(connection.id, tab.id),
      },
      {
        id: "ws.format",
        title: "Format SQL",
        group: "Query",
        shortcut: "Ctrl+Shift+F",
        run: () => void formatCurrentSql(),
      },
      {
        id: "ws.save-query",
        title: "Save query",
        group: "Query",
        run: saveCurrentQuery,
      },
      {
        id: "ws.new-tab",
        title: "New query tab",
        group: "Tabs",
        shortcut: "Ctrl+T",
        run: () => openQuery(connection.id),
      },
      {
        id: "ws.close-tab",
        title: "Close tab",
        group: "Tabs",
        run: () => closeTab(connection.id, tab.id),
      },
      {
        id: "ws.history",
        title: "Query history",
        group: "View",
        shortcut: "Ctrl+H",
        run: () => {
          setPanelTab("history");
          setHistoryOpen(true);
        },
      },
      {
        id: "ws.snippets",
        title: "Saved queries",
        group: "View",
        run: () => {
          setPanelTab("snippets");
          setHistoryOpen(true);
        },
      },
      {
        id: "ws.explain",
        title: "Explain statement",
        group: "Query",
        shortcut: "Ctrl+Shift+E",
        run: () => {
          const current = activeTab(connection.id);
          if (current) void explain(connection.id, current.id, false);
        },
      },
      {
        id: "ws.activity",
        title: "Show server activity",
        group: "Data",
        run: () => openActivity(connection.id),
      },
      {
        id: "ws.import",
        title: "Import SQL file",
        group: "Data",
        run: () => void importSql(),
      },
      {
        id: "ws.undo",
        title: "Undo cell edit",
        group: "Data",
        shortcut: "Ctrl+Z",
        run: () => void undo(connection.id, tab.id),
      },
    ]);
  }, [
    registerCommands,
    connection.id,
    tab,
    run,
    openQuery,
    closeTab,
    setHistoryOpen,
    setPanelTab,
    undo,
  ]);

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
        <ExportProgress />

        {tab ? (
          <>
            <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
              {/* Every control here acts on a statement, and an activity tab has
                  none — it carries its own refresh instead. */}
              {tab.kind === "activity" ? (
                <span className="text-[11px] text-text-muted">
                  Live view of the server. Nothing here is cached.
                </span>
              ) : (
                <>
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

              {tab.kind === "query" && (
                <Button
                  variant="ghost"
                  className="h-6"
                  disabled={!tab.sql.trim()}
                  onClick={() => void formatCurrentSql()}
                  title="Format SQL (Ctrl+Shift+F)"
                >
                  Format
                </Button>
              )}
              {tab.kind === "query" && driver?.capabilities.explain && (
                <Button
                  variant="ghost"
                  className="h-6"
                  disabled={!tab.sql.trim()}
                  onClick={() => void explain(connection.id, tab.id, false)}
                  title="Show how the engine intends to run this (Ctrl+Shift+E)"
                >
                  Explain
                </Button>
              )}
              {tab.kind === "query" && driver?.capabilities.explain_analyze && (
                <Button
                  variant="ghost"
                  className="h-6"
                  disabled={!tab.sql.trim()}
                  onClick={() => void explain(connection.id, tab.id, true)}
                  title="Run it inside a transaction that is rolled back, and measure"
                >
                  Analyze
                </Button>
              )}
              <Button
                variant="ghost"
                className="h-6"
                disabled={!tab.sql.trim()}
                onClick={saveCurrentQuery}
                title="Keep this statement under a name"
              >
                Save query
              </Button>
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
                </>
              )}
            </div>

            {/* A table tab gives its whole height to the rows: there is no
                statement to edit, and the SQL that produced them is one line
                the tab's own context already describes. */}
            {tab.kind === "activity" ? (
              <ActivityPanel
                connectionId={connection.id}
                readOnly={connection.read_only}
                onOpenQuery={(sql) => openScriptTab(connection.id, { title: "Statement", sql })}
              />
            ) : (
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

              {tab.notice && (
                <p
                  role="status"
                  className="shrink-0 border-b border-ok/30 bg-ok/10 px-2 py-1.5 text-[11px] text-ok"
                  data-selectable
                >
                  {tab.notice}
                </p>
              )}

              {tab.outcome && tab.outcome.statements.length > 1 && (
                <StatementTabs
                  statements={tab.outcome.statements}
                  active={tab.activeStatement}
                  onSelect={(i) => setActiveStatement(connection.id, tab.id, i)}
                />
              )}

              {tab.plan ? (
                <PlanView plan={tab.plan} onClose={() => clearPlan(connection.id, tab.id)} />
              ) : tab.running && !tab.outcome ? (
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
            )}
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
              onExport: (format, extension) => void exportTable(menu.node, format, extension),
              onExportDatabase: () => void exportDatabase(menu.node.name),
              onImport: () => void importSql(),
              onActivity: () => openActivity(connection.id),
              onImportCsv: () => void importCsv(menu.node),
              onRefresh: menu.refresh,
            },
          )}
          onClose={() => setMenu(null)}
        />
      )}

      {csvImport && (
        <CsvImportDialog
          open
          path={csvImport.path}
          table={csvImport.node.name}
          columns={csvImport.columns}
          onClose={() => setCsvImport(null)}
          onImport={(options) => void runCsvImport(csvImport, options)}
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
    onExport: (format: ExportFormat, extension: string) => void;
    onExportDatabase: () => void;
    onImport: () => void;
    onImportCsv: () => void;
    onActivity: () => void;
    onRefresh: (() => void) | null;
  },
): MenuItem[] {
  const items: MenuItem[] = [];
  const qualified = node.qualified ?? node.name;

  // A database is dumped whole and restored whole; the per-table formats do
  // not apply to it, and a per-table menu does not apply to a database.
  if (node.kind === "database") {
    items.push({ label: "Export database as SQL…", onSelect: actions.onExportDatabase });
    items.push({ label: "Import SQL file…", onSelect: actions.onImport });
    items.push({ label: "Server activity…", separated: true, onSelect: actions.onActivity });
    if (actions.onRefresh) {
      items.push({ label: "Refresh", separated: true, onSelect: actions.onRefresh });
    }
    return items;
  }

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

  // Export reads rows, so it is offered wherever rows can be read — including
  // views, which export exactly as well as tables do.
  if (isRelation) {
    for (const { format, label, extension } of EXPORT_FORMATS) {
      items.push({
        label: `Export as ${label}…`,
        separated: format === EXPORT_FORMATS[0]!.format,
        onSelect: () => actions.onExport(format, extension),
      });
    }
  }

  if (node.kind === "table") {
    items.push({ label: "Import CSV file…", onSelect: actions.onImportCsv });
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
