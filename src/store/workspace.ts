/**
 * Tabs, and the state each one holds.
 *
 * A tab is the unit of work: one editor and one result, or one table's rows.
 * Tabs belong to a connection, so switching connections in the sidebar shows
 * that connection's tabs rather than resetting anything.
 *
 * The database a tab was opened against is part of the tab, not just a label.
 * A session points at one database at a time, so activating a tab that belongs
 * to another one switches the session back — otherwise a query typed against
 * `app_staging` would silently run against whatever was selected last.
 */

import { create } from "zustand";
import { ipc, IpcError } from "@/lib/ipc";
import type { CompletionScope, QueryOutcome, RowEdit, StatementResult, Value } from "@/lib/types";

/** One applied cell edit, retained so it can be reversed. */
export interface AppliedEdit {
  rowIndex: number;
  columnIndex: number;
  before: Value;
  after: Value;
  /** The statement needed to put it back. */
  inverse: RowEdit;
}

export interface QueryError {
  message: string;
  /** 1-based character offset, used to underline the offending token. */
  position?: number | undefined;
  code?: string | undefined;
}

/**
 * A table tab shows one object's rows with no editor; a query tab is an editor
 * over a result. They share everything else, so they share a shape.
 */
export type TabKind = "query" | "table";

export interface Tab {
  id: string;
  kind: TabKind;
  title: string;
  /** The database this tab's statements belong to, when the engine has any. */
  database: string | null;
  /** Where a table tab's object lives, shown as the tab's context line. */
  schema?: string | undefined;

  sql: string;
  outcome: QueryOutcome | null;
  error: QueryError | null;
  running: boolean;
  /** Index of the statement whose results are shown. */
  activeStatement: number;
  undo: AppliedEdit[];
  redo: AppliedEdit[];
}

/**
 * The tab list for a connection that has none yet.
 *
 * One shared instance, because a zustand selector's result is compared by
 * identity: returning a fresh `[]` for a missing key makes every render look
 * like a change, and React tears down the component with "Maximum update depth
 * exceeded" rather than rendering it.
 */
const NO_TABS: readonly Tab[] = Object.freeze([]);

/** Tabs for a connection, stable for connections with none. */
export function tabsOf(state: { tabs: Record<string, Tab[]> }, connectionId: string): Tab[] {
  return state.tabs[connectionId] ?? (NO_TABS as Tab[]);
}

let counter = 0;
function nextId(): string {
  counter += 1;
  return `tab-${counter}`;
}

function blankTab(overrides: Partial<Tab> = {}): Tab {
  return {
    id: nextId(),
    kind: "query",
    title: "Query",
    database: null,
    sql: "",
    outcome: null,
    error: null,
    running: false,
    activeStatement: 0,
    undo: [],
    redo: [],
    ...overrides,
  };
}

interface WorkspaceState {
  /** Tabs per connection id, in display order. */
  tabs: Record<string, Tab[]>;
  /** Active tab id per connection id. */
  active: Record<string, string>;
  /** Autocomplete data per connection — it describes the session, not a tab. */
  completion: Record<string, CompletionScope | null>;
  /** The database each session is currently pointed at. */
  database: Record<string, string | null>;
  /** Set while a database switch is in flight, so the tree can show it. */
  switching: Record<string, boolean>;

  tabsFor: (connectionId: string) => Tab[];
  activeTab: (connectionId: string) => Tab | null;

  openQuery: (connectionId: string) => void;
  openTable: (
    connectionId: string,
    object: { title: string; qualified: string; schema?: string | undefined },
  ) => void;
  /** A query tab that opens with content already in it, e.g. an object's DDL. */
  openScript: (connectionId: string, script: { title: string; sql: string }) => void;
  closeTab: (connectionId: string, tabId: string) => void;
  selectTab: (connectionId: string, tabId: string) => Promise<void>;

  setSql: (connectionId: string, tabId: string, sql: string) => void;
  /** Show a failure that did not come from running this tab's own statement. */
  setTabError: (connectionId: string, tabId: string, message: string) => void;
  setActiveStatement: (connectionId: string, tabId: string, index: number) => void;
  run: (connectionId: string, tabId: string, sqlOverride?: string) => Promise<void>;

  loadSession: (connectionId: string) => Promise<void>;
  useDatabase: (connectionId: string, database: string) => Promise<void>;
  loadCompletionFor: (connectionId: string) => Promise<void>;

  applyEdit: (
    connectionId: string,
    tabId: string,
    rowIndex: number,
    columnIndex: number,
    next: Value,
  ) => Promise<void>;
  undo: (connectionId: string, tabId: string) => Promise<void>;
  redo: (connectionId: string, tabId: string) => Promise<void>;
  reset: (connectionId: string) => void;
}

/** Read-modify-write one tab without disturbing the others. */
function patchTab(
  tabs: Record<string, Tab[]>,
  connectionId: string,
  tabId: string,
  changes: Partial<Tab>,
): Record<string, Tab[]> {
  const list = tabs[connectionId] ?? [];
  return {
    ...tabs,
    [connectionId]: list.map((t) => (t.id === tabId ? { ...t, ...changes } : t)),
  };
}

