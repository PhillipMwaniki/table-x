/**
 * Modal dialog built on the native `<dialog>` element.
 *
 * Using the platform element rather than a hand-rolled overlay gets focus
 * trapping, the top layer, inert background content, and Escape handling from
 * the browser — all things a custom implementation gets subtly wrong.
 */

import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { cx } from "./primitives";

export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  width = "narrow",
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: string;
  children: ReactNode;
  footer?: ReactNode;
  width?: "narrow" | "wide";
}) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // showModal() throws if already open, and close() on a closed dialog is a
    // no-op that still fires nothing — so both are guarded on current state.
    if (open && !el.open) el.showModal();
    else if (!open && el.open) el.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      // Escape fires `cancel`; without preventing the default the element closes
      // itself and React state desyncs from the DOM.
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onClose={onClose}
      // Clicking the backdrop hits the dialog element itself, not its contents.
      onClick={(e) => {
        if (e.target === ref.current) onClose();
      }}
      aria-labelledby="dialog-title"
      className={cx(
        "m-auto rounded-lg border border-border bg-surface-1 p-0 text-text shadow-2xl",
        "backdrop:bg-black/50",
        "open:flex open:flex-col max-h-[85vh]",
        width === "narrow" ? "w-[min(30rem,92vw)]" : "w-[min(46rem,92vw)]",
      )}
    >
      <header className="shrink-0 border-b border-border px-4 py-3">
        <h2 id="dialog-title" className="text-[13px] font-semibold">
          {title}
        </h2>
        {description && (
          <p className="mt-0.5 text-[11px] text-text-muted">{description}</p>
        )}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">{children}</div>

      {footer && (
        <footer className="shrink-0 border-t border-border px-4 py-2.5">{footer}</footer>
      )}
    </dialog>
  );
}
