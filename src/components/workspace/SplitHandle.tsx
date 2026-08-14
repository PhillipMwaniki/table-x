/**
 * The draggable divider between the editor and its results.
 *
 * Pointer events rather than mouse events, so a trackpad, a pen, and a touch
 * screen all work from one code path; pointer capture keeps the drag attached
 * to this element even as the pointer travels over the editor or the grid,
 * which would otherwise swallow the move events.
 */

import { useRef, useState } from "react";
import { cx } from "../ui/primitives";
import { clampRatio, DEFAULT_SETTINGS } from "@/lib/settings";

/** Keyboard nudge per arrow press, as a fraction of the pane. */
const STEP = 0.02;

export function SplitHandle({
  /** Height of the pane being divided, measured when a drag starts. */
  containerRef,
  ratio,
  onPreview,
  onCommit,
}: {
  containerRef: React.RefObject<HTMLElement | null>;
  ratio: number;
  /** Called continuously while dragging, for the live layout. */
  onPreview: (ratio: number) => void;
  /** Called once the drag ends, when the value is worth storing. */
  onCommit: (ratio: number) => void;
}) {
  const dragging = useRef(false);
  const [active, setActive] = useState(false);

  const ratioAt = (clientY: number): number => {
    const rect = containerRef.current?.getBoundingClientRect();
    if (!rect || rect.height === 0) return ratio;
    return clampRatio((clientY - rect.top) / rect.height);
  };

  const nudge = (delta: number) => {
    const next = clampRatio(ratio + delta);
    onPreview(next);
    onCommit(next);
  };

  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize the editor"
      aria-valuenow={Math.round(ratio * 100)}
      aria-valuemin={12}
      aria-valuemax={85}
      tabIndex={0}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        dragging.current = true;
        setActive(true);
      }}
      onPointerMove={(e) => {
        if (!dragging.current) return;
        onPreview(ratioAt(e.clientY));
      }}
      onPointerUp={(e) => {
        if (!dragging.current) return;
        e.currentTarget.releasePointerCapture(e.pointerId);
        dragging.current = false;
        setActive(false);
        onCommit(ratioAt(e.clientY));
      }}
      // A cancelled pointer — a system gesture, a lost window — leaves the
      // layout where it was dragged to rather than snapping back.
      onPointerCancel={() => {
        dragging.current = false;
        setActive(false);
        onCommit(ratio);
      }}
      onKeyDown={(e) => {
        if (e.key === "ArrowUp") {
          e.preventDefault();
          nudge(-STEP);
        } else if (e.key === "ArrowDown") {
          e.preventDefault();
          nudge(STEP);
        }
      }}
      // Double-click restores the default split, which is the standard escape
      // from having dragged something somewhere unhelpful.
      onDoubleClick={() => {
        onPreview(DEFAULT_SETTINGS.editorRatio);
        onCommit(DEFAULT_SETTINGS.editorRatio);
      }}
      title="Drag to resize · double-click to reset"
      className={cx(
        // The hit area is taller than the line it draws: a 1px target is a
        // thing you hunt for, and this one sits between two panes people are
        // constantly clicking into.
        "group relative h-2 shrink-0 cursor-row-resize",
        "before:absolute before:inset-x-0 before:top-1/2 before:h-px before:-translate-y-1/2",
        "before:bg-border before:transition-colors",
        active ? "before:bg-accent" : "hover:before:bg-accent/60",
      )}
    >
      {/* A grip, shown only when the pointer is near, so the divider reads as
          draggable without drawing a permanent line of dots across the pane. */}
      <span
        aria-hidden
        className={cx(
          "pointer-events-none absolute left-1/2 top-1/2 h-0.5 w-8 -translate-x-1/2 -translate-y-1/2 rounded-full",
          "opacity-0 transition-opacity group-hover:opacity-100",
          active ? "bg-accent opacity-100" : "bg-border",
        )}
      />
    </div>
  );
}