/** The result set currently displayed, if the active statement returned rows. */
function activeRows(tab: Tab): (StatementResult & { type: "rows" }) | null {
  const statement = tab.outcome?.statements[tab.activeStatement];
  return statement?.type === "rows" ? statement : null;
}

export const useWorkspace = create<WorkspaceState>((set, get) => ({
  tabs: {},
  active: {},
  completion: {},
  database: {},
  switching: {},

  tabsFor: (id) => tabsOf(get(), id),
  activeTab: (id) => {
    const list = tabsOf(get(), id);
    return list.find((t) => t.id === get().active[id]) ?? list[0] ?? null;
  },

  openQuery: (id) => {
    const list = tabsOf(get(), id);
    // Numbered by how many query tabs exist rather than by total tabs, so the
    // names stay predictable when table tabs are opened and closed between them.
    const n = list.filter((t) => t.kind === "query").length + 1;
    const tab = blankTab({
      kind: "query",
      title: `Query ${n}`,
      database: get().database[id] ?? null,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
  },

  openTable: (id, object) => {
    const list = tabsOf(get(), id);
    const database = get().database[id] ?? null;

    // Opening the same table twice focuses the tab that is already there, the
    // way a browser does. Two identical tabs would just be two ways to lose
    // track of which one has your unsaved filter on it.
    const existing = list.find(
      (t) => t.kind === "table" && t.title === object.title && t.database === database,
    );
    if (existing) {
      set((s) => ({ active: { ...s.active, [id]: existing.id } }));
      return;
    }

    const tab = blankTab({
      kind: "table",
      title: object.title,
      database,
      schema: object.schema,
      // The driver supplied the quoted, qualified name, so this is correct for
      // the engine's own quoting rules without the UI knowing them.
      sql: `SELECT * FROM ${object.qualified}`,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
    void get().run(id, tab.id);
  },

  openScript: (id, script) => {
    const tab = blankTab({
      kind: "query",
      title: script.title,
      database: get().database[id] ?? null,
      sql: script.sql,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...tabsOf(s, id), tab] },
      active: { ...s.active, [id]: tab.id },
    }));
    // Deliberately not run. This is a CREATE statement for something that
    // already exists; running it on open would fail at best.
  },

  closeTab: (id, tabId) => {
    const list = tabsOf(get(), id);
    const index = list.findIndex((t) => t.id === tabId);
    const remaining = list.filter((t) => t.id !== tabId);

    // Focus moves to the neighbour on the left, which is where the eye already
    // is after closing something.
    const nextActive =
      get().active[id] === tabId
        ? (remaining[Math.max(0, index - 1)]?.id ?? "")
        : (get().active[id] ?? "");

    set((s) => ({
      tabs: { ...s.tabs, [id]: remaining },
      active: { ...s.active, [id]: nextActive },
    }));
  },

  selectTab: async (id, tabId) => {
    set((s) => ({ active: { ...s.active, [id]: tabId } }));

    // A tab belongs to the database it was opened in. Bringing it forward has
    // to bring its database with it, or its SQL would run somewhere else.
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (tab?.database && tab.database !== get().database[id]) {
      await get().useDatabase(id, tab.database);
    }
  },

  setSql: (id, tabId, sql) => set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { sql }) })),

  setTabError: (id, tabId, message) =>
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { error: { message } }) })),

  setActiveStatement: (id, tabId, index) =>
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { activeStatement: index }) })),

  reset: (id) =>
    set((s) => ({
      tabs: { ...s.tabs, [id]: [] },
      active: { ...s.active, [id]: "" },
      database: { ...s.database, [id]: null },
    })),

  loadSession: async (id) => {
    try {
      const info = await ipc.sessionInfo(id);
      set((s) => ({ database: { ...s.database, [id]: info.database ?? null } }));
    } catch {
      // A session that cannot report its database is still usable; the tree
      // simply will not mark one.
    }

    // Every connection starts with somewhere to type.
    if (tabsOf(get(), id).length === 0) get().openQuery(id);
  },

  useDatabase: async (id, database) => {
    if (get().database[id] === database) return;
    set((s) => ({ switching: { ...s.switching, [id]: true } }));
    try {
      const now = await ipc.useDatabase(id, database);
      set((s) => ({ database: { ...s.database, [id]: now } }));
      // Autocomplete describes the database that was open, so it is refetched
      // rather than left describing the previous one.
      void get().loadCompletionFor(id);
    } catch (e) {
      const err = e as IpcError;
      const tab = get().activeTab(id);
      if (tab) {
        set((s) => ({
          tabs: patchTab(s.tabs, id, tab.id, { error: { message: err.message } }),
        }));
      }
    } finally {
      set((s) => ({ switching: { ...s.switching, [id]: false } }));
    }
  },

  run: async (id, tabId, sqlOverride) => {
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (!tab) return;
    const sql = sqlOverride ?? tab.sql;
    if (!sql.trim()) return;
    // One query per tab at a time. The session is locked for the duration
    // anyway, so a second submission would only queue behind the first.
    if (tab.running) return;

    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { running: true, error: null }) }));
    try {
      const outcome = await ipc.execute({ connection_id: id, sql });
      set((s) => ({
        tabs: patchTab(s.tabs, id, tabId, {
          outcome,
          running: false,
          activeStatement: 0,
          // A new result invalidates the edit history: the undo statements
          // reference rows that may no longer be on screen.
          undo: [],
          redo: [],
        }),
      }));
    } catch (e) {
      const err = e as IpcError;
      set((s) => ({
        tabs: patchTab(s.tabs, id, tabId, {
          running: false,
          error: { message: err.message, position: err.position, code: err.code },
          // Keep the previous outcome visible. Blanking the grid on a typo
          // loses results the user may still be reading.
        }),
      }));
    }
  },

  applyEdit: async (id, tabId, rowIndex, columnIndex, next) => {
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (!tab) return;
    const rows = activeRows(tab);
    if (!rows || !rows.editable) return;

    const column = rows.columns[columnIndex];
    const row = rows.rows[rowIndex];
    if (!column || !row) return;

    const before = row[columnIndex];
    if (!before) return;

    const source = column.source;
    if (!source) return;

    // The WHERE clause is built from the row's *original* key values, so a
    // concurrent change elsewhere makes the update match zero rows and fail
    // loudly rather than overwriting someone else's edit.
    const key = rows.key_columns.map((name) => {
      const index = rows.columns.findIndex((c) => c.name === name);
      return [name, row[index] ?? { kind: "null" as const }] as [string, Value];
    });

    const edit: RowEdit = {
      schema: source.schema,
      table: source.table,
      changes: [[source.column, next]],
      key,
    };

    await ipc.applyEdit(id, edit);

    // Only mutate the local copy once the database has confirmed the write.
    const updatedRow = [...row];
    updatedRow[columnIndex] = next;
    const updatedRows = [...rows.rows];
    updatedRows[rowIndex] = updatedRow;

    const statements = [...(tab.outcome?.statements ?? [])];
    statements[tab.activeStatement] = { ...rows, rows: updatedRows };

    const applied: AppliedEdit = {
      rowIndex,
      columnIndex,
      before,
      after: next,
      inverse: { ...edit, changes: [[source.column, before]] },
    };

    set((s) => ({
      tabs: patchTab(s.tabs, id, tabId, {
        outcome: { ...tab.outcome!, statements },
        undo: [...tab.undo, applied],
        // Any new edit invalidates the redo branch, as in every text editor.
        redo: [],
      }),
    }));
  },

  undo: async (id, tabId) => {
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (!tab) return;
    const last = tab.undo[tab.undo.length - 1];
    if (!last) return;

    await ipc.applyEdit(id, last.inverse);

    const rows = activeRows(tab);
    if (!rows) return;
    const row = rows.rows[last.rowIndex];
    if (!row) return;

    const updatedRow = [...row];
    updatedRow[last.columnIndex] = last.before;
    const updatedRows = [...rows.rows];
    updatedRows[last.rowIndex] = updatedRow;
    const statements = [...(tab.outcome?.statements ?? [])];
    statements[tab.activeStatement] = { ...rows, rows: updatedRows };

    set((s) => ({
      tabs: patchTab(s.tabs, id, tabId, {
        outcome: { ...tab.outcome!, statements },
        undo: tab.undo.slice(0, -1),
        redo: [...tab.redo, last],
      }),
    }));
  },

  redo: async (id, tabId) => {
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (!tab) return;
    const next = tab.redo[tab.redo.length - 1];
    if (!next) return;

    // Re-apply by inverting the inverse.
    const forward: RowEdit = {
      ...next.inverse,
      changes: [[next.inverse.changes[0]![0], next.after]],
    };
    await ipc.applyEdit(id, forward);

    const rows = activeRows(tab);
    if (!rows) return;
    const row = rows.rows[next.rowIndex];
    if (!row) return;

    const updatedRow = [...row];
    updatedRow[next.columnIndex] = next.after;
    const updatedRows = [...rows.rows];
    updatedRows[next.rowIndex] = updatedRow;
    const statements = [...(tab.outcome?.statements ?? [])];
    statements[tab.activeStatement] = { ...rows, rows: updatedRows };

    set((s) => ({
      tabs: patchTab(s.tabs, id, tabId, {
        outcome: { ...tab.outcome!, statements },
        undo: [...tab.undo, next],
        redo: tab.redo.slice(0, -1),
      }),
    }));
  },

  loadCompletionFor: async (id) => {
    try {
      const completion = await ipc.completionScope(id);
      set((s) => ({ completion: { ...s.completion, [id]: completion } }));
    } catch {
      // Autocomplete is an enhancement; failing to load it must not surface as
      // an error banner over the user's results.
    }
  },
}));
