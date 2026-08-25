/**
 * A menu at the pointer.
 *
 * Rendered where the right-click happened rather than anchored to the row, and
 * nudged back on screen when it would overflow — a menu whose items are off the
 * bottom edge is a menu with items nobody can reach.
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cx } from "./primitives";

export interface MenuItem {
  label: string;
  /** Absent on an item that only opens a submenu — there is nothing to pick. */
  onSelect?: (() => void) | undefined;
  /**
   * A nested menu, opened by pointing at this item.
   *
   * For a group that would otherwise be eight items long. A menu is read top to
   * bottom every time it opens, so the eight formats a row can be copied as
   * belong behind one line that says "Copy as" rather than in front of someone
   * looking for "Duplicate".
   */
  items?: MenuItem[] | undefined;
  /** Shown greyed with the reason as its tooltip. */
  disabledReason?: string | undefined;
  /** Draws a divider above this item. */
  separated?: boolean | undefined;
}

/**
 * Width a submenu is assumed to need when deciding which side to open on.
 *
 * A guess rather than a measurement: the decision has to be made before the
 * submenu exists, and being wrong costs a menu that opens leftwards when it
 * had room to open rightwards.
 */
const SUBMENU_WIDTH = 190;

/** Height a submenu is assumed to need, for the same reason and with the same
 *  caveat as [`SUBMENU_WIDTH`]. */
const SUBMENU_HEIGHT = 230;

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x, y });
  /** Whether submenus have to open leftwards to stay on screen. */
  const [flip, setFlip] = useState(false);

  // Measured after paint: the menu's size depends on its longest label, which
  // is not known until it is in the document.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const left = Math.min(x, window.innerWidth - width - 4);
    setPosition({
      x: left,
      y: Math.min(y, window.innerHeight - height - 4),
    });
    setFlip(left + width + SUBMENU_WIDTH > window.innerWidth);
  }, [x, y]);

  useEffect(() => {
    const close = () => onClose();
    const closeIfOutside = (e: PointerEvent) => {
      // A press *inside* the menu must not dismiss it. Pointerdown precedes
      // click, so closing here would unmount the item before the click could
      // land on it — the menu would open, then do nothing whatever you picked.
      if (ref.current?.contains(e.target as Node)) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // Capture, so a click that also does something else still dismisses this.
    window.addEventListener("pointerdown", closeIfOutside, true);
    window.addEventListener("keydown", onKey);
    // Any scroll moves the row this menu was opened against, leaving it
    // pointing at whatever slid underneath.
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("pointerdown", closeIfOutside, true);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", close, true);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      role="menu"
      style={{ left: position.x, top: position.y }}
      className="fixed z-50 min-w-40 rounded-md border border-border bg-surface-2 py-1 shadow-2xl"
    >
      {items.map((item) => (
        <Item key={item.label} item={item} flip={flip} onClose={onClose} />
      ))}
    </div>
  );
}

/** One row of a menu, which may itself hold a menu. */
function Item({ item, flip, onClose }: { item: MenuItem; flip: boolean; onClose: () => void }) {
  const [open, setOpen] = useState(false);
  /** Whether this item's submenu has to hang upwards to stay on screen. */
  const [up, setUp] = useState(false);
  const row = useRef<HTMLDivElement>(null);
  const disabled = Boolean(item.disabledReason);
  const nested = item.items && item.items.length > 0;

  return (
    <div
      ref={row}
      className="relative"
      // Opened by pointing rather than clicking, which is what every other
      // menu on the platform does. It closes on the way out rather than on a
      // timer: a submenu that lingers covers the item below the one you moved
      // to.
      onPointerEnter={() => {
        const rect = row.current?.getBoundingClientRect();
        // Measured on the way in rather than after opening: the submenu has to
        // know which way to hang before it is drawn, or it appears off the
        // bottom of the screen and then jumps.
        if (rect) setUp(rect.top + SUBMENU_HEIGHT > window.innerHeight);
        setOpen(!disabled && Boolean(nested));
      }}
      onPointerLeave={() => setOpen(false)}
    >
      <button
        role="menuitem"
        disabled={disabled}
        title={item.disabledReason}
        aria-haspopup={nested ? "menu" : undefined}
        aria-expanded={nested ? open : undefined}
        onClick={() => {
          // An item that only opens a submenu has nothing to do on a click, and
          // closing the menu under the pointer would be the opposite of what
          // clicking it looks like it should do.
          if (!item.onSelect) return;
          item.onSelect();
          onClose();
        }}
        className={cx(
          "flex w-full items-center gap-3 px-3 py-1 text-left text-[12px]",
          item.separated && "mt-1 border-t border-border pt-1.5",
          disabled
            ? "cursor-default text-text-muted/50"
            : cx(
                "text-text hover:bg-accent hover:text-accent-fg",
                open && "bg-accent text-accent-fg",
              ),
        )}
      >
        <span className="min-w-0 flex-1 truncate">{item.label}</span>
        {nested && <span aria-hidden>›</span>}
      </button>

      {nested && open && (
        <div
          role="menu"
          className={cx(
            "absolute z-50 min-w-44 rounded-md border border-border bg-surface-2 py-1 shadow-2xl",
            up ? "bottom-0" : "top-0",
            // No gap between the two: a pointer crossing a 2px strip that
            // belongs to neither would leave the parent item and close the
            // submenu it was on its way into.
            flip ? "right-full" : "left-full",
          )}
        >
          {item.items?.map((child) => (
            <Item key={child.label} item={child} flip={flip} onClose={onClose} />
          ))}
        </div>
      )}
    </div>
  );
}
