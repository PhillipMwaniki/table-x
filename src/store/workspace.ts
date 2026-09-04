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
import { useSettings } from "@/store/settings";
import { noteLinkFailure, useConnections } from "@/store/connections";
import { load as loadStore } from "@tauri-apps/plugin-store";
import type { Store } from "@tauri-apps/plugin-store";
import { parseSaved, shouldAutoRun, toSaved } from "@/lib/session";
import { changesCatalog } from "@/lib/statements";
import type {
  CompletionScope,
  Design,
  DiffReport,
  ErrorCategory,
  Notebook,
  NotebookCell,
  Plan,
  QueryOutcome,
  QueryProgress,
  RowEdit,
  StatementResult,
  TransactionState,
  Value,
} from "@/lib/types";

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
  /**
   * What kind of failure this was.
   *
   * Carried so the banner can offer the recovery that fits. A dropped link and
   * a typo in a WHERE clause both arrive here as red text, but only one of them
   * is fixed by rebuilding the connection rather than by editing the statement.
   */
  category?: ErrorCategory | undefined;
}

/**
 * A table tab shows one object's rows with no editor; a query tab is an editor
 * over a result. They share everything else, so they share a shape.
 *
 * An activity tab has neither — it is a live view of the server rather than of
 * a result — but it is still a tab, because watching the server while you work
 * is the whole point and a modal would put it in front of the work instead.
 */
export type TabKind =
  "query" | "table" | "activity" | "diagram" | "diff" | "privileges" | "notebook" | "design";

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
  /** A one-off success message — an export that finished, say. */
  notice?: string | undefined;
  /** A plan being shown in place of this tab's results. */
  plan?: Plan | null;
  /** A schema comparison, for a diff tab. */
  diff?: DiffReport | null;
  /**
   * Which half of a table tab is showing.
   *
   * On the tab rather than in the view, so switching to another tab and back
   * returns to the side you were on — the two are views of one thing, and
   * losing your place on every switch would make the toggle a chore.
   */
  view?: "data" | "structure";
  /** The cells of a notebook tab. Results are held in the view, not here. */
  cells?: NotebookCell[];
  /** The stored notebook this tab is editing, once it has been saved. */
  notebookId?: string | undefined;
  /**
   * The schema design this tab is showing.
   *
   * The design itself is held here rather than fetched by the view on every
   * render: a drag moves a table sixty times a second, and a round trip per
   * frame to find out where the table already is would make the canvas fight
   * the pointer.
   */
  design?: Design | undefined;
  running: boolean;
  /**
   * How far a multi-statement run has got, while `running`.
   *
   * Null for a single statement, which has nothing to count, and once the run
   * is over. The backend reports it; the tab is where the UI reads it from.
   */
  progress?: { done: number; total: number } | null;
  /** Index of the statement whose results are shown. */
  activeStatement: number;
  /**
   * Rows skipped by the last fetch — the page this tab is showing.
   *
   * Kept on the tab rather than in the grid because it is a property of the
   * fetch, not of the display: changing it means going back to the server,
   * and the grid's own filtering and sorting only ever reach the rows already
   * here.
   */
  offset: number;
  /** Rows the last fetch asked for, so "is there a next page" has an answer. */
  limit: number;
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

const WORKSPACE_FILE = "workspace.json";

/**
 * How long to wait before writing tabs to disk, in ms.
 *
 * Every keystroke in the editor changes a tab. Writing on each one would put a
 * file write in the typing path for no benefit — what is being protected
 * against is a crash, and half a second of typing is what such a crash costs.
 */
const PERSIST_DELAY = 500;

let handle: Store | null = null;
/** One pending write per connection — two connections must not cancel each other. */
const persistTimers = new Map<string, ReturnType<typeof setTimeout>>();

/**
 * Write one connection's tabs, a beat after they stop changing.
 *
 * Failures are logged and swallowed. A workspace that could not be saved is
 * annoying at the next launch; an error dialog in the middle of typing is
 * annoying now, and about something the user cannot act on.
 */
