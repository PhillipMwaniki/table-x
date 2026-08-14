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
  onSelect: () => void;
  /** Shown greyed with the reason as its tooltip. */
  disabledReason?: string | undefined;
  /** Draws a divider above this item. */
  separated?: boolean | undefined;
}

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

  // Measured after paint: the menu's size depends on its longest label, which
  // is not known until it is in the document.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPosition({
      x: Math.min(x, window.innerWidth - width - 4),
      y: Math.min(y, window.innerHeight - height - 4),
    });
  }, [x, y]);

  useEffect(() => {
    const close = () => onClose();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // Capture, so a click that also does something else still dismisses this.
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("keydown", onKey);
    // Any scroll moves the row this menu was opened against, leaving it
    // pointing at whatever slid underneath.
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("pointerdown", close, true);
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
        <button
          key={item.label}
          role="menuitem"
          disabled={Boolean(item.disabledReason)}
          title={item.disabledReason}
          onClick={() => {
            item.onSelect();
            onClose();
          }}
          className={cx(
            "block w-full px-3 py-1 text-left text-[12px]",
            item.separated && "mt-1 border-t border-border pt-1.5",
            item.disabledReason
              ? "cursor-default text-text-muted/50"
              : "text-text hover:bg-accent hover:text-accent-fg",
          )}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
