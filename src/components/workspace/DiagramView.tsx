/**
 * The schema as a picture: tables as boxes, foreign keys as lines between them.
 *
 * The layout is computed in Rust and arrives already positioned, so this draws
 * and does not decide. That split is deliberate — the placement has to be
 * identical every time the diagram is opened, and a layout that lives in a
 * component gets re-derived on every render by whatever state happens to be
 * around.
 *
 * Tables that reference nothing sit along the bottom and each table sits above
 * what it points at, so the lookup tables form the base and the tables that tie
 * everything together rise to the top.
 */

import { useEffect, useRef, useState } from "react";
import { Banner, Button, Spinner } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { Diagram, DiagramBox } from "@/lib/types";

const HEADER = 24;
const ROW = 16;
const MIN_SCALE = 0.2;
const MAX_SCALE = 2.5;

export function DiagramView({
  connectionId,
  schema,
}: {
  connectionId: string;
  schema: string | null;
}) {
  const [diagram, setDiagram] = useState<Diagram | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [view, setView] = useState({ scale: 1, x: 0, y: 0 });
  /** The table under the pointer, so its own relations stand out. */
  const [focus, setFocus] = useState<number | null>(null);

  const drag = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);
  const surface = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    ipc
      .schemaDiagram(connectionId, schema ?? undefined)
      .then((next) => {
        if (cancelled) return;
        setDiagram(next);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setError((e as IpcError).message);
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [connectionId, schema]);

  // Zooming is bound natively because React's wheel listener is passive, and a
  // passive listener cannot preventDefault — without which the page scrolls
  // behind the zoom.
  useEffect(() => {
    const el = surface.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey && Math.abs(e.deltaY) < 2) return;
      e.preventDefault();
      setView((was) => {
        const next = Math.min(
          MAX_SCALE,
          Math.max(MIN_SCALE, was.scale * (e.deltaY < 0 ? 1.1 : 0.9)),
        );
        // Zoom toward the pointer rather than the origin, so the thing being
        // looked at stays where it is.
        const rect = el.getBoundingClientRect();
        const px = e.clientX - rect.left;
        const py = e.clientY - rect.top;
        const ratio = next / was.scale;
        return { scale: next, x: px - (px - was.x) * ratio, y: py - (py - was.y) * ratio };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Spinner className="text-text-muted" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex-1 p-3">
        <Banner tone="error">{error}</Banner>
      </div>
    );
  }

  if (!diagram || diagram.boxes.length === 0) {
    return (
      <div className="flex flex-1 items-center justify-center px-6 text-center text-[12px] text-text-muted">
        {schema ? `${schema} has no tables to draw.` : "There are no tables to draw."}
      </div>
    );
  }

  /** Whether an edge touches the focused box. */
  const lit = (from: number, to: number) => focus == null || focus === from || focus === to;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 py-1">
        <span className="text-[11px] text-text-muted">
          {diagram.boxes.length} table{diagram.boxes.length === 1 ? "" : "s"} ·{" "}
          {diagram.edges.length} relation{diagram.edges.length === 1 ? "" : "s"}
        </span>
        <div className="flex-1" />
        <span className="tabular-nums text-[11px] text-text-muted">
          {Math.round(view.scale * 100)}%
        </span>
        <Button variant="ghost" className="h-5" onClick={() => setView({ scale: 1, x: 0, y: 0 })}>
          Reset view
        </Button>
      </div>

      {diagram.dangling.length > 0 && (
        <div className="shrink-0 px-2 pt-2">
          {/* Named rather than dropped: a diagram that silently omits a
              relation draws a schema in which it does not exist. */}
          <Banner tone="info">
            {diagram.dangling.length} key
            {diagram.dangling.length === 1 ? "" : "s"} point outside this schema and are not drawn:{" "}
            {diagram.dangling.slice(0, 4).join("; ")}
            {diagram.dangling.length > 4 && ` and ${diagram.dangling.length - 4} more`}
          </Banner>
        </div>
      )}

      <div
        ref={surface}
        className="min-h-0 flex-1 cursor-grab overflow-hidden active:cursor-grabbing"
        onPointerDown={(e) => {
          drag.current = { x: e.clientX, y: e.clientY, ox: view.x, oy: view.y };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d) return;
          setView((was) => ({ ...was, x: d.ox + (e.clientX - d.x), y: d.oy + (e.clientY - d.y) }));
        }}
        onPointerUp={() => {
          drag.current = null;
        }}
      >
        <svg
          width="100%"
          height="100%"
          role="img"
          aria-label={`Diagram of ${diagram.boxes.length} tables`}
        >
          <g transform={`translate(${view.x} ${view.y}) scale(${view.scale})`}>
            {diagram.edges.map((edge, i) => {
              const from = diagram.boxes[edge.from];
              const to = diagram.boxes[edge.to];
              if (!from || !to) return null;
              return (
                <path
                  key={i}
                  d={edge.reflexive ? loopPath(from) : linkPath(from, to)}
                  fill="none"
                  stroke={lit(edge.from, edge.to) ? "var(--color-accent)" : "var(--color-border)"}
                  strokeWidth={lit(edge.from, edge.to) ? 1.5 : 1}
                  opacity={lit(edge.from, edge.to) ? 0.9 : 0.35}
                />
              );
            })}

            {diagram.boxes.map((box, i) => (
              <g
                key={i}
                transform={`translate(${box.x} ${box.y})`}
                onPointerEnter={() => setFocus(i)}
                onPointerLeave={() => setFocus(null)}
              >
                <rect
                  width={box.width}
                  height={box.height}
                  rx={4}
                  fill="var(--color-surface-1)"
                  stroke={focus === i ? "var(--color-accent)" : "var(--color-border)"}
                  strokeWidth={focus === i ? 1.5 : 1}
                />
                <rect width={box.width} height={HEADER} rx={4} fill="var(--color-surface-2)" />
                <text
                  x={8}
                  y={16}
                  className="fill-[var(--color-text)] text-[11px] font-medium"
                  style={{ fontFamily: "var(--font-ui)" }}
                >
                  {box.table}
                </text>

                {box.columns.map((column, c) => (
                  <text
                    key={c}
                    x={8}
                    y={HEADER + 12 + c * ROW}
                    className="fill-[var(--color-text-muted)] text-[10px]"
                    style={{ fontFamily: "var(--font-data)" }}
                  >
                    {/* An arrow out means this column points at another table;
                        an arrow in means another table points here. A column
                        that is both gets both. */}
                    {column.outgoing ? "→ " : ""}
                    {column.incoming ? "◆ " : ""}
                    {column.name}
                  </text>
                ))}
              </g>
            ))}
          </g>
        </svg>
      </div>
    </div>
  );
}

/**
 * A curve from the referencing table down to the one it references.
 *
 * Leaves the bottom of the child and arrives at the top of the parent, because
 * the layout always places a parent below its children — so the line always
 * travels the same way and the direction can be read without an arrowhead.
 */
function linkPath(from: DiagramBox, to: DiagramBox): string {
  const x1 = from.x + from.width / 2;
  const y1 = from.y + from.height;
  const x2 = to.x + to.width / 2;
  const y2 = to.y;
  const bend = Math.max(24, Math.abs(y2 - y1) / 2);
  return `M ${x1} ${y1} C ${x1} ${y1 + bend}, ${x2} ${y2 - bend}, ${x2} ${y2}`;
}

/** A table that references itself: a loop off its right edge. */
function loopPath(box: DiagramBox): string {
  const x = box.x + box.width;
  const y = box.y + box.height / 2;
  return `M ${x} ${y - 8} C ${x + 28} ${y - 20}, ${x + 28} ${y + 20}, ${x} ${y + 8}`;
}