function persist(connectionId: string, tabs: Tab[], activeId: string | undefined) {
  // An empty list is not saved. Disconnecting clears a connection's tabs, and
  // writing that through would erase the workspace someone expects to find
  // when they connect again — the cost of the other choice is that closing
  // every tab by hand does not clear the file, which is the harmless failure.
  if (tabs.length === 0) return;

  const pending = persistTimers.get(connectionId);
  if (pending) clearTimeout(pending);

  persistTimers.set(
    connectionId,
    setTimeout(async () => {
      persistTimers.delete(connectionId);
      try {
        handle ??= await loadStore(WORKSPACE_FILE);
        await handle.set(connectionId, toSaved(tabs, activeId));
        await handle.save();
      } catch (e) {
        // A workspace that could not be saved is annoying at the next launch;
        // an error dialog mid-typing is annoying now, about something the user
        // cannot act on.
        console.warn("could not save the workspace", e);
      }
    }, PERSIST_DELAY),
  );
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
    offset: 0,
    limit: 0,
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
  /**
   * Transaction state per connection.
   *
   * Refreshed after anything that could have changed it rather than polled: the
   * backend already knows, and a badge that lags behind by a poll interval is a
   * badge that is wrong at exactly the moment somebody reads it.
   */
  transaction: Record<string, TransactionState | undefined>;

  tabsFor: (connectionId: string) => Tab[];
  activeTab: (connectionId: string) => Tab | null;

  openQuery: (connectionId: string) => void;
  openTable: (
    connectionId: string,
    object: { title: string; qualified: string; schema?: string | undefined },
  ) => void;
  /** A query tab that opens with content already in it, e.g. an object's DDL. */
  openScript: (connectionId: string, script: { title: string; sql: string }) => void;
  openActivity: (connectionId: string) => void;
  openPrivileges: (connectionId: string) => void;
  openDiagram: (connectionId: string, schema: string | null) => void;
  openDiff: (connectionId: string, title: string, report: DiffReport) => void;
  openNotebook: (connectionId: string, notebook?: Notebook) => void;
  /** Show a design on a canvas, focusing the tab that already has it. */
  openDesign: (connectionId: string, design: Design) => void;
  /** Follow a design as it is edited — a rename retitles its tab. */
  setDesign: (connectionId: string, tabId: string, design: Design) => void;
  setCells: (connectionId: string, tabId: string, cells: NotebookCell[]) => void;
  setTabView: (connectionId: string, tabId: string, view: "data" | "structure") => void;
  renameNotebookTab: (
    connectionId: string,
    tabId: string,
    notebookId: string,
    name: string,
  ) => void;
  closeTab: (connectionId: string, tabId: string) => void;
  selectTab: (connectionId: string, tabId: string) => Promise<void>;

  setSql: (connectionId: string, tabId: string, sql: string) => void;
  /** Show a failure that did not come from running this tab's own statement. */
  setTabError: (connectionId: string, tabId: string, message: string) => void;
  /** Report something that worked, in the same place failures are reported. */
  setTabNotice: (connectionId: string, tabId: string, message: string) => void;
  /**
   * Bumped when a statement that changes the object tree succeeds.
   *
   * A counter rather than the tree itself: the store has no business knowing
   * how the tree is built, and the browser already knows how to rebuild itself.
   * This only says that what it is showing is now out of date.
   */
  schemaVersion: Record<string, number>;
  /**
   * Say that the catalogue has changed, so the tree rebuilds.
   *
   * Bumped automatically after a statement that changes it; this is for the
   * changes that do not go through the editor -- a table or a database created
   * from a form.
   */
  bumpSchema: (connectionId: string) => void;
  setActiveStatement: (connectionId: string, tabId: string, index: number) => void;
  run: (connectionId: string, tabId: string, sqlOverride?: string) => Promise<void>;
  /** Subscribe to the backend's per-statement progress. Safe to call more than once. */
  watchProgress: () => Promise<void>;
  /** Record a progress report against the tab it names. */
  noteProgress: (progress: QueryProgress) => void;
  /** Stop whatever this connection is running. */
  cancelQuery: (connectionId: string) => Promise<void>;
  /** Re-read whether a transaction is open. */
  refreshTransaction: (connectionId: string) => Promise<void>;
  /** Open a transaction, so the next statements can be taken back. */
  beginTransaction: (connectionId: string) => Promise<void>;
  /** Settle the open transaction, one way or the other. */
  endTransaction: (connectionId: string, how: "commit" | "rollback") => Promise<void>;
  /** Re-run this tab's statement at a different offset. */
  goToPage: (connectionId: string, tabId: string, offset: number) => Promise<void>;

  loadSession: (connectionId: string) => Promise<void>;
  /**
   * Rebuild a link that has broken, and put the workspace back on its feet.
   *
   * The tabs, their statements, and their last results stay exactly as they
   * are — losing an afternoon's work to a dropped Wi-Fi connection is the thing
   * this exists to prevent. Everything that described the *session* rather than
   * the work is asked again, because it belongs to a socket that no longer
   * exists.
   */
  reconnect: (connectionId: string) => Promise<void>;
  /** Bring back the tabs this connection had when the app last closed. */
  restore: (connectionId: string) => Promise<void>;
  /**
   * Point the session at another database.
   *
   * Not `useDatabase`: a function named `use*` in a React codebase reads as a
   * hook to both people and the rules-of-hooks lint, and this is neither.
   */
  switchDatabase: (connectionId: string, database: string) => Promise<void>;
  loadCompletionFor: (connectionId: string) => Promise<void>;

  applyEdit: (
    connectionId: string,
    tabId: string,
    rowIndex: number,
    columnIndex: number,
    next: Value,
  ) => Promise<void>;
  explain: (connectionId: string, tabId: string, analyze: boolean) => Promise<void>;
  clearPlan: (connectionId: string, tabId: string) => void;
  undo: (connectionId: string, tabId: string) => Promise<void>;
  redo: (connectionId: string, tabId: string) => Promise<void>;
  reset: (connectionId: string) => void;
}

