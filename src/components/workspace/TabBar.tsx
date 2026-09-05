/**
 * The tab strip.
 *
 * Each tab shows what it is and where it lives, because on a server with
 * several databases the name alone is ambiguous — `users` in `app_staging` and
 * `users` in `app_production` are different tables, and the difference is the
 * whole reason to have both open.
 *
 * Tabs drag into a new order, and a right-click offers to pin one or close the
 * rest. A pinned tab keeps to the left and loses its close button: it is the
 * one you keep coming back to among the ones you open and shut.
 */

import { useRef, useState } from "react";
import { cx } from "../ui/primitives";
import { ContextMenu } from "../ui/ContextMenu";
import type { MenuItem } from "../ui/ContextMenu";
import { tabsOf, useWorkspace } from "@/store/workspace";
import type { Tab } from "@/store/workspace";

/** How far a tab has to move before it is a drag rather than a click. */
const DRAG_THRESHOLD = 6;

export function TabBar({ connectionId }: { connectionId: string }) {
  const tabs = useWorkspace((s) => tabsOf(s, connectionId));
  const activeId = useWorkspace((s) => s.active[connectionId] ?? "");
  const { selectTab, closeTab, closeOtherTabs, closeTabsToRight, openQuery, moveTab, togglePin } =
    useWorkspace();
  const strip = useRef<HTMLDivElement>(null);
  /**
   * The tab being dragged, the slot it would drop into, and where to draw the
   * line for that slot. Measured when the pointer moves rather than when the
   * strip renders, since the measurement reads the DOM.
   */
  const [drag, setDrag] = useState<{ id: string; slot: number; left: number } | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; tab: Tab } | null>(null);

  /**
   * Which gap between tabs the pointer is nearest.
   *
   * Measured from the tabs themselves rather than computed from widths: a tab
   * is as wide as its title, and the strip scrolls.
   */
  const slotAt = (clientX: number): { slot: number; left: number } => {
    const el = strip.current;
    if (!el) return { slot: 0, left: 0 };
    const items = el.querySelectorAll<HTMLElement>("[role=tab]");
    // In the strip's content coordinates, so the line scrolls with the tabs.
    const origin = el.getBoundingClientRect().left - el.scrollLeft;
    let slot = 0;
    let end = 0;
    for (const item of items) {
      const { left, width, right } = item.getBoundingClientRect();
      if (clientX < left + width / 2) return { slot, left: left - origin };
      slot++;
      end = right - origin;
    }
    return { slot, left: end };
  };

  const drop = () => {
    if (!drag) return;
    const { id, slot } = drag;
    setDrag(null);
    const from = tabs.findIndex((t) => t.id === id);
    if (from === -1 || slot === from || slot === from + 1) return;
    moveTab(connectionId, id, tabs[slot]?.id ?? null);
  };

  const menuItems = (tab: Tab): MenuItem[] => {
    const index = tabs.findIndex((t) => t.id === tab.id);
    const others = tabs.filter((t) => t.id !== tab.id && !t.pinned).length;
    const toRight = tabs.slice(index + 1).filter((t) => !t.pinned).length;
    return [
      {
        label: tab.pinned ? "Unpin tab" : "Pin tab",
        onSelect: () => togglePin(connectionId, tab.id),
      },
      {
        label: "Close tab",
        separated: true,
        disabledReason: tab.pinned ? "Pinned. Unpin it first." : undefined,
        onSelect: () => closeTab(connectionId, tab.id),
      },
      {
        label: "Close other tabs",
        disabledReason: others === 0 ? "No other unpinned tabs" : undefined,
        onSelect: () => closeOtherTabs(connectionId, tab.id),
      },
      {
        label: "Close tabs to the right",
        disabledReason: toRight === 0 ? "Nothing unpinned to the right" : undefined,
        onSelect: () => closeTabsToRight(connectionId, tab.id),
      },
    ];
  };

  return (
    <div className="flex h-9 shrink-0 items-stretch border-b border-border bg-surface-2">
      <div ref={strip} className="relative flex min-w-0 flex-1 items-stretch overflow-x-auto">
        {/* Where the dragged tab would land: a line in the gap nearest the pointer. */}
        {drag && (
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-y-1 z-10 w-0.5 bg-accent"
            style={{ left: drag.left - 1 }}
          />
        )}
        {tabs.map((tab) => (
          <TabButton
            key={tab.id}
            tab={tab}
            active={tab.id === activeId}
            dragging={drag?.id === tab.id}
            onSelect={() => void selectTab(connectionId, tab.id)}
            onClose={() => closeTab(connectionId, tab.id)}
            onDrag={(clientX) => setDrag({ id: tab.id, ...slotAt(clientX) })}
            onDrop={drop}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, tab });
            }}
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

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(menu.tab)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

