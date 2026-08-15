/**
 * Ctrl+K over every action the app can do right now.
 *
 * Also where the keyboard shortcuts live: each command shows its own, so the
 * palette doubles as the reference nobody would otherwise go looking for.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { cx } from "./primitives";
import { fuzzyFilter } from "@/lib/fuzzy";
import { useCommands } from "@/store/commands";

export function CommandPalette() {
  const open = useCommands((s) => s.open);
  const setOpen = useCommands((s) => s.setOpen);
  const sources = useCommands((s) => s.sources);

  const [query, setQuery] = useState("");
  const [highlighted, setHighlighted] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);

  const commands = useMemo(() => Object.values(sources).flat(), [sources]);
  const matches = useMemo(() => fuzzyFilter(query, commands, (c) => c.title), [query, commands]);

  // Every keystroke changes the list, so the highlight returns to the top —
  // otherwise Enter runs whatever happens to be at the old index.
  useEffect(() => setHighlighted(0), [query]);

  // A palette that remembers last time's query is a palette you have to clear
  // before you can use it.
  useEffect(() => {
    if (open) {
      setQuery("");
      setHighlighted(0);
    }
  }, [open]);

  useEffect(() => {
    listRef.current
      ?.querySelector('[data-highlighted="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [highlighted]);

  if (!open) return null;

  const run = (index: number) => {
    const command = matches[index];
    if (!command) return;
    setOpen(false);
    command.run();
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-[12vh]"
      onClick={() => setOpen(false)}
    >
      <div
        role="dialog"
        aria-label="Command palette"
        onClick={(e) => e.stopPropagation()}
        className="flex max-h-[60vh] w-[min(34rem,92vw)] flex-col overflow-hidden rounded-lg border border-border bg-surface-1 shadow-2xl"
      >
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setHighlighted((i) => Math.min(i + 1, matches.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setHighlighted((i) => Math.max(i - 1, 0));
            } else if (e.key === "Enter") {
              e.preventDefault();
              run(highlighted);
            } else if (e.key === "Escape") {
              e.preventDefault();
              setOpen(false);
            }
          }}
          placeholder="Type a command…"
          aria-label="Search commands"
          className="h-11 w-full shrink-0 border-b border-border bg-transparent px-3 text-[13px] text-text outline-none placeholder:text-text-muted/60"
        />

        <ul ref={listRef} className="min-h-0 flex-1 overflow-y-auto py-1">
          {matches.length === 0 ? (
            <li className="px-3 py-3 text-[12px] text-text-muted">No matching command.</li>
          ) : (
            matches.map((command, index) => (
              <li key={command.id}>
                <button
                  data-highlighted={index === highlighted}
                  // Pointer move rather than enter: moving the mouse across the
                  // list should follow the pointer, but an arrow-key selection
                  // must not jump because the pointer happens to rest there.
                  onPointerMove={() => setHighlighted(index)}
                  onClick={() => run(index)}
                  className={cx(
                    "flex w-full items-center gap-3 px-3 py-1.5 text-left",
                    index === highlighted ? "bg-accent text-accent-fg" : "text-text",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate text-[12.5px]">{command.title}</span>
                  <span
                    className={cx(
                      "shrink-0 text-[10.5px]",
                      index === highlighted ? "text-accent-fg/70" : "text-text-muted",
                    )}
                  >
                    {command.group}
                  </span>
                  {command.shortcut && (
                    <span
                      className={cx(
                        "shrink-0 rounded border px-1 font-mono text-[10px]",
                        index === highlighted
                          ? "border-accent-fg/40 text-accent-fg"
                          : "border-border text-text-muted",
                      )}
                    >
                      {command.shortcut}
                    </span>
                  )}
                </button>
              </li>
            ))
          )}
        </ul>
      </div>
    </div>
  );
}
