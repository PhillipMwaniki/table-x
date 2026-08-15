/**
 * The controls a cell is edited with, chosen by what the value is.
 *
 * Short scalars stay in the grid — an input in the cell is the fastest way to
 * change a name or a number. Anything that needs room (JSON, long text) or
 * anything that a text box would let you get wrong (booleans, binary) opens a
 * panel instead.
 */

import { useEffect, useRef, useState } from "react";
import { Button, cx } from "../ui/primitives";
import { boolChoiceOf, byteSize, checkJson, hexDump, prettyJson } from "@/lib/editors";
import type { BoolChoice } from "@/lib/editors";
import type { Value } from "@/lib/types";

/** A single-line input, for values that fit on one. */
export function InlineEditor({
  draft,
  saving,
  onDraft,
  onCommit,
}: {
  draft: string;
  saving: boolean;
  onDraft: (value: string) => void;
  onCommit: () => void;
}) {
  return (
    <input
      autoFocus
      value={draft}
      disabled={saving}
      onChange={(e) => onDraft(e.target.value)}
      onBlur={onCommit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          onCommit();
        }
      }}
      style={{ height: "var(--row-height)" }}
      className="w-full border border-accent bg-surface-0 px-1.5 font-mono text-[length:var(--text-data)] outline-none"
    />
  );
}

/**
 * True, False, or NULL — and nothing else.
 *
 * A text box on a boolean column invites "yes", "1", "y", and a round trip to
 * find out which the engine accepts. Three options cannot be got wrong.
 */
export function BoolEditor({
  value,
  nullable,
  saving,
  onChoose,
}: {
  value: Value;
  nullable: boolean;
  saving: boolean;
  onChoose: (choice: BoolChoice) => void;
}) {
  return (
    <select
      autoFocus
      disabled={saving}
      value={boolChoiceOf(value)}
      onChange={(e) => onChoose(e.target.value as BoolChoice)}
      style={{ height: "var(--row-height)" }}
      className="w-full border border-accent bg-surface-0 px-1 font-mono text-[length:var(--text-data)] outline-none"
    >
      <option value="true">true</option>
      <option value="false">false</option>
      {/* Offered only where the column allows it, so the list cannot suggest a
          write the database will refuse. */}
      {nullable && <option value="null">NULL</option>}
    </select>
  );
}

/**
 * A panel for values that a single line hides.
 *
 * Ctrl+Enter saves and Escape cancels, which is the pair every multi-line
 * editor in every tool uses — Enter itself has to insert a newline.
 */
export function ValuePanel({
  title,
  draft,
  json,
  saving,
  onDraft,
  onCommit,
  onCancel,
}: {
  title: string;
  draft: string;
  /** Validate and offer reformatting. */
  json: boolean;
  saving: boolean;
  onDraft: (value: string) => void;
  onCommit: () => void;
  onCancel: () => void;
}) {
  const check = json ? checkJson(draft) : { valid: true as const };
  const area = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    area.current?.focus();
    // Caret at the end rather than selecting everything: the usual intent is to
    // amend a document, not replace it.
    const length = area.current?.value.length ?? 0;
    area.current?.setSelectionRange(length, length);
  }, []);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/40 p-6"
      onClick={onCancel}
    >
      <div
        role="dialog"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
        className="flex max-h-[80vh] w-[min(46rem,92vw)] flex-col overflow-hidden rounded-lg border border-border bg-surface-1 shadow-2xl"
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2">
          <span className="text-[12px] font-medium text-text">{title}</span>
          <span className="text-[10.5px] text-text-muted">
            Ctrl+Enter saves · Escape cancels
          </span>
          <div className="flex-1" />
          {json && (
            <Button
              variant="ghost"
              className="h-6"
              disabled={!check.valid}
              onClick={() => onDraft(prettyJson(draft))}
            >
              Reformat
            </Button>
          )}
        </div>

        <textarea
          ref={area}
          value={draft}
          disabled={saving}
          onChange={(e) => onDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              if (check.valid) onCommit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
            }
          }}
          spellCheck={false}
          className="min-h-[16rem] flex-1 resize-none bg-surface-0 p-3 font-mono text-[length:var(--text-data)] text-text outline-none"
        />

        <div className="flex shrink-0 items-center gap-2 border-t border-border px-3 py-2">
          {/* The database would answer with a message about the column; this
              one is about the character, while the text is still on screen. */}
          {!check.valid && (
            <span role="alert" className="min-w-0 flex-1 truncate text-[11px] text-danger">
              {check.error}
            </span>
          )}
          <div className="flex-1" />
          <Button variant="ghost" onClick={onCancel} disabled={saving}>
            Cancel
          </Button>
          <Button variant="primary" onClick={onCommit} busy={saving} disabled={!check.valid}>
            Save
          </Button>
        </div>
      </div>
    </div>
  );
}

/**
 * Bytes, shown rather than edited.
 *
 * No engine here reports enough about a binary column to validate a typed
 * replacement, and a byte array retyped by hand is a corrupted file. Reading it
 * is the useful part anyway: what is actually in this blob.
 */
export function BinaryViewer({ bytes, onClose }: { bytes: number[]; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const dump = hexDump(bytes);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/40 p-6"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-label="Binary value"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
        }}
        className="flex max-h-[80vh] w-[min(46rem,92vw)] flex-col overflow-hidden rounded-lg border border-border bg-surface-1 shadow-2xl"
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2">
          <span className="text-[12px] font-medium text-text">Binary value</span>
          <span className="font-mono text-[10.5px] text-text-muted">{byteSize(bytes.length)}</span>
          <div className="flex-1" />
          <Button
            variant="ghost"
            className="h-6"
            onClick={() => {
              void navigator.clipboard?.writeText(dump);
              setCopied(true);
            }}
          >
            {copied ? "Copied" : "Copy hex"}
          </Button>
          <Button variant="ghost" className="h-6" onClick={onClose}>
            Close
          </Button>
        </div>

        <pre
          className={cx(
            "min-h-0 flex-1 overflow-auto bg-surface-0 p-3",
            "font-mono text-[length:calc(var(--text-data)*0.95)] text-text",
          )}
        >
          {dump || "(empty)"}
        </pre>

        <p className="shrink-0 border-t border-border px-3 py-1.5 text-[10.5px] text-text-muted">
          Read-only. Editing binary by hand is how a file gets corrupted.
        </p>
      </div>
    </div>
  );
}
