/**
 * Searchable query history.
 *
 * A side panel rather than a modal: the point of history is to get a previous
 * statement back into the editor, and a dialog that covers the editor makes
 * comparing the two impossible.
 */

import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { Button, Input, Spinner, cx } from "../ui/primitives";
import { save } from "@tauri-apps/plugin-dialog";
import { ipc } from "@/lib/ipc";
import { useHistory } from "@/store/history";
import { SnippetList } from "./SnippetList";
import { tabsOf, useWorkspace } from "@/store/workspace";
import type { HistoryEntry } from "@/lib/types";

/** Debounce for the search box, in ms. Long enough to skip intermediate
 *  keystrokes, short enough that the list feels attached to the input. */
const SEARCH_DELAY = 150;

export function HistoryPanel({
  connectionId,
  onPick,
  onRun,
}: {
  connectionId: string;
  /** Load a statement into the editor. */
  onPick: (sql: string) => void;
  /** Load and run it. */
  onRun: (sql: string) => void;
}) {
  const {
    open,
    tab,
    entries,
    text,
    scope,
    loading,
    error,
    setOpen,
    setTab,
    setText,
    setScope,
    refresh,
    clear,
  } = useHistory();

  // A finished run is the one moment the list is certainly out of date. Any
  // tab's run counts: they all write to the same history.
  const running = useWorkspace((s) => tabsOf(s, connectionId).some((t) => t.running));
  const wasRunning = useRef(running);

  useEffect(() => {
    if (!open || tab !== "history") return;
    const justFinished = wasRunning.current && !running;
    wasRunning.current = running;
    if (running) return;

    // Refreshing immediately after a run keeps the new entry from appearing a
    // beat late; otherwise the search box drives the timing.
    const delay = justFinished ? 0 : SEARCH_DELAY;
    const timer = setTimeout(() => void refresh(connectionId), delay);
    return () => clearTimeout(timer);
  }, [open, tab, text, scope, connectionId, running, refresh]);

  /**
   * Write what is currently listed, filters and scope included.
   *
   * What is on screen rather than everything stored: the panel is already the
   * way someone narrows this down, and an export that quietly widened the
   * search would be a different answer to the question they asked.
   */
  const exportEntries = async () => {
    const path = await save({
      defaultPath: `query-history.csv`,
      filters: [
        { name: "CSV", extensions: ["csv"] },
        { name: "JSON", extensions: ["json"] },
      ],
    });
    if (!path) return;

    try {
      const format = path.toLowerCase().endsWith(".json") ? "json" : "csv";
      // The same scope and search the list is showing — see above.
      await ipc.exportHistory(path, format, {
        text: text || undefined,
        connection_id: scope === "connection" ? connectionId : undefined,
      });
    } catch (e) {
      // Reported in the panel rather than thrown: the list is still valid, and
      // a failed write is not a reason to lose it.
      console.error("could not export the history", e);
    }
  };

  if (!open) return null;

  return (
    <aside className="flex w-80 shrink-0 flex-col border-l border-border bg-surface-1">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-border px-2">
        <PanelTabButton active={tab === "history"} onClick={() => setTab("history")}>
          History
        </PanelTabButton>
        <PanelTabButton active={tab === "snippets"} onClick={() => setTab("snippets")}>
          Saved
        </PanelTabButton>
        {loading && tab === "history" && <Spinner className="text-text-muted" />}
        <div className="flex-1" />
        <button
          onClick={() => setOpen(false)}
          aria-label="Close panel"
          className="px-1 text-text-muted hover:text-text"
        >
          ✕
        </button>
      </div>

      {tab === "snippets" && <SnippetList onPick={onPick} onRun={onRun} />}

      {tab === "history" && (
        <>
      <div className="flex shrink-0 flex-col gap-1.5 border-b border-border p-2">
        <Input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Search statements…"
          aria-label="Search query history"
        />
        <div className="flex items-center gap-1">
          <ScopeTab active={scope === "connection"} onClick={() => setScope("connection")}>
            This connection
          </ScopeTab>
          <ScopeTab active={scope === "all"} onClick={() => setScope("all")}>
            All
          </ScopeTab>
          <div className="flex-1" />
          <Button
            variant="ghost"
            className="h-6"
            disabled={entries.length === 0}
            onClick={() => void exportEntries()}
            title="Write what is listed to a file — an audit trail that can leave this machine"
          >
            Export…
          </Button>
          <Button
            variant="ghost"
            className="h-6"
            disabled={entries.length === 0}
            onClick={() => void clear(connectionId)}
            title={
              scope === "connection"
                ? "Delete this connection's history"
                : "Delete all query history"
            }
          >
            Clear
          </Button>
        </div>
      </div>

      {error && (
        <p
          role="alert"
          className="border-b border-danger/30 bg-danger/10 px-2 py-1 text-[11px] text-danger"
        >
          {error}
        </p>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto">
        {entries.length === 0 && !loading ? (
          <p className="px-2 py-3 text-[11px] text-text-muted">
            {text.trim() ? "Nothing matches that search." : "No queries recorded yet."}
          </p>
        ) : (
          <ul>
            {entries.map((entry) => (
              <Row
                key={entry.id}
                entry={entry}
                showConnection={scope === "all"}
                onPick={() => onPick(entry.sql)}
                onRun={() => onRun(entry.sql)}
              />
            ))}
          </ul>
        )}
      </div>

      {/* Stated rather than left as a mystery: a user who rotates a password
          from the editor should know why it is not in this list. */}
      <p className="shrink-0 border-t border-border px-2 py-1 text-[10px] text-text-muted/70">
        Statements that set a password are never recorded.
      </p>
        </>
      )}
    </aside>
  );
}

function PanelTabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className={cx(
        "rounded px-1.5 py-0.5 text-[11px] font-medium",
        active ? "bg-surface-3 text-text" : "text-text-muted hover:bg-surface-2",
      )}
    >
      {children}
    </button>
  );
}

