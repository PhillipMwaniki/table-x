/**
 * Modal dialog built on the native `<dialog>` element.
 *
 * Using the platform element rather than a hand-rolled overlay gets focus
 * trapping, the top layer, inert background content, and Escape handling from
 * the browser — all things a custom implementation gets subtly wrong.
 *
 * # Why the layout lives on an inner wrapper
 *
 * The obvious shape is to make the `<dialog>` itself the column flex container,
 * with the scroll region as `flex-1`. That renders correctly in WebView2 and
 * collapses in WebKitGTK: measured on 2.52.3, a dialog holding 996px of fields
 * came out **110px** tall — header, footer, and a 24px body that was nothing but
 * its own padding. It is specific to the combination. A `flex-basis` of `0%`
 * (what `flex-1` means) contributes nothing to the container's content-based
 * height, and when that container is the `<dialog>` element there is no other
 * height to fall back on, so the box resolves to almost nothing. The user-visible
 * result on Linux was a dialog cut off after the first fields with no usable way
 * to scroll — worst on the connection form, which is the tallest one here.
 *
 * The same column on a plain `<div>` does not collapse, and neither does the
 * dialog with a `flex-auto` basis; either change alone is enough. Both are kept:
 * the wrapper because a plain block container is the arrangement WebKit gets
 * right, and `flex-auto` because it is what makes the height come from content in
 * the first place. Keeping the `<dialog>` in its user-agent block display also
 * drops the need for Tailwind's `open:` variant, which is a small mercy — it
 * compiles to `:is([open], :popover-open, :open)`, and `:open` is still
 * unimplemented here, so the rule survives only on forgiving `:is()` parsing.
 * (That parsing does work: the collapsed dialog above computed `display: flex`,
 * which is how the selector was ruled out as the cause.)
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
        width === "narrow" ? "w-[min(30rem,92vw)]" : "w-[min(46rem,92vw)]",
      )}
    >
      <div className="flex max-h-[85vh] flex-col">
        <header className="shrink-0 border-b border-border px-4 py-3">
          <h2 id="dialog-title" className="text-[13px] font-semibold">
            {title}
          </h2>
          {description && <p className="mt-0.5 text-[11px] text-text-muted">{description}</p>}
        </header>

        {/* `flex-auto` rather than `flex-1`: a `0%` basis contributes nothing to a
            content-sized column, which is what collapsed this dialog in WebKitGTK.
            An `auto` basis contributes the content height — capped by the
            wrapper's max-height above — and `min-h-0` is what still lets it shrink
            below that content height so it scrolls instead of overflowing. */}
        <div className="min-h-0 flex-auto overflow-y-auto px-4 py-3">{children}</div>

        {footer && (
          <footer className="shrink-0 border-t border-border px-4 py-2.5">{footer}</footer>
        )}
      </div>
    </dialog>
  );
}
