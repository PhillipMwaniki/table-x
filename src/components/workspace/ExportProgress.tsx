/**
 * A strip showing exports in flight.
 *
 * Sits above the results rather than in a floating toast: an export is work the
 * user started and may want to stop, so it belongs in the layout where it can
 * be read and acted on, not somewhere that fades.
 */

import { Button } from "../ui/primitives";
import { useExports } from "@/store/exports";

export function ExportProgress() {
  const running = useExports((s) => s.running);
  const cancel = useExports((s) => s.cancel);
  const entries = Object.values(running);

  if (entries.length === 0) return null;

  return (
    <div className="shrink-0 border-b border-border bg-surface-1">
      {entries.map((job) => {
        // The estimate is the planner's, so a real export can pass it. Capping
        // the bar keeps it from looking broken; the row count next to it stays
        // truthful.
        const fraction = job.total && job.total > 0 ? Math.min(1, job.rows / job.total) : null;

        return (
          <div key={job.id} className="flex items-center gap-2 px-2 py-1">
            <span className="shrink-0 text-[11px] text-text">Exporting {job.table}</span>

            <span className="min-w-0 flex-1">
              <span
                role="progressbar"
                aria-label={`Exporting ${job.table}`}
                aria-valuenow={fraction === null ? undefined : Math.round(fraction * 100)}
                className="block h-1 overflow-hidden rounded-full bg-surface-3"
              >
                <span
                  className={
                    fraction === null
                      ? // No estimate to work from — a moving bar says "still
                        // going" without claiming to know how far.
                        "block h-full w-1/3 animate-pulse rounded-full bg-accent"
                      : "block h-full rounded-full bg-accent transition-[width] duration-300"
                  }
                  style={fraction === null ? undefined : { width: `${fraction * 100}%` }}
                />
              </span>
            </span>

            <span className="shrink-0 font-mono text-[10.5px] text-text-muted">
              {job.rows.toLocaleString()}
              {job.total ? ` of about ${job.total.toLocaleString()}` : ""} rows
            </span>

            <Button
              variant="ghost"
              className="h-5"
              disabled={job.cancelling}
              onClick={() => void cancel(job.id)}
            >
              {job.cancelling ? "Stopping…" : "Cancel"}
            </Button>
          </div>
        );
      })}
    </div>
  );
}
