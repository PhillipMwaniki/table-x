/**
 * Reading a query plan: which step is expensive, and which estimate was wrong.
 *
 * The parsing happens in Rust, one parser per engine. What is left is the part
 * that decides what a reader's eye lands on — and that is presentation, so it
 * lives here rather than in both places under two definitions of "expensive".
 */

import type { PlanNode } from "./types";

/**
 * A step's cost with its children's removed.
 *
 * This is the number that finds the problem. Every engine that reports a cost
 * reports a cumulative one, so the root always has the highest total and always
 * will — reading it tells you nothing. The step that *added* the most to that
 * total is the one to look at.
 */
export function selfCost(node: PlanNode): number | null {
  if (node.cost == null) return null;
  const below = node.children.reduce((sum, child) => sum + (child.cost ?? 0), 0);
  return Math.max(node.cost - below, 0);
}

/**
 * How far a row estimate missed by, as a ratio, when both numbers are known.
 *
 * A plan is only as good as its estimates: when a step expects 10 rows and gets
 * 400,000, every choice made above it was made for a different problem. This is
 * usually the actual answer to "why is it slow", which is why it is measured
 * symmetrically — over-estimating by a hundredfold is the same size of mistake
 * as under-estimating by one.
 */
export function estimateError(node: PlanNode): number | null {
  const { rows: expected, actual_rows: actual } = node;
  if (expected == null || actual == null) return null;
  // Zero rows is not an infinite error, it is no information.
  if (expected <= 0 || actual <= 0) return null;
  return actual > expected ? actual / expected : expected / actual;
}

/** An estimate off by more than this is worth pointing at. */
export const BAD_ESTIMATE = 10;

/** The largest self-cost anywhere in the tree, for scaling the bars. */
export function maxSelfCost(root: PlanNode): number {
  let max = 0;
  walk(root, (node) => {
    const cost = selfCost(node);
    if (cost != null && cost > max) max = cost;
  });
  return max;
}

/** Visit a node and everything under it. */
export function walk(node: PlanNode, visit: (node: PlanNode) => void): void {
  visit(node);
  for (const child of node.children) walk(child, visit);
}

/**
 * Row counts, at the magnitude a reader can hold.
 *
 * Plans deal in estimates that span nine orders of magnitude, and `1.2e6` in a
 * column of numbers is harder to compare at a glance than `1.2M`.
 */
export function formatRows(rows: number): string {
  if (rows < 1000) return Math.round(rows).toLocaleString();
  if (rows < 1_000_000) return `${(rows / 1000).toFixed(1)}K`;
  if (rows < 1_000_000_000) return `${(rows / 1_000_000).toFixed(1)}M`;
  return `${(rows / 1_000_000_000).toFixed(1)}B`;
}
