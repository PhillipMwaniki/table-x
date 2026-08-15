/**
 * How the engine intends to run a statement.
 *
 * Drawn as a tree with the flow going down: the root is the last thing to
 * happen and the leaves are the scans that feed it, which is the shape every
 * engine's own output already has and the one people read plans in.
 *
 * Two things are highlighted, because they are the two things a plan is read to
 * find. The cost bar shows each step's *own* cost rather than its cumulative
 * one — the root always has the highest total, so highlighting that would point
 * at the answer's container instead of the answer. And a row estimate that
 * missed by a wide margin is flagged outright, because when a step expects ten
 * rows and gets four hundred thousand, every choice above it was made for a
 * different problem.
 */

import { useState } from "react";
import { Button, cx } from "../ui/primitives";
import { BAD_ESTIMATE, estimateError, formatRows, maxSelfCost, selfCost } from "@/lib/plan";
import type { Plan, PlanNode } from "@/lib/types";

export function PlanView({ plan, onClose }: { plan: Plan; onClose: () => void }) {
  const [showRaw, setShowRaw] = useState(false);
  const scale = maxSelfCost(plan.root);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
        <span className="text-[11px] font-medium text-text">Query plan</span>
        <span className="text-[11px] text-text-muted">
          {plan.analyzed
            ? "Measured — the statement was run inside a transaction and rolled back."
            : "Estimated — the statement was not run."}
        </span>

        <div className="flex-1" />

        <Button
          variant="ghost"
          className={cx("h-5", showRaw && "bg-surface-3 text-text")}
          onClick={() => setShowRaw(!showRaw)}
          aria-pressed={showRaw}
          title="The server's own output, before this view rearranged it"
        >
          Raw
        </Button>
        <Button variant="ghost" className="h-5" onClick={onClose}>
          Back to results
        </Button>
      </div>

      {showRaw ? (
        // A parser only lifts what it was taught to look for, so the engine's
        // own text stays one click away rather than being replaced by this.
        <pre
          className="min-h-0 flex-1 overflow-auto p-2 font-mono text-[length:var(--text-data)] leading-relaxed"
          data-selectable
        >
          {plan.raw}
        </pre>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto p-1">
          <Node node={plan.root} depth={0} scale={scale} analyzed={plan.analyzed} />
        </div>
      )}
    </div>
  );
}

function Node({
  node,
  depth,
  scale,
  analyzed,
}: {
  node: PlanNode;
  depth: number;
  /** The largest self-cost in the plan, so every bar is drawn to one ruler. */
  scale: number;
  analyzed: boolean;
}) {
  const cost = selfCost(node);
  const error = estimateError(node);
  const bad = error != null && error >= BAD_ESTIMATE;
  const share = cost != null && scale > 0 ? cost / scale : 0;

  return (
    <div>
      <div
        className={cx(
          "flex items-baseline gap-2 rounded px-1.5 py-1 hover:bg-surface-2",
          bad && "bg-warn/10",
        )}
        style={{ marginLeft: depth * 16 }}
      >
        <span className="min-w-0 flex-1">
          <span className="font-medium text-text">{node.label}</span>
          {node.detail && (
            <span className="ml-1.5 font-mono text-[11px] text-text-muted">{node.detail}</span>
          )}
        </span>

        {node.rows != null && (
          <span
            className={cx("shrink-0 tabular-nums", bad ? "text-warn" : "text-text-muted")}
            title={
              analyzed && node.actual_rows != null
                ? `Expected ${formatRows(node.rows)}, got ${formatRows(node.actual_rows)}`
                : "Rows the planner expects"
            }
          >
            {analyzed && node.actual_rows != null
              ? `${formatRows(node.actual_rows)} of ${formatRows(node.rows)}`
              : formatRows(node.rows)}
            {bad && (
              <span className="ml-1 font-medium">×{Math.round(error!).toLocaleString()}</span>
            )}
          </span>
        )}

        {node.actual_ms != null && (
          <span className="w-16 shrink-0 text-right tabular-nums text-text-muted">
            {node.actual_ms.toFixed(1)}ms
          </span>
        )}

        {/* Drawn to the plan's own scale. The number itself is in no unit worth
            printing — PostgreSQL's cost is arbitrary page fetches, SQL Server's
            is a different arbitrary unit — so only the comparison is shown. */}
        {cost != null && (
          <span
            className="h-1.5 w-20 shrink-0 self-center overflow-hidden rounded-full bg-surface-3"
            title={`Cost of this step alone: ${cost.toFixed(2)}`}
          >
            <span
              className={cx("block h-full rounded-full", share > 0.5 ? "bg-warn" : "bg-accent")}
              style={{ width: `${Math.max(share * 100, 2)}%` }}
            />
          </span>
        )}
      </div>

      {node.children.map((child, i) => (
        <Node key={i} node={child} depth={depth + 1} scale={scale} analyzed={analyzed} />
      ))}
    </div>
  );
}
