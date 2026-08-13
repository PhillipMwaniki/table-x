/**
 * Small UI primitives.
 *
 * Hand-rolled rather than pulled from a component library: the set needed here is
 * tiny, and a dependency would bring styling opinions that fight the design
 * tokens in `styles.css`.
 */

import { forwardRef } from "react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes } from "react";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

// ---------------------------------------------------------------------------

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant | undefined;
  busy?: boolean | undefined;
}

const BUTTON_VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-fg hover:opacity-90",
  secondary: "bg-surface-2 text-text hover:bg-surface-3 border border-border",
  ghost: "text-text-muted hover:bg-surface-2 hover:text-text",
  danger: "bg-danger text-white hover:opacity-90",
};

export function Button({
  variant = "secondary",
  busy = false,
  className,
  children,
  disabled,
  ...rest
}: ButtonProps) {
  return (
    <button
      {...rest}
      // A busy button stays visible but inert, so the layout does not shift
      // and the user cannot double-submit.
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      className={cx(
        "inline-flex h-7 items-center justify-center gap-1.5 rounded-md px-2.5",
        "text-[12px] font-medium whitespace-nowrap transition-colors",
        "disabled:pointer-events-none disabled:opacity-50",
        BUTTON_VARIANTS[variant],
        className,
      )}
    >
      {busy && <Spinner />}
      {children}
    </button>
  );
}

export function Spinner({ className }: { className?: string }) {
  return (
    <span
      role="status"
      aria-label="Loading"
      className={cx(
        "inline-block size-3 shrink-0 animate-spin rounded-full",
        "border-[1.5px] border-current border-t-transparent",
        className,
      )}
    />
  );
}

// ---------------------------------------------------------------------------

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(
  function Input({ className, ...rest }, ref) {
    return (
      <input
        ref={ref}
        {...rest}
        className={cx(
          "h-7 w-full rounded-md border border-border bg-surface-0 px-2",
          "text-[12px] text-text placeholder:text-text-muted/60",
          "focus:border-accent focus:outline-none",
          "disabled:opacity-50",
          className,
        )}
      />
    );
  },
);

export const Select = forwardRef<HTMLSelectElement, SelectHTMLAttributes<HTMLSelectElement>>(
  function Select({ className, children, ...rest }, ref) {
    return (
      <select
        ref={ref}
        {...rest}
        className={cx(
          "h-7 w-full rounded-md border border-border bg-surface-0 px-1.5",
          "text-[12px] text-text focus:border-accent focus:outline-none",
          className,
        )}
      >
        {children}
      </select>
    );
  },
);

// ---------------------------------------------------------------------------

export function Field({
  label,
  hint,
  error,
  children,
  className,
}: {
  label: string;
  hint?: string | undefined;
  error?: string | undefined;
  children: ReactNode;
  className?: string | undefined;
}) {
  return (
    <label className={cx("block", className)}>
      <span className="mb-1 block text-[11px] font-medium text-text-muted">{label}</span>
      {children}
      {/* Errors replace hints rather than stacking, so the form height is stable. */}
      {error ? (
        <span className="mt-1 block text-[11px] text-danger">{error}</span>
      ) : hint ? (
        <span className="mt-1 block text-[11px] text-text-muted/70">{hint}</span>
      ) : null}
    </label>
  );
}

export function Checkbox({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint?: string | undefined;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-2">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 size-3.5 shrink-0 accent-[var(--color-accent)]"
      />
      <span>
        <span className="block text-[12px] text-text">{label}</span>
        {hint && <span className="block text-[11px] text-text-muted/70">{hint}</span>}
      </span>
    </label>
  );
}

// ---------------------------------------------------------------------------

/** An inline message. `tone` picks the colour; the icon is intentionally absent
 *  so it reads as text rather than a notification. */
export function Banner({
  tone,
  children,
  onDismiss,
}: {
  tone: "error" | "success" | "info";
  children: ReactNode;
  onDismiss?: (() => void) | undefined;
}) {
  const tones = {
    error: "border-danger/40 bg-danger/10 text-danger",
    success: "border-ok/40 bg-ok/10 text-ok",
    info: "border-border bg-surface-2 text-text-muted",
  };
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={cx(
        "flex items-start gap-2 rounded-md border px-2.5 py-1.5 text-[12px]",
        tones[tone],
      )}
    >
      <span className="min-w-0 flex-1 break-words" data-selectable>
        {children}
      </span>
      {onDismiss && (
        <button
          onClick={onDismiss}
          aria-label="Dismiss"
          className="shrink-0 opacity-60 hover:opacity-100"
        >
          ✕
        </button>
      )}
    </div>
  );
}
