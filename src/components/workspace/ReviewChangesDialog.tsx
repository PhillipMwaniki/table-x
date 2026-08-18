/**
 * The statements a set of structure edits would run, before they run.
 *
 * This is the whole bargain that made the structure view editable. The schema
 * diff has always answered "does this say what you meant" by showing the script
 * rather than asking for a confirmation, and editing a table is the same
 * question with a smaller scope — so the edits produce a script here too, and
 * Apply is the second click rather than the first.
 *
 * The plan comes from the backend, not from here. Rendering DDL in the view
 * would mean a second emitter that could disagree with the one that executes,
 * and the interesting bugs live exactly in that gap.
 */

import { useEffect, useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Banner, Button, Spinner, cx } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { Change, DdlPlan } from "@/lib/types";

export function ReviewChangesDialog({
  open,
  onClose,
  connectionId,
  changes,
  onApplied,
}: {
  open: boolean;
  onClose: () => void;
  connectionId: string;
  changes: Change[];
  /** Applied cleanly — the caller refetches and clears what it had staged. */
  onApplied: (applied: number) => void;
}) {
  const [plan, setPlan] = useState<DdlPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;

    ipc
      .previewTableChanges(connectionId, changes)
      .then((next) => !cancelled && setPlan(next))
      .catch((e) => !cancelled && setError((e as IpcError).message));

    return () => {
      cancelled = true;
    };
  }, [open, connectionId, changes]);

  const blocked =
    Boolean(plan?.refusals.length) || Boolean(plan?.statements.some((s) => s.unsupported));
  const destructive = plan?.statements.filter((s) => s.destructive).length ?? 0;

  const apply = async () => {
    setApplying(true);
    setError(null);
    try {
      const outcome = await ipc.applyTableChanges(connectionId, changes);
      onApplied(outcome.applied);
      onClose();
    } catch (e) {
      // Kept open on failure. The message names the statement that failed, and
      // closing the dialog would take it away along with the list it refers to.
      setError((e as IpcError).message);
    } finally {
      setApplying(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Review changes"
      description={
        plan && !plan.transactional
          ? "This engine commits each statement as it runs, so a failure partway through leaves the earlier ones applied."
          : "These run as one transaction; a failure undoes the rest."
      }
      width="wide"
      footer={
        <div className="flex items-center gap-2">
          {destructive > 0 && (
            <span className="text-[11px] text-warn">
              {destructive} destructive statement{destructive === 1 ? "" : "s"}
            </span>
          )}
          <div className="flex-1" />
          <Button variant="ghost" onClick={onClose} disabled={applying}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => void apply()}
            busy={applying}
            disabled={!plan || blocked || plan.statements.length === 0}
          >
            {plan ? `Apply ${plan.statements.length}` : "Apply"}
          </Button>
        </div>
      }
    >
      <div className="space-y-3">
        {error && <Banner tone="error">{error}</Banner>}

        {plan?.refusals.map((why, i) => (
          <Banner key={i} tone="error">
            {why}
          </Banner>
        ))}

        {!plan && !error && (
          <div className="flex justify-center py-6">
            <Spinner className="text-text-muted" />
          </div>
        )}

        {plan?.statements.map((statement, i) => (
          <div
            key={i}
            className={cx(
              "rounded border",
              statement.unsupported
                ? "border-danger/40 bg-danger/5"
                : statement.destructive
                  ? "border-warn/40 bg-warn/5"
                  : "border-border bg-surface-2",
            )}
          >
            <pre
              data-selectable
              className="overflow-x-auto px-2.5 py-2 font-mono text-[11.5px] whitespace-pre text-text"
            >
              {statement.sql}
            </pre>
            {statement.note && (
              <p
                className={cx(
                  "border-t px-2.5 py-1.5 text-[11px]",
                  statement.unsupported
                    ? "border-danger/30 text-danger"
                    : "border-warn/30 text-warn",
                )}
              >
                {statement.note}
              </p>
            )}
          </div>
        ))}

        {plan?.statements.length === 0 && (
          <p className="text-[11.5px] text-text-muted">Nothing to run — the edits cancel out.</p>
        )}
      </div>
    </Dialog>
  );
}