function TabButton({
  tab,
  active,
  dragging,
  onSelect,
  onClose,
  onDrag,
  onDrop,
  onContextMenu,
}: {
  tab: Tab;
  active: boolean;
  dragging: boolean;
  onSelect: () => void;
  onClose: () => void;
  /** The pointer's position while this tab is being dragged. */
  onDrag: (clientX: number) => void;
  onDrop: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  // Database first, then schema: that is the order the object is addressed in,
  // and the database is the part that changes underneath you.
  const context = [tab.database, tab.schema].filter(Boolean).join(" · ");
  /** Where the press landed, and whether it has moved far enough to be a drag. */
  const press = useRef<{ x: number; moved: boolean } | null>(null);
  /** Set by a drop so the click that follows it does not also select. */
  const dragged = useRef(false);

  return (
    <div
      role="tab"
      aria-selected={active}
      onClick={() => {
        if (dragged.current) {
          dragged.current = false;
          return;
        }
        onSelect();
      }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        // The close button is inside the tab; a press on it is not a drag.
        if ((e.target as HTMLElement).closest("button")) return;
        press.current = { x: e.clientX, moved: false };
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        const p = press.current;
        if (!p) return;
        if (!p.moved) {
          if (Math.abs(e.clientX - p.x) < DRAG_THRESHOLD) return;
          p.moved = true;
        }
        onDrag(e.clientX);
      }}
      onPointerUp={() => {
        const p = press.current;
        press.current = null;
        if (p?.moved) {
          dragged.current = true;
          onDrop();
        }
      }}
      onPointerCancel={() => {
        if (press.current?.moved) onDrop();
        press.current = null;
      }}
      onContextMenu={onContextMenu}
      // Middle-click closes, as in every browser and editor — except a pinned
      // tab, which is pinned precisely so that it does not go by accident.
      onAuxClick={(e) => {
        if (e.button === 1 && !tab.pinned) {
          e.preventDefault();
          onClose();
        }
      }}
      title={context ? `${tab.title} — ${context}` : tab.title}
      className={cx(
        "group flex min-w-0 max-w-52 shrink-0 cursor-default touch-none items-center gap-1.5 border-r border-border px-2.5 select-none",
        active ? "bg-surface-0" : "hover:bg-surface-1",
        dragging && "opacity-50",
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
                : tab.kind === "privileges"
                  ? "⚿"
                  : tab.kind === "notebook"
                    ? "▤▤"
                    : "›"}
      </span>

      <span className="flex min-w-0 flex-col leading-tight">
        <span className={cx("truncate text-[11.5px]", active ? "text-text" : "text-text-muted")}>
          {tab.title}
        </span>
        {context && <span className="truncate text-[9.5px] text-text-muted/80">{context}</span>}
      </span>

      {tab.running && (
        <span
          aria-label="Running"
          className="size-1.5 shrink-0 animate-pulse rounded-full bg-accent"
        />
      )}

      {tab.pinned ? (
        // The pin stands where the close button would, so the tab reads as
        // "this one stays" in the place the eye goes to shut it.
        <span
          aria-label="Pinned"
          title="Pinned — right-click to unpin"
          className="shrink-0 px-1 text-[10px] text-accent"
        >
          ⚲
        </span>
      ) : (
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
      )}
    </div>
  );
}