/**
 * Turn a failed call into what the banner will show, noting a broken link.
 *
 * A connection-category failure means the socket is gone rather than that the
 * statement was wrong, and every later call on that session fails the same way
 * until the link is rebuilt. Recording it here, where every failure already
 * passes, is what puts a Reconnect button in front of the user instead of
 * leaving them to work out which of these red messages is worth retrying.
 */
function failure(connectionId: string, e: unknown): QueryError {
  const err = e as IpcError;
  noteLinkFailure(connectionId, e);
  return {
    message: err.message,
    position: err.position,
    code: err.code,
    category: err.category,
  };
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

/** Whether the process-wide progress subscription has been made. */
let watchingProgress = false;

export const useWorkspace = create<WorkspaceState>((set, get) => ({
  tabs: {},
  schemaVersion: {},
  active: {},
  completion: {},
  database: {},
  switching: {},
  transaction: {},

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

  openNotebook: (id, notebook) => {
    const list = tabsOf(get(), id);
    // Reopening a saved notebook focuses the tab already showing it rather than
    // opening a second copy that would then disagree with the first on save.
    const existing = notebook && list.find((t) => t.notebookId === notebook.id);
    if (existing) {
      set((s) => ({ active: { ...s.active, [id]: existing.id } }));
      return;
    }

    const n = list.filter((t) => t.kind === "notebook").length + 1;
    const tab = blankTab({
      kind: "notebook",
      title: notebook?.name ?? `Notebook ${n}`,
      database: get().database[id] ?? null,
      cells: notebook?.cells ?? [],
      notebookId: notebook?.id,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
  },

  openDesign: (id, design) => {
    const list = tabsOf(get(), id);
    // Reopening a design focuses the tab already showing it rather than opening
    // a second copy, which would then save over the first.
    const existing = list.find((t) => t.design?.id === design.id);
    if (existing) {
      set((s) => ({
        active: { ...s.active, [id]: existing.id },
        // The list's copy is newer than whatever the tab was holding.
        tabs: patchTab(s.tabs, id, existing.id, { design, title: design.name }),
      }));
      return;
    }

    const tab = blankTab({ kind: "design", title: design.name, design });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
  },

  setDesign: (id, tabId, design) =>
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { design, title: design.name }) })),

  setCells: (id, tabId, cells) => set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { cells }) })),

  setTabView: (id, tabId, view) => set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { view }) })),

  renameNotebookTab: (id, tabId, notebookId, name) =>
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { notebookId, title: name }) })),

  openDiff: (id, title, report) => {
    const tab = blankTab({
      kind: "diff",
      title,
      database: get().database[id] ?? null,
      diff: report,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...tabsOf(s, id), tab] },
      active: { ...s.active, [id]: tab.id },
    }));
  },

  openDiagram: (id, schema) => {
    const list = tabsOf(get(), id);
    const title = schema ? `Diagram — ${schema}` : "Diagram";
    // One per schema. Two diagrams of the same schema would be two copies of
    // the same picture, since the layout is deterministic.
    const existing = list.find((t) => t.kind === "diagram" && t.title === title);
    if (existing) {
      set((s) => ({ active: { ...s.active, [id]: existing.id } }));
      return;
    }

    const tab = blankTab({
      kind: "diagram",
      title,
      database: get().database[id] ?? null,
      schema: schema ?? undefined,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
  },

  openPrivileges: (id) => {
    const list = tabsOf(get(), id);
    const existing = list.find((t) => t.kind === "privileges");
    if (existing) {
      set((s) => ({ active: { ...s.active, [id]: existing.id } }));
      return;
    }
    const tab = blankTab({
      kind: "privileges",
      title: "Privileges",
      database: get().database[id] ?? null,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
  },

  openActivity: (id) => {
    const list = tabsOf(get(), id);
    // One per connection. A second live view of the same server would refresh
    // on its own timer and show a different answer to the same question.
    const existing = list.find((t) => t.kind === "activity");
    if (existing) {
      set((s) => ({ active: { ...s.active, [id]: existing.id } }));
      return;
    }

    const tab = blankTab({
      kind: "activity",
      title: "Server activity",
      database: get().database[id] ?? null,
    });
    set((s) => ({
      tabs: { ...s.tabs, [id]: [...list, tab] },
      active: { ...s.active, [id]: tab.id },
    }));
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
      await get().switchDatabase(id, tab.database);
    }
  },

  setSql: (id, tabId, sql) =>
    // Back to the first page: an offset counts rows of the statement that
    // produced it, and editing the statement makes that count meaningless.
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { sql, offset: 0 }) })),

  setTabError: (id, tabId, message) =>
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { error: { message } }) })),

  setTabNotice: (id, tabId, message) =>
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { notice: message }) })),

  bumpSchema: (id) =>
    set((s) => ({
      schemaVersion: { ...s.schemaVersion, [id]: (s.schemaVersion[id] ?? 0) + 1 },
    })),

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

    // Whether the engine has transactions at all decides whether the controls
    // are drawn, so it is known before the first tab appears.
    await get().refreshTransaction(id);

    // Only for a connection with nothing open. Reconnecting mid-session must
    // not throw away the tabs already in front of somebody.
    if (tabsOf(get(), id).length === 0) await get().restore(id);

    // Every connection starts with somewhere to type.
    if (tabsOf(get(), id).length === 0) get().openQuery(id);
  },

  reconnect: async (id) => {
    // The connection store owns the link itself, including whether this one is
    // still marked broken afterwards. Nothing below is true if it failed.
    if (!(await useConnections.getState().reconnect(id))) return;

    // Errors about the old link describe something that no longer exists. The
    // statements that produced them are left untouched: they are still what the
    // user typed, and still what they will want to run.
    set((s) => ({
      tabs: {
        ...s.tabs,
        [id]: tabsOf(s, id).map((t) =>
          t.error?.category === "connection" ? { ...t, error: null } : t,
        ),
      },
    }));

    // Whatever described the session — which database it is on, whether a
    // transaction is open, what names autocomplete knows — belonged to the
    // socket that died and is asked again rather than assumed.
    await get().loadSession(id);
    await get().loadCompletionFor(id);

    // The object tree is the other thing that describes the server rather than
    // the work. It caches what it fetched and only refetches when this counter
    // moves, so without the bump it keeps whatever it had when the link went —
    // usually the connection error it failed on, with no way to retry it. The
    // catalogue may genuinely have changed while the link was down, too.
    get().bumpSchema(id);

    const tab = get().activeTab(id);
    // Deliberately not re-running the statement that failed. A link that dropped
    // mid-statement leaves no way to tell whether the server applied it, and
    // silently sending an INSERT a second time is a far worse outcome than
    // asking somebody to press Run.
    if (tab) get().setTabNotice(id, tab.id, "Reconnected. Run the statement again when ready.");
  },

  restore: async (id) => {
    let saved;
    try {
      handle ??= await loadStore(WORKSPACE_FILE);
      saved = parseSaved(await handle.get(id));
    } catch (e) {
      // A workspace that cannot be read is a workspace that starts empty,
      // which is recoverable. Failing the connection over it would not be.
      console.warn("could not read the saved workspace", e);
      return;
    }
    if (!saved) return;

    // Fresh ids rather than the saved ones: the counter that hands them out
    // starts at zero each launch, so reusing them would collide with the very
    // next tab opened.
    const tabs = saved.tabs.map((entry) =>
      blankTab({
        kind: entry.kind,
        title: entry.title,
        database: entry.database,
        schema: entry.schema,
        sql: entry.sql,
        ...(entry.view ? { view: entry.view } : {}),
        ...(entry.cells ? { cells: entry.cells } : {}),
        notebookId: entry.notebookId,
      }),
    );

    const active = tabs[saved.active] ?? tabs[0];
    set((s) => ({
      tabs: { ...s.tabs, [id]: tabs },
      active: { ...s.active, [id]: active?.id ?? "" },
    }));

    // Only the one being looked at, and only if it is safe to run unbidden —
    // see `shouldAutoRun`. Loading every restored tab would fire a query per
    // tab at connect time.
    const entry = saved.tabs[saved.active];
    if (active && entry && shouldAutoRun(entry)) {
      void get().run(id, active.id);
    }
  },

  switchDatabase: async (id, database) => {
    if (get().database[id] === database) return;
    set((s) => ({ switching: { ...s.switching, [id]: true } }));
    try {
      const now = await ipc.useDatabase(id, database);
      set((s) => ({ database: { ...s.database, [id]: now } }));
      // Autocomplete describes the database that was open, so it is refetched
      // rather than left describing the previous one.
      void get().loadCompletionFor(id);
    } catch (e) {
      const error = failure(id, e);
      const tab = get().activeTab(id);
      if (tab) set((s) => ({ tabs: patchTab(s.tabs, id, tab.id, { error }) }));
    } finally {
      set((s) => ({ switching: { ...s.switching, [id]: false } }));
    }
  },

  explain: async (id, tabId, analyze) => {
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (!tab) return;
    const sql = tab.sql.trim();
    if (!sql) return;

    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { running: true, error: null }) }));
    try {
      const plan = await ipc.explain(id, sql, analyze);
      set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { plan, running: false }) }));
    } catch (e) {
      set((s) => ({
        tabs: patchTab(s.tabs, id, tabId, { running: false, error: failure(id, e) }),
      }));
    }
  },

  clearPlan: (id, tabId) => {
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { plan: null }) }));
  },

  refreshTransaction: async (id) => {
    try {
      const state = await ipc.transactionState(id);
      set((s) => ({ transaction: { ...s.transaction, [id]: state } }));
    } catch {
      // A connection that has gone away has no transaction state to show, and
      // failing the query that triggered this refresh would report the wrong
      // problem.
      set((s) => ({ transaction: { ...s.transaction, [id]: undefined } }));
    }
  },

  beginTransaction: async (id) => {
    await ipc.beginTransaction(id);
    await get().refreshTransaction(id);
  },

  endTransaction: async (id, how) => {
    try {
      if (how === "commit") await ipc.commitTransaction(id);
      else await ipc.rollbackTransaction(id);
    } finally {
      // Even on failure. A commit that was refused usually means the
      // transaction is already gone, and leaving the badge lit would strand the
      // user with buttons that cannot succeed.
      await get().refreshTransaction(id);
    }
  },

  cancelQuery: async (id) => {
    // Nothing is patched here. The statement's own rejection is what ends the
    // running state, and pre-emptively clearing it would leave the tab looking
    // idle while the query was still being torn down.
    await ipc.cancelQuery(id);
  },

  goToPage: async (id, tabId, offset) => {
    // Written before the run so the fetch reads it, and clamped because a
    // "previous" from the first page is a request for row minus one thousand.
    set((s) => ({ tabs: patchTab(s.tabs, id, tabId, { offset: Math.max(0, offset) }) }));
    await get().run(id, tabId);
  },

  watchProgress: async () => {
    if (watchingProgress) return;
    watchingProgress = true;
    // Imported here rather than at the top so the store loads without a Tauri
    // shell, which is where its tests run.
    const { listen } = await import("@tauri-apps/api/event");
    await listen<QueryProgress>("query-progress", (event) => {
      get().noteProgress(event.payload);
    });
  },

  noteProgress: (progress) => {
    // The id is a tab id, and tab ids are unique across connections, so the
    // report carries no connection id and the tab is found by looking.
    set((s) => {
      for (const [connectionId, list] of Object.entries(s.tabs)) {
        const tab = list.find((t) => t.id === progress.id);
        // A report that arrives after the run finished — the events are async
        // and the reply can overtake the last of them — must not put a bar
        // back on a tab that has already shown its result.
        if (!tab || !tab.running) continue;
        return {
          tabs: patchTab(s.tabs, connectionId, tab.id, {
            progress: { done: progress.done, total: progress.total },
          }),
        };
      }
      return {};
    });
  },

  run: async (id, tabId, sqlOverride) => {
    const tab = tabsOf(get(), id).find((t) => t.id === tabId);
    if (!tab) return;
    const sql = sqlOverride ?? tab.sql;
    if (!sql.trim()) return;
    // One query per tab at a time. The session is locked for the duration
    // anyway, so a second submission would only queue behind the first.
    if (tab.running) return;

    set((s) => ({
      tabs: patchTab(s.tabs, id, tabId, {
        running: true,
        progress: null,
        error: null,
        notice: undefined,
        plan: null,
      }),
    }));
    try {
      // Read at call time rather than closed over: a page change writes the
      // offset immediately before calling this.
      const current = tabsOf(get(), id).find((t) => t.id === tabId);
      const offset = sqlOverride ? 0 : (current?.offset ?? 0);
      // Read off the settings store rather than passed in: page size is a
      // preference, and threading it through every caller of run() would make
      // every one of them responsible for a decision none of them makes.
      const limit = useSettings.getState().pageSize;

      const outcome = await ipc.execute({
        connection_id: id,
        sql,
        max_rows: limit,
        offset,
        progress_id: tabId,
      });
      // Only after it succeeded, and only for statements that could have moved
      // something: a CREATE DATABASE that failed leaves the tree correct, and
      // refetching after every SELECT would query the catalogue constantly.
      if (changesCatalog(sql)) {
        set((s) => ({
          schemaVersion: { ...s.schemaVersion, [id]: (s.schemaVersion[id] ?? 0) + 1 },
        }));
      }
      set((s) => ({
        tabs: patchTab(s.tabs, id, tabId, {
          outcome,
          running: false,
          progress: null,
          activeStatement: 0,
          offset,
          limit,
          // A new result invalidates the edit history: the undo statements
          // reference rows that may no longer be on screen.
          undo: [],
          redo: [],
        }),
      }));
      // A statement can open or close a transaction without going anywhere near
      // the buttons. The backend already knows which; asking it is cheaper than
      // keeping a second SQL scanner here that could disagree.
      void get().refreshTransaction(id);
    } catch (e) {
      const err = e as IpcError;
      // Cancelling is something the user asked for, so it is reported the way
      // a finished job is rather than as a failure. A red alert for the button
      // you just pressed working is the wrong signal.
      const cancelled = err.category === "cancelled";
      set((s) => ({
        tabs: patchTab(s.tabs, id, tabId, {
          running: false,
          progress: null,
          error: cancelled ? null : failure(id, e),
          ...(cancelled ? { notice: "Cancelled." } : {}),
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

// Saving is wired here rather than into each action that changes a tab. There
// are a dozen of those, and one of them forgetting would be a bug nobody
// notices until the crash it was supposed to protect against.
useWorkspace.subscribe((state, previous) => {
  for (const id of Object.keys(state.tabs)) {
    const changed =
      state.tabs[id] !== previous.tabs[id] || state.active[id] !== previous.active[id];
    if (changed) persist(id, state.tabs[id] ?? [], state.active[id]);
  }
});
