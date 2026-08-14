/**
 * Per-connection workspace state: editor contents, the last result, and the
 * undo stack for inline edits.
 *
 * Keyed by connection id so switching connections in the sidebar preserves each
 * one's query and results rather than resetting the pane.
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

interface Pane {
  sql: string;
  outcome: QueryOutcome | null;
  error: QueryError | null;
  running: boolean;
  /** Index of the statement whose results are shown. */
  activeStatement: number;
  completion: CompletionScope | null;
  undo: AppliedEdit[];
  redo: AppliedEdit[];
}

function blankPane(): Pane {
  return {
    sql: "",
    outcome: null,
    error: null,
    running: false,
    activeStatement: 0,
    completion: null,
    undo: [],
    redo: [],
  };
}

interface WorkspaceState {
  panes: Record<string, Pane>;

  pane: (connectionId: string) => Pane;
  setSql: (connectionId: string, sql: string) => void;
  setActiveStatement: (connectionId: string, index: number) => void;
  run: (connectionId: string, sqlOverride?: string) => Promise<void>;
  loadCompletion: (connectionId: string) => Promise<void>;
  applyEdit: (
    connectionId: string,
    rowIndex: number,
    columnIndex: number,
    next: Value,
  ) => Promise<void>;
  undo: (connectionId: string) => Promise<void>;
  redo: (connectionId: string) => Promise<void>;
  reset: (connectionId: string) => void;
}

/** Read-modify-write one pane without disturbing the others. */
function patch(
  panes: Record<string, Pane>,
  id: string,
  changes: Partial<Pane>,
): Record<string, Pane> {
  return { ...panes, [id]: { ...(panes[id] ?? blankPane()), ...changes } };
}

/** The result set currently displayed, if the active statement returned rows. */
function activeRows(pane: Pane): (StatementResult & { type: "rows" }) | null {
  const statement = pane.outcome?.statements[pane.activeStatement];
  return statement?.type === "rows" ? statement : null;
}

export const useWorkspace = create<WorkspaceState>((set, get) => ({
  panes: {},

  pane: (id) => get().panes[id] ?? blankPane(),

  setSql: (id, sql) => set((s) => ({ panes: patch(s.panes, id, { sql }) })),

  setActiveStatement: (id, index) =>
    set((s) => ({ panes: patch(s.panes, id, { activeStatement: index }) })),

  reset: (id) => set((s) => ({ panes: patch(s.panes, id, blankPane()) })),

  run: async (id, sqlOverride) => {
    const pane = get().pane(id);
    const sql = sqlOverride ?? pane.sql;
    if (!sql.trim()) return;
    // One query per pane at a time. The session is locked for the duration
    // anyway, so a second submission would only queue behind the first - and
    // an impatient double-click on a table should not run it twice.
    if (pane.running) return;

    set((s) => ({ panes: patch(s.panes, id, { running: true, error: null }) }));
    try {
      const outcome = await ipc.execute({ connection_id: id, sql });
      set((s) => ({
        panes: patch(s.panes, id, {
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
        panes: patch(s.panes, id, {
          running: false,
          error: { message: err.message, position: err.position, code: err.code },
          // Keep the previous outcome visible. Blanking the grid on a typo
          // loses results the user may still be reading.
        }),
      }));
    }
  },

  loadCompletion: async (id) => {
    try {
      const completion = await ipc.completionScope(id);
      set((s) => ({ panes: patch(s.panes, id, { completion }) }));
    } catch {
      // Autocomplete is an enhancement; failing to load it must not surface as
      // an error banner over the user's results.
    }
  },

  applyEdit: async (id, rowIndex, columnIndex, next) => {
    const pane = get().pane(id);
    const rows = activeRows(pane);
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

    const statements = [...(pane.outcome?.statements ?? [])];
    statements[pane.activeStatement] = { ...rows, rows: updatedRows };

    const applied: AppliedEdit = {
      rowIndex,
      columnIndex,
      before,
      after: next,
      inverse: { ...edit, changes: [[source.column, before]] },
    };

    set((s) => ({
      panes: patch(s.panes, id, {
        outcome: { ...pane.outcome!, statements },
        undo: [...pane.undo, applied],
        // Any new edit invalidates the redo branch, as in every text editor.
        redo: [],
      }),
    }));
  },

  undo: async (id) => {
    const pane = get().pane(id);
    const last = pane.undo[pane.undo.length - 1];
    if (!last) return;

    await ipc.applyEdit(id, last.inverse);

    const rows = activeRows(pane);
    if (!rows) return;
    const row = rows.rows[last.rowIndex];
    if (!row) return;

    const updatedRow = [...row];
    updatedRow[last.columnIndex] = last.before;
    const updatedRows = [...rows.rows];
    updatedRows[last.rowIndex] = updatedRow;
    const statements = [...(pane.outcome?.statements ?? [])];
    statements[pane.activeStatement] = { ...rows, rows: updatedRows };

    set((s) => ({
      panes: patch(s.panes, id, {
        outcome: { ...pane.outcome!, statements },
        undo: pane.undo.slice(0, -1),
        redo: [...pane.redo, last],
      }),
    }));
  },

  redo: async (id) => {
    const pane = get().pane(id);
    const next = pane.redo[pane.redo.length - 1];
    if (!next) return;

    // Re-apply by inverting the inverse.
    const forward: RowEdit = {
      ...next.inverse,
      changes: [[next.inverse.changes[0]![0], next.after]],
    };
    await ipc.applyEdit(id, forward);

    const rows = activeRows(pane);
    if (!rows) return;
    const row = rows.rows[next.rowIndex];
    if (!row) return;

    const updatedRow = [...row];
    updatedRow[next.columnIndex] = next.after;
    const updatedRows = [...rows.rows];
    updatedRows[next.rowIndex] = updatedRow;
    const statements = [...(pane.outcome?.statements ?? [])];
    statements[pane.activeStatement] = { ...rows, rows: updatedRows };

    set((s) => ({
      panes: patch(s.panes, id, {
        outcome: { ...pane.outcome!, statements },
        undo: [...pane.undo, next],
        redo: pane.redo.slice(0, -1),
      }),
    }));
  },
}));
