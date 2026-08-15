/**
 * The tab strip.
 *
 * Each tab shows what it is and where it lives, because on a server with
 * several databases the name alone is ambiguous — `users` in `app_staging` and
 * `users` in `app_production` are different tables, and the difference is the
 * whole reason to have both open.
 */

import { cx } from "../ui/primitives";
import { tabsOf, useWorkspace } from "@/store/workspace";
import type { Tab } from "@/store/workspace";

export function TabBar({ connectionId }: { connectionId: string }) {
  const tabs = useWorkspace((s) => tabsOf(s, connectionId));
  const activeId = useWorkspace((s) => s.active[connectionId] ?? "");
  const { selectTab, closeTab, openQuery } = useWorkspace();

  return (
    <div className="flex h-9 shrink-0 items-stretch border-b border-border bg-surface-2">
      <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
        {tabs.map((tab) => (
          <TabButton
            key={tab.id}
            tab={tab}
            active={tab.id === activeId}
            onSelect={() => void selectTab(connectionId, tab.id)}
            onClose={() => closeTab(connectionId, tab.id)}
          />
        ))}
      </div>

      <button
        onClick={() => openQuery(connectionId)}
        title="New query (Ctrl+T)"
        aria-label="New query tab"
        className="shrink-0 border-l border-border px-2.5 text-[14px] text-text-muted hover:bg-surface-3 hover:text-text"
      >
        +
      </button>
    </div>
  );
}

function TabButton({
  tab,
  active,
  onSelect,
  onClose,
}: {
  tab: Tab;
  active: boolean;
  onSelect: () => void;
  onClose: () => void;
}) {
  // Database first, then schema: that is the order the object is addressed in,
  // and the database is the part that changes underneath you.
  const context = [tab.database, tab.schema].filter(Boolean).join(" · ");

  return (
    <div
      role="tab"
      aria-selected={active}
      onClick={onSelect}
      // Middle-click closes, as in every browser and editor.
      onAuxClick={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          onClose();
        }
      }}
      title={context ? `${tab.title} — ${context}` : tab.title}
      className={cx(
        "group flex min-w-0 max-w-52 shrink-0 cursor-default items-center gap-1.5 border-r border-border px-2.5",
        active ? "bg-surface-0" : "hover:bg-surface-1",
      )}
    >
      <span aria-hidden className="shrink-0 text-[10px] text-text-muted">
        {tab.kind === "table"
          ? "▤"
          : tab.kind === "activity"
            ? "◴"
            : tab.kind === "diagram"
              ? "⬡"
              : tab.kind === "diff"
                ? "⇄"
                : "›"}
      </span>

      <span className="flex min-w-0 flex-col leading-tight">
        <span
          className={cx("truncate text-[11.5px]", active ? "text-text" : "text-text-muted")}
        >
          {tab.title}
        </span>
        {context && (
          <span className="truncate text-[9.5px] text-text-muted/80">{context}</span>
        )}
      </span>

      {tab.running && (
        <span
          aria-label="Running"
          className="size-1.5 shrink-0 animate-pulse rounded-full bg-accent"
        />
      )}

      <button
        onClick={(e) => {
          // The tab itself is clickable; without this, closing also selects.
          e.stopPropagation();
          onClose();
        }}
        aria-label={`Close ${tab.title}`}
        className={cx(
          "shrink-0 rounded px-1 text-[11px] text-text-muted hover:bg-surface-3 hover:text-text",
          // Kept out of the way until wanted, but always present for the active
          // tab so its close target does not move as the pointer arrives.
          active ? "opacity-70" : "opacity-0 group-hover:opacity-70",
        )}
      >
        ✕
      </button>
    </div>
  );
}
