/**
 * The last thing between a statement and a table that no longer exists.
 *
 * Two levels, because a gate that fires the same way for everything is a gate
 * people learn to click through without reading. A bounded change asks for one
 * deliberate click; something unbounded — a `DROP`, or a `DELETE` with no
 * `WHERE` — asks for the connection's name to be typed, which cannot be done by
 * muscle memory.
 *
 * The statements are shown rather than summarised away. "This will run 3
 * destructive statements" asks whether you are sure; showing them asks the
 * better question, which is whether they say what you meant.
 */

import { useEffect, useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Banner, Button, Input, cx } from "../ui/primitives";
import type { HazardItem } from "@/lib/types";

export function ConfirmDestructive({
  open,
  connectionName,
  hazards,
  sql,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  /** Typed back by the user when anything is unbounded. */
  connectionName: string;
  hazards: HazardItem[];
  sql: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [typed, setTyped] = useState("");
  const unbounded = hazards.filter((h) => h.unbounded);
  const needsName = unbounded.length > 0;
  const ready = !needsName || typed.trim() === connectionName;

  // Cleared whenever the dialog opens, so a previous confirmation cannot arm
  // the next one.
  useEffect(() => {
    if (open) setTyped("");
  }, [open]);

  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={needsName ? "This destroys data that is not coming back" : "Confirm this change"}
      description={`On ${connectionName}.`}
      footer={
        <div className="flex items-center gap-2">
          <div className="flex-1" />
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="danger" disabled={!ready} onClick={onConfirm}>
            {needsName ? "Run it anyway" : "Run it"}
          </Button>
        </div>
      }
    >
      <div className="space-y-3">
        <ul className="space-y-1">
          {hazards.map((hazard, i) => (
            <li
              key={i}
              className={cx(
                "flex items-baseline gap-2 rounded border px-2 py-1.5 text-[12px]",
                hazard.unbounded
                  ? "border-danger/50 bg-danger/10 text-danger"
                  : "border-warn/40 bg-warn/5 text-warn",
              )}
            >
              <span aria-hidden className="font-mono">
                {hazard.unbounded ? "!!" : "!"}
              </span>
              <span>{hazard.summary}</span>
            </li>
          ))}
        </ul>

        <div>
          <p className="mb-1 text-[11px] text-text-muted">What will run:</p>
          <pre
            className="max-h-40 overflow-auto rounded border border-border bg-surface-2 p-2 font-mono text-[11px] whitespace-pre-wrap text-text"
            data-selectable
          >
            {sql.trim()}
          </pre>
        </div>

        {needsName && (
          <>
            <Banner tone="error">
              {unbounded.length === 1
                ? "One of these affects everything, not a selected subset."
                : `${unbounded.length} of these affect everything, not a selected subset.`}{" "}
              There is no undo for it.
            </Banner>
            <label className="block">
              <span className="mb-1 block text-[11px] text-text-muted">
                Type <span className="font-mono text-text">{connectionName}</span> to confirm you
                mean this connection.
              </span>
              <Input
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                placeholder={connectionName}
                autoFocus
                spellCheck={false}
                autoComplete="off"
              />
            </label>
          </>
        )}
      </div>
    </Dialog>
  );
}
