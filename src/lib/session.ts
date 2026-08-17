/**
 * What a workspace keeps across a restart.
 *
 * Losing unsaved SQL to a crash is the fastest way to lose someone's trust in a
 * tool, so the editor's contents survive. Almost nothing else does, and the
 * exclusions are the substance of this file.
 *
 * **Results do not survive.** A stored result set is a claim about a database
 * as it was whenever the app last closed, presented on reopening as though it
 * were current. Re-running is cheap and honest.
 *
 * **Undo history does not survive.** Each entry is a statement matched on the
 * values a row had when it was read. The edit path refuses anything that does
 * not match exactly one row, so replaying one is not *unsafe* — but offering
 * "undo" for a change made before a restart, against a database that has moved
 * on since, is offering something the word does not mean.
 *
 * **A comparison does not survive.** It is a one-shot report about two schemas
 * at one moment; restoring it would show a diff of something that may no longer
 * be true, with no indication of its age.
 */

import type { Tab, TabKind } from "@/store/workspace";
import type { NotebookCell } from "./types";

/** One tab, reduced to what is worth writing down. */
export interface SavedTab {
  kind: TabKind;
  title: string;
  database: string | null;
  schema?: string | undefined;
  sql: string;
  /** Which half of a table tab was showing. */
  view?: "data" | "structure";
  cells?: NotebookCell[];
  notebookId?: string | undefined;
}

export interface SavedWorkspace {
  tabs: SavedTab[];
  /** Index into `tabs`, rather than an id — ids are reassigned on restore. */
  active: number;
}

/**
 * Tab kinds worth restoring.
 *
 * The live panels are included because they hold no state of their own and
 * refetch on mount, so restoring one costs a query and gives back the view
 * somebody had open. `diff` is excluded: see the note above.
 */
const RESTORABLE: TabKind[] = ["query", "table", "notebook", "activity", "diagram", "privileges"];

/** Reduce a connection's tabs to what should be written to disk. */
export function toSaved(tabs: Tab[], activeId: string | undefined): SavedWorkspace {
  const keep = tabs.filter((tab) => RESTORABLE.includes(tab.kind));
  const active = keep.findIndex((tab) => tab.id === activeId);

  return {
    tabs: keep.map((tab) => ({
      kind: tab.kind,
      title: tab.title,
      database: tab.database,
      ...(tab.schema !== undefined ? { schema: tab.schema } : {}),
      sql: tab.sql,
      ...(tab.view ? { view: tab.view } : {}),
      ...(tab.cells ? { cells: tab.cells } : {}),
      ...(tab.notebookId !== undefined ? { notebookId: tab.notebookId } : {}),
    })),
    // An active tab that was not restorable leaves the first one selected
    // rather than nothing.
    active: active === -1 ? 0 : active,
  };
}

/**
 * Whether a restored tab should load itself.
 *
 * Only a table tab, and this is a safety rule rather than a performance one.
 * A query tab holds whatever somebody typed, which may be a `DELETE` they never
 * ran — running the editor's contents on launch would execute it without the
 * confirmation every other path insists on. A table tab's statement is one this
 * application generated and is a plain read.
 */
export function shouldAutoRun(tab: SavedTab): boolean {
  return tab.kind === "table" && tab.sql.trim().length > 0;
}

/**
 * Check a document read back off disk.
 *
 * Written by hand rather than trusted, because this file can be edited, can be
 * left half-written by a crash, and can have been produced by an older version.
 * Anything unrecognisable is dropped rather than restored into a broken tab.
 */
export function parseSaved(raw: unknown): SavedWorkspace | null {
  if (typeof raw !== "object" || raw === null) return null;
  const doc = raw as Partial<SavedWorkspace>;
  if (!Array.isArray(doc.tabs)) return null;

  const tabs = doc.tabs.filter(
    (tab): tab is SavedTab =>
      typeof tab === "object" &&
      tab !== null &&
      typeof (tab as SavedTab).title === "string" &&
      typeof (tab as SavedTab).sql === "string" &&
      RESTORABLE.includes((tab as SavedTab).kind),
  );

  if (tabs.length === 0) return null;
  const active = typeof doc.active === "number" ? doc.active : 0;
  return { tabs, active: active >= 0 && active < tabs.length ? active : 0 };
}
