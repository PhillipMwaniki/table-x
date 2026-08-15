/**
 * Saved queries, in the side panel beside history.
 *
 * History is what ran; this is what someone decided was worth keeping. They sit
 * together because the question — "where is that query I had?" — is the same
 * one, and the answer is in one of the two.
 */

import { useEffect, useState } from "react";
import { Input, Spinner, cx } from "../ui/primitives";
import { useSnippets } from "@/store/snippets";
import type { Snippet } from "@/lib/types";

export function SnippetList({
  onPick,
  onRun,
}: {
  /** Load a saved query into the editor. */
  onPick: (sql: string) => void;
  /** Load and run it. */
  onRun: (sql: string) => void;
}) {
  const { filter, loading, error, load, setFilter, remove, visible } = useSnippets();
  const [confirming, setConfirming] = useState<string | null>(null);

  useEffect(() => {
    void load();
  }, [load]);

  const snippets = visible();

  return (
    <>
      <div className="shrink-0 border-b border-border p-2">
        <Input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Search saved queries…"
          aria-label="Search saved queries"
        />
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
        {loading && snippets.length === 0 ? (
          <div className="flex justify-center py-4">
            <Spinner className="text-text-muted" />
          </div>
        ) : snippets.length === 0 ? (
          <p className="px-2 py-3 text-[11px] text-text-muted">
            {filter.trim()
              ? "Nothing matches that search."
              : "No saved queries yet. Run something worth keeping, then use Save query."}
          </p>
        ) : (
          <ul>
            {snippets.map((snippet) => (
              <Row
                key={snippet.id}
                snippet={snippet}
                confirming={confirming === snippet.id}
                onPick={() => onPick(snippet.sql)}
                onRun={() => onRun(snippet.sql)}
                onAskDelete={() => setConfirming(snippet.id)}
                onCancelDelete={() => setConfirming(null)}
                onConfirmDelete={() => {
                  setConfirming(null);
                  void remove(snippet.id);
                }}
              />
            ))}
          </ul>
        )}
      </div>
    </>
  );
}

function Row({
  snippet,
  confirming,
  onPick,
  onRun,
  onAskDelete,
  onCancelDelete,
  onConfirmDelete,
}: {
  snippet: Snippet;
  confirming: boolean;
  onPick: () => void;
  onRun: () => void;
  onAskDelete: () => void;
  onCancelDelete: () => void;
  onConfirmDelete: () => void;
}) {
  return (
    <li className="group border-b border-border/60">
      <div
        role="button"
        tabIndex={0}
        onClick={onPick}
        onDoubleClick={onRun}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            onPick();
          }
        }}
        title={snippet.sql}
        className="block w-full cursor-default px-2 py-1.5 text-left hover:bg-surface-2"
      >
        <span className="flex items-center gap-1.5">
          <span className="min-w-0 flex-1 truncate text-[11.5px] font-medium text-text">
            {snippet.name}
          </span>

          {/* Deleting something the user deliberately kept asks first, unlike
              closing a tab — this one cannot be undone from anywhere. */}
          {confirming ? (
            <span className="flex shrink-0 items-center gap-1">
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onConfirmDelete();
                }}
                className="rounded px-1 text-[10px] text-danger hover:bg-danger/10"
              >
                Delete
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onCancelDelete();
                }}
                className="rounded px-1 text-[10px] text-text-muted hover:bg-surface-3"
              >
                Keep
              </button>
            </span>
          ) : (
            <button
              onClick={(e) => {
                e.stopPropagation();
                onAskDelete();
              }}
              aria-label={`Delete ${snippet.name}`}
              className={cx(
                "shrink-0 rounded px-1 text-[11px] text-text-muted",
                "opacity-0 transition-opacity group-hover:opacity-100 hover:bg-surface-3 hover:text-text",
              )}
            >
              ✕
            </button>
          )}
        </span>

        <span className="mt-0.5 block truncate font-mono text-[10px] text-text-muted">
          {snippet.sql.replace(/\s+/g, " ").trim()}
        </span>
      </div>
    </li>
  );
}
