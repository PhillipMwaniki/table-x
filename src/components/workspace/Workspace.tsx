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
import { BrowseBar } from "./BrowseBar";
import { ResultGrid } from "./ResultGrid";
import { HistoryPanel } from "./HistoryPanel";
import { TabBar } from "./TabBar";
import { ExportProgress } from "./ExportProgress";
import { SplitHandle } from "./SplitHandle";
import { CsvImportDialog } from "./CsvImportDialog";
import { ActivityPanel } from "./ActivityPanel";
import { PlanView } from "./PlanView";
import { DiagramView } from "./DiagramView";
import { DiffView } from "./DiffView";
import { CompareDialog } from "./CompareDialog";
import { PrivilegesPanel } from "./PrivilegesPanel";
import { ConfirmDestructive } from "./ConfirmDestructive";
import { NotebookView } from "./NotebookView";
import { StructureView } from "./StructureView";
import { InsertRowDialog } from "./InsertRowDialog";
import { CreateTableDialog } from "./CreateTableDialog";
import { NameDialog } from "./NameDialog";
import { DesignView } from "./DesignView";
import { Button, Spinner, cx } from "../ui/primitives";
import { ContextMenu } from "../ui/ContextMenu";
import { Dialog } from "../ui/Dialog";
import type { MenuItem } from "../ui/ContextMenu";
import { ipc, IpcError } from "@/lib/ipc";
import { hasOrderBy } from "@/lib/paging";
import { layoutKeyFor } from "@/lib/columns";
import { tableStatement } from "@/lib/browse";
import { readOnlyExplanation } from "@/lib/guarantees";
import { drop, selectFrom, truncate } from "@/lib/statements";
import { keyTypeFor } from "@/lib/design";
import { formatValue } from "@/lib/value";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useHistory } from "@/store/history";
import { useSnippets } from "@/store/snippets";
import { useCommands } from "@/store/commands";
import { useSettings } from "@/store/settings";
import { useConnections } from "@/store/connections";
import { useExports } from "@/store/exports";
import { useWorkspace } from "@/store/workspace";
import type { KeptResult } from "@/store/workspace";
import type {
  ColumnDef,
  ConnectionConfig,
  Design,
  DriverInfo,
  ExportFormat,
  HazardItem,
  NodeKind,
  QueryOutcome,
  Value,
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
    watchProgress,
    loadSession,
    loadCompletionFor,
    openScript: openScriptTab,
    openActivity,
    openPrivileges,
    openDiagram,
    openDiff,
    openNotebook,
    openDesign,
    setDesign,
    setCells,
    setTabView,
    renameNotebookTab,
    setTabError,
    setTabNotice,
    switchDatabase,
    bumpSchema,
    reconnect,
    applyEdit,
    goToPage,
    setClauses,
    keepResult,
    viewKept,
    discardKept,
    setKeptStatement,
    cancelQuery,
    beginTransaction,
    endTransaction,
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
  const pageSize = useSettings((s) => s.pageSize);
  const setPageSize = useSettings((s) => s.setPageSize);
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  const watchExports = useExports((s) => s.watch);
  const beginExport = useExports((s) => s.begin);
  const endExport = useExports((s) => s.end);
  /** A submission held back until its hazards are confirmed. */
  const [pending, setPending] = useState<{
    tabId: string;
    sql: string;
    hazards: HazardItem[];
    /**
     * What to do if it is confirmed.
     *
     * Absent for a statement, which is simply run. Present for a grid delete,
     * which is several statements and so cannot be described by one string —
     * the gate is about the decision, not about the shape of what follows it.
     */
    onConfirm?: () => void;
  } | null>(null);

  /** The table an insert form is open for, with its columns. */
  const [inserting, setInserting] = useState<{
    table: string;
    columns: ColumnDef[];
    /** Values to open the form with, by column name. Null means NULL. */
    initial?: Record<string, string | null> | undefined;
  } | null>(null);

  /** Rows picked in the grid, waiting for a format to be chosen. */
  const [exporting, setExporting] = useState<Value[][] | null>(null);

  /** The create menu, when the tree's + has been pressed. */
  const [creating, setCreating] = useState<{ x: number; y: number } | null>(null);
  /** A container being named, and what kind it is. */
  const [naming, setNaming] = useState<"database" | "schema" | null>(null);
  /** Whether the new-table form is open. */
  const [newTable, setNewTable] = useState(false);

  /** The schema a comparison is being set up for, if any. */
  // Selected straight off the store rather than derived: a selector that
  // builds an array returns a new one every call, and zustand compares by
  // identity — which is how this component learned to tear itself down.
  const connections = useConnections((s) => s.connections);
  const openConnections = useConnections((s) => s.open);
  /** Every driver this build has, for a design that has to pick one. */
  const drivers = useConnections((s) => s.drivers);
  /**
   * What this engine's DDL can do, which decides what the create menu offers.
   *
   * A missing driver means a connection whose descriptor has not loaded, and
   * nothing offered is the right answer for that moment.
   */
  const ddl = drivers.find((d) => d.id === connection.driver)?.capabilities.ddl;
  /** Whether this connection's link has failed, and whether it is being rebuilt. */
  const linkLost = useConnections((s) => s.broken.has(connection.id));
  const reconnecting = useConnections((s) => s.busy.has(connection.id));

  const [compare, setCompare] = useState<{
    connectionId: string;
    schema: string | null;
    label: string;
  } | null>(null);

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
  const transaction = useWorkspace((s) => s.transaction[connection.id]);

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
    void watchProgress();
  }, [watchExports, watchProgress]);

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
   * The one table a result's columns all came from.
   *
   * Not a field on the result: provenance lives per column, and `editable` is
   * exactly the guarantee that they agree on a single table. Reading it back
   * here rather than trusting a separate field keeps one source of truth.
   */
  const sourceOf = (result: StatementResult | undefined) => {
    if (result?.type !== "rows" || !result.editable) return null;
    const source = result.columns.find((c) => c.source)?.source;
    return source ? { table: source.table, schema: source.schema } : null;
  };

  /**
   * Open the insert form, once the table's columns are known.
   *
   * Fetched rather than taken from the result: a result carries the columns
   * the *query* returned, and inserting needs the ones the table has —
   * including the defaults and generated keys that decide which fields can be
   * left alone.
   */
  const beginInsert = async (from?: Value[]) => {
    const current = activeTab(connection.id);
    const result = current?.outcome?.statements[current.activeStatement];
    const source = sourceOf(result);
    if (!current || !source || result?.type !== "rows") return;

    try {
      const detail = await ipc.tableDetail(connection.id, source.table, source.schema);
      // Duplicating fills the form in from the row that was pointed at, keyed
      // by column *name* rather than position: the query's columns and the
      // table's are not the same list, and a `SELECT` of three columns out of
      // twenty must not fill in the first three fields of the form.
      const initial = from
        ? Object.fromEntries(
            result.columns.map((column, i): [string, string | null] => {
              const value = from[i];
              return [column.name, !value || value.kind === "null" ? null : formatValue(value)];
            }),
          )
        : undefined;
      setInserting({ table: detail.name, columns: detail.columns, initial });
    } catch (e) {
      reportJobFailure(e as IpcError, current.id, "Reading the table");
    }
  };

  /** Apply a filled-in insert form, then re-read so the new row is the server's. */
  const insertRow = async (table: string, values: [string, Value][]) => {
    const current = activeTab(connection.id);
    const source = sourceOf(current?.outcome?.statements[current.activeStatement]);
    if (!current) return;

    try {
      await ipc.insertRow(connection.id, {
        schema: source?.schema,
        table,
        values,
      });
      await run(connection.id, current.id);
      setTabNotice(connection.id, current.id, "Row inserted.");
    } catch (e) {
      reportJobFailure(e as IpcError, current.id, "Insert");
    }
  };

  /**
   * Confirm before deleting, using the same gate every other destructive path
   * goes through — a row removed from the grid is as gone as one removed by a
   * statement, and the connection that asks about one should ask about both.
   */
  const confirmDelete = async (rowIndexes: number[]) => {
    if (rowIndexes.length === 0) return;
    const current = activeTab(connection.id);
    if (!current) return;

    const report = await ipc
      .inspectStatement(connection.id, "DELETE FROM x WHERE id = 1")
      .catch(() => null);

    if (report?.confirms) {
      setPending({
        tabId: current.id,
        sql: `${rowIndexes.length} selected row${rowIndexes.length === 1 ? "" : "s"}`,
        hazards: [
          {
            summary: `deletes ${rowIndexes.length} row${rowIndexes.length === 1 ? "" : "s"} from ${
              sourceOf(current.outcome?.statements[current.activeStatement])?.table ?? "this table"
            }`,
            unbounded: false,
          },
        ],
        onConfirm: () => void deleteRows(rowIndexes),
      });
      return;
    }
    await deleteRows(rowIndexes);
  };

  /**
   * Delete the picked rows, one statement each.
   *
   * One at a time rather than a single `IN (…)`: each carries the full key the
   * row had when it was read, and the driver refuses anything matching other
   * than exactly one row. A combined statement could not make that check per
   * row, which is the guarantee worth keeping on the operation that cannot be
   * undone.
   */
  const deleteRows = async (rowIndexes: number[]) => {
    const current = activeTab(connection.id);
    const result = current?.outcome?.statements[current.activeStatement];
    const source = sourceOf(result);
    if (!current || result?.type !== "rows" || !source) return;

    let removed = 0;
    try {
      for (const index of rowIndexes) {
        const row = result.rows[index];
        if (!row) continue;
        const key: [string, Value][] = result.key_columns.map((name) => [
          name,
          row[result.columns.findIndex((c) => c.name === name)] ?? { kind: "null" },
        ]);
        await ipc.deleteRow(connection.id, {
          schema: source.schema,
          table: source.table,
          key,
        });
        removed += 1;
      }
      // Re-read rather than splicing the grid: the rows that remain are the
      // server's answer, and a locally patched view would disagree with it the
      // moment anything cascaded.
      await run(connection.id, current.id);
      setTabNotice(connection.id, current.id, `Deleted ${removed} row${removed === 1 ? "" : "s"}.`);
    } catch (e) {
      // Whatever was removed before the failure stays removed, so the grid is
      // refreshed either way rather than left describing rows that are gone.
      await run(connection.id, current.id);
      reportJobFailure(e as IpcError, current.id, `Delete after ${removed} row(s)`);
    }
  };

  /**
   * Put rows on the clipboard in one of the export formats.
   *
   * The text is built by the backend rather than here. An INSERT copied out of
   * this menu is a statement somebody will paste and run, and identifier
   * quoting, string escaping and NULL are exactly what a second implementation
   * in the webview would get subtly wrong -- so it uses the writers a file
   * export already uses.
   */
  const copyRows = async (request: {
    rows: Value[][];
    format: ExportFormat;
    table: string;
    header: boolean;
  }) => {
    const { rows, format, table, header } = request;
    const current = activeTab(connection.id);
    const result = current?.outcome?.statements[current.activeStatement];
    if (!current || result?.type !== "rows" || rows.length === 0) return;

    try {
      const text = await ipc.formatRows({
        connection_id: connection.id,
        format,
        table,
        header,
        columns: result.columns,
        rows,
      });
      await navigator.clipboard.writeText(text);
      // Said out loud, because a clipboard write leaves nothing on screen: the
      // only other way to find out whether it worked is to paste somewhere and
      // look.
      setTabNotice(
        connection.id,
        current.id,
        `Copied ${rows.length} row${rows.length === 1 ? "" : "s"} as ${format.toUpperCase()}.`,
      );
    } catch (e) {
      setTabError(connection.id, current.id, `Could not copy: ${(e as Error).message}`);
    }
  };

  /**
   * Write the rows picked out of the grid.
   *
   * The rows go to the backend rather than being re-queried: the selection was
   * made on rows already on screen, and no WHERE clause generally reproduces an
   * arbitrary set of them. What was shown is what gets written.
   */
  const exportSelection = async (rows: Value[][], format: ExportFormat, extension: string) => {
    const current = activeTab(connection.id);
    const result = current?.outcome?.statements[current.activeStatement];
    if (!current || result?.type !== "rows" || rows.length === 0) return;

    const path = await save({
      defaultPath: `${current.title}-selection.${extension}`,
      filters: [{ name: format.toUpperCase(), extensions: [extension] }],
    });
    if (!path) return;

    try {
      const written = await ipc.exportRows({
        connection_id: connection.id,
        path,
        format,
        table: current.title,
        columns: result.columns,
        rows,
      });
      setTabNotice(
        connection.id,
        current.id,
        `Exported ${written.toLocaleString()} selected row${written === 1 ? "" : "s"} to ${path}`,
      );
    } catch (e) {
      reportJobFailure(e as IpcError, current.id, "Export of the selection");
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
   * Run, unless this connection asks about destructive statements first.
   *
   * Every path that runs SQL goes through here rather than calling `run`
   * directly — a gate that one button skips is not a gate. The check is a round
   * trip, which is cheap next to the statement and is where the analysis lives
   * anyway: the same scanner backs the read-only guard and the MCP refusal, and
   * a second copy in the frontend would eventually disagree with them.
   */
  const runGuarded = async (tabId: string, sqlOverride?: string) => {
    const current = activeTab(connection.id)?.id === tabId ? activeTab(connection.id) : null;
    const sql = sqlOverride ?? current?.sql ?? "";
    if (!sql.trim()) return;

    try {
      const report = await ipc.inspectStatement(connection.id, sql);
      if (report.confirms && report.hazards.length > 0) {
        setPending({ tabId, sql, hazards: report.hazards });
        return;
      }
    } catch {
      // A failed check must not become a way to run unchecked: if the question
      // cannot be answered, the statement is held rather than waved through.
      setPending({
        tabId,
        sql,
        hazards: [
          {
            summary: "This statement could not be checked for destructive operations.",
            unbounded: false,
          },
        ],
      });
      return;
    }

    await run(connection.id, tabId, sqlOverride);
  };

  /**
   * Keep this notebook, asking for a name the first time.
   *
   * The tab remembers the id it was saved under, so a second save updates the
   * same notebook rather than leaving a trail of near-identical copies — which
   * is what happens when "save" always means "save a new one".
   */
  const saveNotebook = async (tabId: string) => {
    const current = activeTab(connection.id);
    if (!current || current.id !== tabId) return;

    const name = current.notebookId
      ? current.title
      : window.prompt("Name this notebook", current.title);
    if (!name?.trim()) return;

    try {
      const saved = await ipc.saveNotebook({
        id: current.notebookId ?? crypto.randomUUID(),
        name: name.trim(),
        cells: current.cells ?? [],
        connection_id: connection.id,
        created_at: "",
        updated_at: "",
      });
      renameNotebookTab(connection.id, tabId, saved.id, saved.name);
      setTabNotice(connection.id, tabId, `Saved as ${saved.name}`);
    } catch (e) {
      reportJobFailure(e as IpcError, tabId, "Saving the notebook");
    }
  };

  /**
   * Run one statement for a notebook cell and hand back its result.
   *
   * Goes through the same hazard check as everything else — a notebook must not
   * become the one route that skips the confirmation. The dialog is modal, so a
   * held statement resolves to null rather than waiting: the cell reports
   * nothing ran, which is true.
   */
  const runCell = async (sql: string): Promise<StatementResult | null> => {
    const report = await ipc.inspectStatement(connection.id, sql);
    if (report.confirms && report.hazards.length > 0) {
      setPending({ tabId: tab?.id ?? "", sql, hazards: report.hazards });
      return null;
    }

    const outcome = await ipc.execute({
      connection_id: connection.id,
      sql,
      max_rows: pageSize,
    });
    return outcome.statements[0] ?? null;
  };

  /**
   * Compare this schema with another, and write the migration between them.
   *
   * The script is generated for *this* connection's engine, because this is the
   * side it would be run against. Generating it in the other side's dialect
   * would produce statements that are correct about the wrong database.
   */
  const runCompare = async (to: { connectionId: string; schema: string | null; label: string }) => {
    const from = compare;
    setCompare(null);
    if (!from) return;

    const id = crypto.randomUUID();
    const current = activeTab(connection.id);
    beginExport(id, `Comparing ${from.label}`, "tables");
    try {
      const report = await ipc.compareSchemas({
        id,
        from: { connection_id: from.connectionId, schema: from.schema, label: from.label },
        to: { connection_id: to.connectionId, schema: to.schema, label: to.label },
        driver: connection.driver,
      });
      openDiff(connection.id, `${from.label} ⇄ ${to.label}`, report);
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, "Comparison");
    } finally {
      endExport(id);
    }
  };

  /**
   * Make a database or a schema, then show it.
   *
   * The tree is rebuilt rather than patched: what a server holds is the
   * server's answer, and a name added to the list optimistically would be a
   * claim this app is in no position to make.
   */
  const createContainer = async (kind: "database" | "schema", name: string) => {
    const current = activeTab(connection.id);
    try {
      if (kind === "database") {
        await ipc.createDatabase(connection.id, name);
      } else {
        await ipc.createSchema(connection.id, name);
      }
      bumpSchema(connection.id);
      if (current) setTabNotice(connection.id, current.id, `Created ${kind} ${name}.`);
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, `Creating the ${kind}`);
    }
  };

  /**
   * Compare a design against the connected database.
   *
   * The direction is the one a design is for: the database is what the script
   * would run against, the design is what it is being brought to. It opens in
   * the same diff view a schema comparison does, because it is the same thing —
   * one side simply happens to be a document rather than a server.
   */
  const syncDesign = async (design: Design) => {
    const id = crypto.randomUUID();
    const current = activeTab(connection.id);
    beginExport(id, `Comparing ${connection.name}`, "tables");
    try {
      const report = await ipc.designSync(design.id, connection.id, database ?? undefined);
      openDiff(connection.id, `${connection.name} ⇄ ${design.name}`, report);
    } catch (e) {
      reportJobFailure(e as IpcError, current?.id, "Comparison with the design");
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
    // What the tab runs, clauses included, rather than the bare SELECT a
    // table tab holds: the saved query should bring back the same rows.
    void saveSnippet(name, tableStatement(tab)).then(() => {
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
  /**
   * Whether the editor holds something a preview can be made of.
   *
   * A first-word test, because the button is drawn on every keystroke and
   * the real decision is the backend's: a statement that starts like an
   * UPDATE but is shaped in a way the rewrite cannot place is refused there,
   * with the reason shown where the error would go.
   */
  const canPreview = tab?.kind === "query" && /^\s*(?:update|insert|delete)\b/i.test(tab.sql);

  /**
   * Run the statement as a read of the rows it would touch.
   *
   * The preview is a SELECT, so it goes through the same gate every run does
   * and comes back as an ordinary result; the notice under it is what says it
   * was a preview. Nothing is written — which is the point, and the reason a
   * run is not a good enough way to see what a statement will do.
   */
  const previewChanges = async () => {
    if (!tab || !canPreview) return;
    let preview;
    try {
      preview = await ipc.previewStatement(connection.id, tab.sql);
    } catch (e) {
      setTabError(connection.id, tab.id, (e as Error).message);
      return;
    }
    await runGuarded(tab.id, preview.select);
    setTabNotice(
      connection.id,
      tab.id,
      `Preview only — ${preview.note} Nothing was written; expressions were evaluated now and will be evaluated again by the write.`,
    );
  };

  const formatCurrentSql = async () => {
    if (!tab?.sql.trim()) return;
    try {
      const formatted = await ipc.formatSql(tab.sql);
      if (formatted.trim()) setSql(connection.id, tab.id, formatted);
    } catch (e) {
      setTabError(connection.id, tab.id, (e as Error).message);
    }
  };

  /**
   * What the result pane shows: a kept result when one is being looked at,
   * otherwise the latest. A kept result is read-only by construction — it is a
   * copy of the rows as they were, and an edit made against it would be
   * matched on values the table may no longer hold.
   */
  const shown = tab?.viewing ? (tab.kept?.find((k) => k.id === tab.viewing) ?? null) : null;
  const outcome = shown?.outcome ?? tab?.outcome ?? null;
  const statementIndex = shown ? shown.activeStatement : (tab?.activeStatement ?? 0);
  const active = outcome?.statements[statementIndex];

  /** Run into a new result: keep the one on screen, then run as usual. */
  const runKeeping = (tabId: string, sqlOverride?: string) => {
    keepResult(connection.id, tabId);
    void runGuarded(tabId, sqlOverride);
  };

  /*
   * Sorting, the row filter, the per-column filters and the scroll position are
   * display state, so they live in the grid rather than on the tab -- see the
   * note on `Tab.offset`. Only the active tab has a grid, so switching away
   * unmounts it; this key is what the grid remembers that state under, and
   * what brings it back when the tab is returned to.
   *
   * The column names are in the key because the filters are keyed by column
   * index. Re-running the same query keeps your filter, which is what you want;
   * running a different one drops it, rather than applying "> 100" to whatever
   * now occupies column three.
   */
  const gridKey =
    tab && active?.type === "rows"
      ? `${tab.id}:${shown?.id ?? "latest"}:${statementIndex}:${active.columns.map((c) => c.name).join("\0")}`
      : null;

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
        run: () => void runGuarded(tab.id),
      },
      {
        id: "ws.run-keep",
        title: "Run into a new result",
        group: "Query",
        shortcut: "Ctrl+Shift+Enter",
        run: () => runKeeping(tab.id),
      },
      {
        id: "ws.keep",
        title: "Keep this result",
        group: "Query",
        run: () => keepResult(connection.id, tab.id),
      },
      ...(canPreview
        ? [
            {
              id: "ws.preview",
              title: "Preview changes as a SELECT",
              group: "Query",
              run: () => void previewChanges(),
            },
          ]
        : []),
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
        id: "ws.notebook",
        title: "New notebook",
        group: "Query",
        run: () => openNotebook(connection.id),
      },
      {
        id: "ws.privileges",
        title: "Show privileges and roles",
        group: "Data",
        run: () => openPrivileges(connection.id),
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

  /**
   * The same question answered specifically enough to act on.
   *
   * Three situations render as "read-only" and the remedy differs for each, so
   * this needs the result's own key columns rather than only the connection and
   * the driver.
   */
  const readOnlyDetail = useMemo(
    () =>
      readOnlyExplanation({
        connectionReadOnly: connection.read_only,
        driverName: driver?.name ?? "This driver",
        hasProvenance: driver?.capabilities.column_provenance ?? false,
        keyColumns: active?.type === "rows" ? active.key_columns : [],
      }),
    [connection.read_only, driver, active],
  );

  return (
    <div className="flex min-h-0 flex-1">
      <aside className="flex w-60 shrink-0 flex-col overflow-y-auto border-r border-border bg-surface-1">
        <SchemaTree
          connectionId={connection.id}
          activeDatabase={database}
          // A table tab is showing one object; a query tab is showing a
          // statement, which may touch several and marks none of them.
          activeObject={
            tab?.kind === "table"
              ? { name: tab.title, schema: tab.schema, database: tab.database ?? undefined }
              : null
          }
          onOpenTable={(node) =>
            openTable(connection.id, {
              title: node.name,
              // The driver already quoted and qualified this for its own
              // engine, so the UI never assembles a name itself.
              qualified: node.qualified ?? node.name,
              schema: node.schema,
            })
          }
          onSelectDatabase={(name) => void switchDatabase(connection.id, name)}
          onOpenScript={(node) => void openScript(node)}
          onContextMenu={(node, at, refresh) => setMenu({ node, x: at.x, y: at.y, refresh })}
          onCreate={(at) => setCreating(at)}
        />
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <TabBar connectionId={connection.id} />
        <ExportProgress />

        {/* A broken link is a fact about the connection, not about whichever tab
            happens to be in front, so it is reported once here rather than in
            every tab's error banner — and it stays put while the user moves
            between tabs looking for what survived.

            Warn rather than danger: nothing has been lost yet. The tabs, the
            statements, and the last results are all still here, and saying so
            is most of the reason this strip exists. */}
        {linkLost && (
          <div
            role="alert"
            className="flex shrink-0 items-center gap-2 border-b border-warn/30 bg-warn/10 px-2 py-1"
          >
            <span className="min-w-0 flex-1 text-[11px] text-warn" data-selectable>
              The link to this server is gone. Your tabs and results are kept — reconnect to run
              anything.
            </span>
            <Button
              variant="secondary"
              className="h-6 shrink-0"
              busy={reconnecting}
              onClick={() => void reconnect(connection.id)}
              title="Open a new link to the server, keeping these tabs and results"
            >
              Reconnect
            </Button>
          </div>
        )}

        {tab ? (
          <>
            <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
              {/* Every control here acts on a statement, and an activity tab has
                  none — it carries its own refresh instead. */}
              {tab.kind === "activity" ||
              tab.kind === "diagram" ||
              tab.kind === "diff" ||
              tab.kind === "notebook" ||
              tab.kind === "design" ||
              tab.kind === "privileges" ? (
                <span className="text-[11px] text-text-muted">
                  {tab.kind === "activity"
                    ? "Live view of the server. Nothing here is cached."
                    : tab.kind === "diff"
                      ? "A comparison and the script that would settle it. Nothing here runs."
                      : tab.kind === "privileges"
                        ? "Principals and their grants, in the engine's own words."
                        : tab.kind === "notebook"
                          ? "Prose and queries together. Results are not saved with it."
                          : tab.kind === "design"
                            ? "A design, not a database. Drag tables about; nothing here runs."
                            : "Tables and the keys between them. Drag to pan, scroll to zoom."}
                </span>
              ) : (
                <>
                  {/* While something is running, the primary button stops it
                      rather than sitting there spinning. A spinner with no way
                      out is the state this whole feature exists to remove —
                      and it is only offered where the driver can actually do
                      it, rather than being drawn and then failing. */}
                  {tab.running && driver?.capabilities.cancel ? (
                    <Button
                      variant="danger"
                      onClick={() => void cancelQuery(connection.id)}
                      className="h-6"
                      title="Stop this statement"
                    >
                      Cancel
                    </Button>
                  ) : (
                    <Button
                      variant="primary"
                      onClick={() => void runGuarded(tab.id)}
                      busy={tab.running}
                      disabled={!tab.sql.trim()}
                      className="h-6"
                    >
                      {tab.kind === "table" ? "Refresh" : "Run"}
                    </Button>
                  )}
                  {tab.kind === "query" && (
                    <span className="text-[10.5px] text-text-muted">Ctrl+Enter</span>
                  )}

                  {/* Two views of one table, so a toggle rather than a second
                      tab: looking at a table means moving between what is in
                      it and how it is built, repeatedly. */}
                  {tab.kind === "table" && (
                    <span className="flex overflow-hidden rounded border border-border">
                      {(["data", "structure"] as const).map((mode) => (
                        <button
                          key={mode}
                          onClick={() => setTabView(connection.id, tab.id, mode)}
                          aria-pressed={(tab.view ?? "data") === mode}
                          className={cx(
                            "px-2 py-0.5 text-[11px] capitalize",
                            (tab.view ?? "data") === mode
                              ? "bg-surface-3 text-text"
                              : "text-text-muted hover:bg-surface-2 hover:text-text",
                          )}
                        >
                          {mode}
                        </button>
                      ))}
                    </span>
                  )}

                  {/* Only where the engine has them: ClickHouse would get three
                      buttons that can do nothing but produce an error.

                      On table tabs as well as query ones. The transaction
                      belongs to the connection rather than to a tab, so hiding
                      the badge on the tab somebody happened to switch to would
                      hide the fact that their edits are still uncommitted —
                      and a grid edit lands inside it just as a statement does. */}
                  {transaction?.supported && (
                    <TransactionControls
                      open={transaction.open}
                      readOnly={connection.read_only}
                      onBegin={() => void beginTransaction(connection.id)}
                      onEnd={(how) => void endTransaction(connection.id, how)}
                    />
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
                  {/* Only while the editor holds a write: a button that
                      says "preview changes" over a SELECT would be asking a
                      question the statement does not raise. */}
                  {canPreview && (
                    <Button
                      variant="ghost"
                      className="h-6"
                      disabled={tab.running}
                      onClick={() => void previewChanges()}
                      title="See the rows this would change, as a SELECT. Nothing is written."
                    >
                      Preview changes
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
                    className="h-6"
                    disabled={!tab.outcome}
                    onClick={() => keepResult(connection.id, tab.id)}
                    title="Keep this result so the next run does not replace it. Ctrl+Shift+Enter keeps and runs in one step."
                  >
                    Keep result
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
            {tab.kind === "design" ? (
              tab.design ? (
                <DesignView
                  design={tab.design}
                  onChange={(design) => setDesign(connection.id, tab.id, design)}
                  onScript={(report) =>
                    openScriptTab(connection.id, {
                      title: `Create — ${report.to}`,
                      sql: report.statements.map((s) => s.sql).join("\n\n"),
                    })
                  }
                  onSync={() => void syncDesign(tab.design!)}
                />
              ) : null
            ) : tab.kind === "notebook" ? (
              <NotebookView
                cells={tab.cells ?? []}
                saved={Boolean(tab.notebookId)}
                onChange={(cells) => setCells(connection.id, tab.id, cells)}
                onRunGuarded={runCell}
                onSave={() => void saveNotebook(tab.id)}
              />
            ) : tab.kind === "privileges" ? (
              <PrivilegesPanel
                connectionId={connection.id}
                quote={driver?.capabilities.identifier_quote ?? '"'}
                onOpenScript={(title, sql) => openScriptTab(connection.id, { title, sql })}
              />
            ) : tab.kind === "diff" ? (
              tab.diff ? (
                <DiffView
                  report={tab.diff}
                  onOpenScript={(sql) =>
                    openScriptTab(connection.id, { title: `Migration — ${tab.title}`, sql })
                  }
                />
              ) : null
            ) : tab.kind === "diagram" ? (
              <DiagramView connectionId={connection.id} schema={tab.schema ?? null} />
            ) : tab.kind === "activity" ? (
              <ActivityPanel
                connectionId={connection.id}
                readOnly={connection.read_only}
                onOpenQuery={(sql) => openScriptTab(connection.id, { title: "Statement", sql })}
              />
            ) : tab.kind === "table" && tab.view === "structure" ? (
              <StructureView
                key={`${connection.id}:${tab.schema ?? ""}:${tab.title}`}
                connectionId={connection.id}
                table={tab.title}
                schema={tab.schema}
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
                        onRun={(text) => void runGuarded(tab.id, text)}
                        onRunKeep={(text) => runKeeping(tab.id, text)}
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

                {/* Keyed on the tab so the drafts belong to it: switching
                    tabs must not carry half-typed WHERE from one table to
                    the next. */}
                {tab.kind === "table" && (
                  <BrowseBar
                    key={tab.id}
                    where={tab.where ?? ""}
                    orderBy={tab.orderBy ?? ""}
                    busy={tab.running}
                    onApply={(clauses) => void setClauses(connection.id, tab.id, clauses)}
                  />
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

                  {/* A script of many statements says where it is. Shown
                      above whatever result is already there rather than in
                      place of it: a re-run keeps the previous rows on screen
                      until the new ones arrive. */}
                  {tab.running && tab.progress && tab.progress.total > 1 && (
                    <div
                      role="progressbar"
                      aria-valuemin={0}
                      aria-valuemax={tab.progress.total}
                      aria-valuenow={tab.progress.done}
                      aria-label="Statements run"
                      className="flex shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 py-1 text-[10.5px] text-text-muted"
                    >
                      <span className="whitespace-nowrap tabular-nums">
                        {tab.progress.done} of {tab.progress.total} statements
                      </span>
                      <div className="h-1 flex-1 overflow-hidden rounded bg-surface-2">
                        <div
                          className="h-full bg-accent transition-[width] duration-150"
                          style={{
                            width: `${(100 * tab.progress.done) / tab.progress.total}%`,
                          }}
                        />
                      </div>
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

                  {tab.kept && tab.kept.length > 0 && (
                    <KeptTabs
                      kept={tab.kept}
                      viewing={tab.viewing ?? null}
                      latest={tab.outcome}
                      onSelect={(id) => viewKept(connection.id, tab.id, id)}
                      onDiscard={(id) => discardKept(connection.id, tab.id, id)}
                    />
                  )}

                  {outcome && outcome.statements.length > 1 && (
                    <StatementTabs
                      statements={outcome.statements}
                      active={statementIndex}
                      onSelect={(i) =>
                        shown
                          ? setKeptStatement(connection.id, tab.id, shown.id, i)
                          : setActiveStatement(connection.id, tab.id, i)
                      }
                    />
                  )}

                  {tab.plan ? (
                    <PlanView plan={tab.plan} onClose={() => clearPlan(connection.id, tab.id)} />
                  ) : tab.running && !outcome ? (
                    <div className="flex flex-1 items-center justify-center">
                      <Spinner className="text-text-muted" />
                    </div>
                  ) : active?.type === "rows" && shown ? (
                    <ResultGrid
                      key={gridKey ?? undefined}
                      memoryKey={gridKey ?? undefined}
                      layoutKey={layoutKeyFor(connection.id, tab, active)}
                      // The copy, marked read-only however the run came back:
                      // its rows are what the table held at the time, and
                      // there is no undo path for a change made against them.
                      result={{ ...active, editable: false }}
                      onEdit={() => Promise.resolve()}
                      onExportRows={(rows) => setExporting(rows)}
                      onCopyRows={(request) => void copyRows(request)}
                      readOnlyDetail={{
                        reason: `This is a result kept at ${timeOf(shown.at)}.`,
                        remedy: "Look at the latest result to edit, or run again.",
                      }}
                    />
                  ) : active?.type === "rows" ? (
                    <ResultGrid
                      /* Keyed so each result has a grid of its own: without a
                         key, React reuses one instance across tab switches and
                         the filter you typed follows you to the next tab. The
                         same key names what the grid remembers -- see `gridKey`. */
                      key={gridKey ?? undefined}
                      memoryKey={gridKey ?? undefined}
                      layoutKey={layoutKeyFor(connection.id, tab, active)}
                      result={active}
                      onEdit={(row, col, next) => applyEdit(connection.id, tab.id, row, col, next)}
                      paging={{
                        offset: tab.offset,
                        limit: tab.limit || pageSize,
                        ordered: hasOrderBy(tableStatement(tab)),
                        busy: tab.running,
                        onGoTo: (offset) => void goToPage(connection.id, tab.id, offset),
                        onPageSize: (rows) => {
                          // Back to the first page: keeping the offset would land
                          // somewhere unrelated to where the reader was.
                          setPageSize(rows);
                          void goToPage(connection.id, tab.id, 0);
                        },
                        orderableBy: active.key_columns,
                        onOrderBy:
                          active.key_columns.length > 0
                            ? () => {
                                const quote = driver?.capabilities.identifier_quote ?? '"';
                                const close = quote === "[" ? "]" : quote;
                                const keys = active.key_columns
                                  .map(
                                    (c) => `${quote}${c.replaceAll(close, close + close)}${close}`,
                                  )
                                  .join(", ");
                                // A table tab has a box for this; a query tab
                                // has only its text.
                                if (tab.kind === "table") {
                                  void setClauses(connection.id, tab.id, {
                                    where: tab.where ?? "",
                                    orderBy: keys,
                                  });
                                  return;
                                }
                                setSql(
                                  connection.id,
                                  tab.id,
                                  `${tab.sql.trimEnd()} ORDER BY ${keys}`,
                                );
                                void goToPage(connection.id, tab.id, 0);
                              }
                            : undefined,
                      }}
                      onExportRows={(rows) => setExporting(rows)}
                      onCopyRows={(request) => void copyRows(request)}
                      onDuplicateRow={sourceOf(active) ? (row) => void beginInsert(row) : undefined}
                      readOnlyDetail={readOnlyDetail}
                      onInsertRow={active.editable ? () => void beginInsert() : undefined}
                      onDeleteRows={
                        active.editable ? (rows) => void confirmDelete(rows) : undefined
                      }
                    />
                  ) : active?.type === "affected" ? (
                    <div className="flex flex-1 items-center justify-center text-[12px] text-text-muted">
                      {active.rows_affected} row{active.rows_affected === 1 ? "" : "s"} affected
                      {active.last_insert_id != null &&
                        ` · last insert id ${active.last_insert_id}`}
                    </div>
                  ) : (
                    <div className="flex flex-1 items-center justify-center px-6 text-center text-[12px] text-text-muted">
                      {tab.error
                        ? "Fix the statement and run again."
                        : "Write a query and press Ctrl+Enter, or click a table in the sidebar."}
                    </div>
                  )}

                  {outcome && (
                    <div className="flex h-5 shrink-0 items-center gap-3 border-t border-border bg-surface-1 px-2 text-[10.5px] text-text-muted">
                      {shown && <span className="text-accent">Kept {timeOf(shown.at)}</span>}
                      <span>{outcome.elapsed_ms} ms</span>
                      {outcome.statements.length > 1 && (
                        <span>{outcome.statements.length} statements</span>
                      )}
                      {outcome.notices.map((n, i) => (
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
              foreignKeys: driver?.capabilities.foreign_keys ?? false,
              privileges: driver?.capabilities.privileges ?? false,
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
              onPrivileges: () => openPrivileges(connection.id),
              onDiagram: () =>
                openDiagram(
                  connection.id,
                  menu.node.kind === "schema" ? menu.node.name : (menu.node.schema ?? null),
                ),
              onCompare: () => {
                const schema =
                  menu.node.kind === "schema" ? menu.node.name : (menu.node.schema ?? null);
                setCompare({
                  connectionId: connection.id,
                  schema,
                  label: `${connection.name}${schema ? ` · ${schema}` : ""}`,
                });
              },
              onImportCsv: () => void importCsv(menu.node),
              onRefresh: menu.refresh,
            },
          )}
          onClose={() => setMenu(null)}
        />
      )}

      {exporting && exporting.length > 0 && (
        <Dialog
          open
          onClose={() => setExporting(null)}
          title={`Export ${exporting.length.toLocaleString()} selected row${exporting.length === 1 ? "" : "s"}`}
          description="Written from what is on screen, in the same formats a whole table exports to."
        >
          <div className="flex flex-col gap-1">
            {EXPORT_FORMATS.map(({ format, label, extension }) => (
              <button
                key={format}
                onClick={() => {
                  const rows = exporting;
                  setExporting(null);
                  void exportSelection(rows, format, extension);
                }}
                className="rounded-md border border-border px-3 py-2 text-left text-[12px] hover:border-accent hover:bg-surface-2"
              >
                {label}
              </button>
            ))}
          </div>
        </Dialog>
      )}

      {pending && (
        <ConfirmDestructive
          open
          connectionName={connection.name}
          hazards={pending.hazards}
          sql={pending.sql}
          onCancel={() => setPending(null)}
          onConfirm={() => {
            const held = pending;
            setPending(null);
            if (held.onConfirm) {
              held.onConfirm();
              return;
            }
            // The statement the dialog showed, not whatever the editor holds
            // now — they can differ if something changed while it was open.
            void run(connection.id, held.tabId, held.sql);
          }}
        />
      )}

      {inserting && (
        <InsertRowDialog
          open
          table={inserting.table}
          columns={inserting.columns}
          initial={inserting.initial}
          onClose={() => setInserting(null)}
          onInsert={(values) => {
            const table = inserting.table;
            setInserting(null);
            void insertRow(table, values);
          }}
        />
      )}

      {creating && (
        <ContextMenu
          x={creating.x}
          y={creating.y}
          onClose={() => setCreating(null)}
          items={[
            {
              label: "New table…",
              onSelect: () => setNewTable(true),
            },
            {
              label: "New schema…",
              separated: true,
              // Offered only where the engine has a statement for it. Oracle
              // has schemas and no way to make one that is not `CREATE USER`.
              disabledReason: ddl?.create_schema
                ? undefined
                : `${connection.driver} cannot create a schema`,
              onSelect: ddl?.create_schema ? () => setNaming("schema") : undefined,
            },
            {
              label: "New database…",
              disabledReason: ddl?.create_database
                ? undefined
                : `${connection.driver} cannot create a database`,
              onSelect: ddl?.create_database ? () => setNaming("database") : undefined,
            },
          ]}
        />
      )}

      {naming && (
        <NameDialog
          open
          title={naming === "database" ? "New database" : "New schema"}
          label="Name"
          description={
            naming === "database"
              ? "Created empty. Nothing is switched to it until you open it."
              : "Created in the database this connection is on."
          }
          onClose={() => setNaming(null)}
          onSubmit={(name) => {
            const kind = naming;
            setNaming(null);
            void createContainer(kind, name);
          }}
        />
      )}

      {newTable && (
        <CreateTableDialog
          open
          connectionId={connection.id}
          schema={database ?? undefined}
          types={drivers.find((d) => d.id === connection.driver)?.column_types ?? []}
          keyType={keyTypeFor(connection.driver)}
          onClose={() => setNewTable(false)}
          onCreated={(name) => {
            setNewTable(false);
            bumpSchema(connection.id);
            const current = activeTab(connection.id);
            if (current) setTabNotice(connection.id, current.id, `Created table ${name}.`);
          }}
        />
      )}

      {compare && (
        <CompareDialog
          open
          from={compare}
          connections={connections}
          connected={openConnections}
          onClose={() => setCompare(null)}
          onCompare={(to) => void runCompare(to)}
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
        onOpenNotebook={(notebook) => openNotebook(connection.id, notebook)}
        schema={database}
        drivers={drivers}
        onOpenDesign={(design) => openDesign(connection.id, design)}
        onPick={(sql) => tab && setSql(connection.id, tab.id, sql)}
        onRun={(sql) => {
          if (!tab) return;
          setSql(connection.id, tab.id, sql);
          void runGuarded(tab.id, sql);
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
  options: {
    driver: string;
    tableScripts: boolean;
    foreignKeys: boolean;
    privileges: boolean;
  },
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
    onPrivileges: () => void;
    onDiagram: () => void;
    onCompare: () => void;
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
    if (options.foreignKeys) {
      items.push({ label: "Diagram…", onSelect: actions.onDiagram });
    }
    items.push({ label: "Compare with…", onSelect: actions.onCompare });
    items.push({ label: "Server activity…", separated: true, onSelect: actions.onActivity });
    if (options.privileges) {
      items.push({ label: "Privileges and roles…", onSelect: actions.onPrivileges });
    }
    if (actions.onRefresh) {
      items.push({ label: "Refresh", separated: true, onSelect: actions.onRefresh });
    }
    return items;
  }

  if (node.kind === "schema" && options.foreignKeys) {
    items.push({ label: "Diagram…", onSelect: actions.onDiagram });
    items.push({ label: "Compare with…", onSelect: actions.onCompare });
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

/**
 * Begin, commit, roll back — and the badge that says which.
 *
 * The badge is the point. Three of the four engines here leave a failed
 * statement's transaction open and refuse everything after it, and somebody who
 * cannot see that they are inside one reads the resulting wall of errors as the
 * database being broken. The buttons are the smaller half of this feature.
 *
 * Nothing here is offered on a read-only connection: a transaction that can only
 * contain reads is a lock held for no reason.
 */
function TransactionControls({
  open,
  readOnly,
  onBegin,
  onEnd,
}: {
  open: boolean;
  readOnly: boolean;
  onBegin: () => void;
  onEnd: (how: "commit" | "rollback") => void;
}) {
  if (readOnly) return null;

  if (!open) {
    return (
      <Button
        variant="ghost"
        className="h-6"
        onClick={onBegin}
        title="Open a transaction, so the next statements can be taken back"
      >
        Begin
      </Button>
    );
  }

  return (
    <span className="flex items-center gap-1.5">
      {/* Loud on purpose. Uncommitted work that the user has forgotten about is
          work that a disconnect throws away. */}
      <span
        className="rounded bg-warn/15 px-1.5 py-0.5 text-[10px] font-medium text-warn"
        title="Nothing since Begin is visible to anyone else yet. Grid editing is unavailable until this is settled; statements in the editor run inside it."
      >
        IN TRANSACTION
      </span>
      <Button variant="ghost" className="h-6" onClick={() => onEnd("commit")} title="Keep it all">
        Commit
      </Button>
      <Button
        variant="danger"
        className="h-6"
        onClick={() => onEnd("rollback")}
        title="Discard everything since Begin"
      >
        Roll back
      </Button>
    </span>
  );
}

/** A clock time, to the second, the way a result's tab labels itself. */
function timeOf(at: number): string {
  return new Date(at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** What a result's tab says about it, in the space a tab has. */
function describeOutcome(outcome: QueryOutcome): string {
  const first = outcome.statements[0];
  if (!first) return "no result";
  const more = outcome.statements.length > 1 ? ` +${outcome.statements.length - 1}` : "";
  if (first.type === "rows") {
    return `${first.rows.length}${first.truncated ? "+" : ""} rows${more}`;
  }
  return `${first.rows_affected} affected${more}`;
}

/**
 * The results a tab is holding, with the latest first.
 *
 * Each kept one is labelled with the time it came back, because that is the
 * thing that makes it different from the latest: the same statement, an
 * earlier moment. The latest has no time, since it is the one that is current.
 */
function KeptTabs({
  kept,
  viewing,
  latest,
  onSelect,
  onDiscard,
}: {
  kept: KeptResult[];
  viewing: string | null;
  latest: QueryOutcome | null;
  onSelect: (id: string | null) => void;
  onDiscard: (id: string) => void;
}) {
  const tabClass = (current: boolean) =>
    cx(
      "flex shrink-0 items-center gap-1 rounded px-2 py-0.5 text-[10.5px] whitespace-nowrap",
      current ? "bg-surface-3 text-text" : "text-text-muted hover:bg-surface-2",
    );
  return (
    <div
      role="tablist"
      aria-label="Results"
      className="flex h-6 shrink-0 items-center gap-px overflow-x-auto border-b border-border bg-surface-1 px-1"
    >
      <button
        role="tab"
        aria-selected={viewing === null}
        onClick={() => onSelect(null)}
        className={tabClass(viewing === null)}
        title="The result of the last run"
      >
        Latest{latest ? ` · ${describeOutcome(latest)}` : ""}
      </button>
      {kept.map((k) => (
        <span key={k.id} className={tabClass(viewing === k.id)}>
          <button
            role="tab"
            aria-selected={viewing === k.id}
            onClick={() => onSelect(k.id)}
            title={k.sql}
          >
            {timeOf(k.at)} · {describeOutcome(k.outcome)}
          </button>
          <button
            onClick={() => onDiscard(k.id)}
            className="rounded px-0.5 text-text-muted/60 hover:text-text"
            title="Let this result go"
            aria-label={`Discard the result kept at ${timeOf(k.at)}`}
          >
            ✕
          </button>
        </span>
      ))}
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
