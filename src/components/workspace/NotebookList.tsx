/**
 * Saved notebooks.
 *
 * Kept beside snippets rather than mixed into them: a snippet is a statement,
 * a notebook is a line of reasoning, and a list that interleaves the two makes
 * both harder to find.
 */

import { useEffect, useState } from "react";
import { Button, Spinner } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { Notebook } from "@/lib/types";

export function NotebookList({
  connectionId,
  onOpen,
}: {
  connectionId: string;
  onOpen: (notebook: Notebook) => void;
}) {
  const [notebooks, setNotebooks] = useState<Notebook[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  const refresh = async () => {
    setLoading(true);
    try {
      setNotebooks(await ipc.listNotebooks());
      setError(null);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const needle = filter.trim().toLowerCase();
  const visible = notebooks.filter(
    (n) =>
      !needle ||
      n.name.toLowerCase().includes(needle) ||
      n.cells.some((c) => c.source.toLowerCase().includes(needle)),
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-2 py-1">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter notebooks…"
          className="h-5 flex-1 rounded border border-border bg-surface-0 px-1.5 text-[11px] outline-none focus:border-accent"
        />
        <Button variant="ghost" className="h-5" onClick={() => void refresh()}>
          Refresh
        </Button>
      </div>

      {error && (
        <p role="alert" className="px-2 py-1 text-[11px] text-danger">
          {error}
        </p>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <div className="flex justify-center p-4">
            <Spinner className="text-text-muted" />
          </div>
        ) : visible.length === 0 ? (
          <p className="p-4 text-center text-[11px] text-text-muted">
            {notebooks.length === 0
              ? "No notebooks yet. Open one from the command palette and save it."
              : "Nothing matches that."}
          </p>
        ) : (
          <ul>
            {visible.map((notebook) => (
              <li key={notebook.id} className="group border-b border-border/50">
                <div className="flex items-center gap-2 px-2 py-1.5">
                  <button onClick={() => onOpen(notebook)} className="min-w-0 flex-1 text-left">
                    <span className="block truncate text-[12px] text-text">{notebook.name}</span>
                    <span className="block text-[10.5px] text-text-muted">
                      {notebook.cells.filter((c) => c.kind === "sql").length} queries ·{" "}
                      {new Date(notebook.updated_at).toLocaleString()}
                      {/* Named when it belongs to a different connection, since
                          its queries were written against that one's schema. */}
                      {notebook.connection_id && notebook.connection_id !== connectionId && (
                        <span className="ml-1 text-warn">· another connection</span>
                      )}
                    </span>
                  </button>
                  <button
                    onClick={async () => {
                      await ipc.deleteNotebook(notebook.id);
                      await refresh();
                    }}
                    className="rounded px-1 py-0.5 text-[10.5px] text-text-muted opacity-0 hover:text-danger group-hover:opacity-100"
                  >
                    Delete
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
