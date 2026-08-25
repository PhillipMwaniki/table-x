/**
 * The geometry two diagrams share.
 *
 * A live schema and a design are drawn by the same code on purpose: the whole
 * point of reverse engineering one into the other is that they are the same
 * picture, and two renderers would eventually disagree about where a line
 * leaves a box.
 */

import type { DiagramBox } from "./types";

/** Height of a box's title bar, and of each column row under it. */
export const HEADER = 24;
export const ROW = 16;

export const MIN_SCALE = 0.2;
export const MAX_SCALE = 2.5;

/**
 * A curve from the referencing table down to the one it references.
 *
 * Leaves the bottom of the child and arrives at the top of the parent, because
 * the automatic layout always places a parent below its children — so the line
 * always travels the same way and the direction can be read without an
 * arrowhead.
 *
 * A design lets tables be dragged anywhere, so that guarantee is gone the
 * moment somebody moves one. The curve still leaves the bottom and arrives at
 * the top; on a table dragged above its parent it doubles back, which reads as
 * exactly what it is rather than as a straight line with no direction.
 */
export function linkPath(from: DiagramBox, to: DiagramBox): string {
  const x1 = from.x + from.width / 2;
  const y1 = from.y + from.height;
  const x2 = to.x + to.width / 2;
  const y2 = to.y;
  const bend = Math.max(24, Math.abs(y2 - y1) / 2);
  return `M ${x1} ${y1} C ${x1} ${y1 + bend}, ${x2} ${y2 - bend}, ${x2} ${y2}`;
}

/** A table that references itself: a loop off its right edge. */
export function loopPath(box: DiagramBox): string {
  const x = box.x + box.width;
  const y = box.y + box.height / 2;
  return `M ${x} ${y - 8} C ${x + 28} ${y - 20}, ${x + 28} ${y + 20}, ${x} ${y + 8}`;
}

/**
 * Where a pointer at client coordinates lands on the canvas.
 *
 * The canvas is translated and scaled as a whole, so a drag has to undo both to
 * know which point on the diagram is under the cursor. Getting this wrong is
 * not subtle — the box leaps away from the pointer the moment the zoom is not
 * exactly 100%.
 */
export function canvasPoint(
  client: { x: number; y: number },
  surface: DOMRect,
  view: { x: number; y: number; scale: number },
): { x: number; y: number } {
  return {
    x: (client.x - surface.left - view.x) / view.scale,
    y: (client.y - surface.top - view.y) / view.scale,
  };
}

/** Keep a zoom within what the canvas can usefully draw. */
export function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}