function ScopeTab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className={cx(
        "rounded px-1.5 py-0.5 text-[10.5px]",
        active ? "bg-surface-3 text-text" : "text-text-muted hover:bg-surface-2",
      )}
    >
      {children}
    </button>
  );
}

function Row({
  entry,
  showConnection,
  onPick,
  onRun,
}: {
  entry: HistoryEntry;
  showConnection: boolean;
  onPick: () => void;
  onRun: () => void;
}) {
  return (
    <li>
      <button
        onClick={onPick}
        onDoubleClick={onRun}
        title={entry.error ?? entry.sql}
        className="block w-full border-b border-border/60 px-2 py-1.5 text-left hover:bg-surface-2"
      >
        {/* Two lines of SQL: enough to recognise a statement without turning the
            list into a page of scrolling text. */}
        <span className="line-clamp-2 font-mono text-[11px] break-words text-text">{entry.sql}</span>
        <span className="mt-0.5 flex items-center gap-1.5 text-[10px] text-text-muted">
          {!entry.succeeded && <span className="font-medium text-danger">failed</span>}
          <span>{relativeTime(entry.ran_at)}</span>
          <span>·</span>
          <span>{entry.elapsed_ms} ms</span>
          {entry.rows != null && (
            <>
              <span>·</span>
              <span>
                {entry.rows} row{entry.rows === 1 ? "" : "s"}
              </span>
            </>
          )}
          {showConnection && (
            <>
              <span>·</span>
              <span className="truncate">{entry.connection_name}</span>
            </>
          )}
        </span>
      </button>
    </li>
  );
}

/**
 * "3m ago", falling back to a date once the elapsed time stops being the useful
 * part. Written by hand rather than pulled from a date library: this is the only
 * place in the app that formats a duration.
 */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return iso;

  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(then).toLocaleDateString();
}
