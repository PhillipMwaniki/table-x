/**
 * A schema design on a canvas.
 *
 * The diagram view next door draws a database. This draws a document: the same
 * boxes and lines, but the tables can be moved and where they end up is part of
 * what is saved. Nothing here touches a server — a design is edited while
 * disconnected and turned into SQL when it is ready, which is the whole reason
 * it is a document rather than a view.
 *
 * The layout still comes from Rust. A table nobody has dragged is placed by the
 * same code that places a live schema, so a design reverse engineered from a
 * database opens looking like that database; a table that has been dragged
 * keeps its position and everything else stays where it was.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Banner, Button, Spinner } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import { HEADER, ROW, canvasPoint, clampScale, linkPath, loopPath } from "@/lib/erd";
import type { Design, Diagram, DiffReport } from "@/lib/types";

/**
 * How long a move sits before it is written to disk.
 *
 * A drag is a stream of positions and only the last one matters. Writing each
 * frame would put a file write in the middle of dragging a box across the
 * screen.
 */
const SAVE_DELAY = 600;

export function DesignView({
  design,
  onChange,
  onScript,
  onSync,
}: {
  design: Design;
  /** A design the store has accepted, so the tab can follow a rename. */
  onChange: (design: Design) => void;
  /** Show the script that would build this design from nothing. */
  onScript: (report: DiffReport) => void;
  /**
   * Show what it would take to bring a database up to this design.
   *
   * Absent when there is no database in front of the user — a design opened
   * from a file before connecting to anything. Everything else here works
   * without a server, which is the point of a design being a document.
   */
  onSync?: (() => void) | undefined;
}) {
  const [diagram, setDiagram] = useState<Diagram | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** The name while it is being typed, so a half-typed one is not saved. */
  const [renaming, setRenaming] = useState<string | null>(null);
  const [view, setView] = useState({ scale: 1, x: 0, y: 0 });
  /** The table under the pointer, so its own relations stand out. */
  const [focus, setFocus] = useState<number | null>(null);

  /** A pan of the whole canvas, or a drag of one box. */
  const pan = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);
  const moving = useRef<{ table: string; dx: number; dy: number } | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);

  // The laid-out diagram is asked for rather than derived here, so a design and
  // the database it came from are drawn by one implementation.
  const draw = useCallback(() => {
    ipc
      .designDiagram(design.id)
      .then(setDiagram)
      .catch((e) => setError((e as IpcError).message));
  }, [design.id]);

  useEffect(() => draw(), [draw]);

  // Bound natively because React's wheel listener is passive, and a passive
  // listener cannot preventDefault — without which the page scrolls behind the
  // zoom.
  useEffect(() => {
    const el = surface.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey && Math.abs(e.deltaY) < 2) return;
      e.preventDefault();
      setView((was) => {
        const next = clampScale(was.scale * (e.deltaY < 0 ? 1.1 : 0.9));
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

  /** Write the design to the store, a beat after it stops changing. */
  const persist = useCallback(
    (next: Design) => {
      onChange(next);
      if (pending.current) clearTimeout(pending.current);
      pending.current = setTimeout(() => {
        pending.current = null;
        ipc
          .saveDesign(next)
          .then(onChange)
          .catch((e) => setError((e as IpcError).message));
      }, SAVE_DELAY);
    },
    [onChange],
  );

  // A move still in the timer when the tab closes would be a move that did not
  // happen, which is indistinguishable from the drag not having worked.
  useEffect(() => {
    return () => {
      if (pending.current) {
        clearTimeout(pending.current);
        void ipc.saveDesign(design).catch(() => {});
      }
    };
  }, [design]);

  const place = (table: string, x: number, y: number) => {
    const layout = design.layout.filter((p) => p.table !== table);
    layout.push({ table, x, y });
    persist({ ...design, layout });
  };

  const rename = () => {
    const name = renaming?.trim();
    setRenaming(null);
    // An empty name is refused by the store; treating it as "no change" here
    // means the box simply springs back rather than showing an error about
    // something the user was in the middle of doing.
    if (!name || name === design.name) return;
    persist({ ...design, name });
  };

  /** Write the design where the user chooses, and remember where that was. */
  const toFile = async () => {
    const path = await save({
      defaultPath: design.path ?? `${design.name}.erd`,
      filters: [{ name: "Table X design", extensions: ["erd"] }],
    });
    if (!path) return;
    setBusy(true);
    try {
      onChange(await ipc.writeDesignFile(design.id, path));
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  const script = async () => {
    setBusy(true);
    try {
      onScript(await ipc.designScript(design.id));
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  if (error) {
    return (
      <div className="flex-1 p-3">
        <Banner tone="error" onDismiss={() => setError(null)}>
          {error}
        </Banner>
      </div>
    );
  }

  if (!diagram) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Spinner className="text-text-muted" />
      </div>
    );
  }

  /** Whether an edge touches the focused box. */
  const lit = (from: number, to: number) => focus == null || focus === from || focus === to;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2 py-1">
        {/* The name is edited in place. A design is a document and its name is
            the first thing about it somebody wants to change; a dialog for one
            field would be a dialog nobody opens. */}
        {renaming === null ? (
          <button
            onClick={() => setRenaming(design.name)}
            title={design.path ?? "Not saved to a file yet"}
            className="max-w-64 truncate rounded px-1 text-[12px] font-medium text-text hover:bg-surface-2"
          >
            {design.name}
          </button>
        ) : (
          <input
            autoFocus
            value={renaming}
            onChange={(e) => setRenaming(e.target.value)}
            onBlur={rename}
            onKeyDown={(e) => {
              if (e.key === "Enter") rename();
              if (e.key === "Escape") setRenaming(null);
            }}
            aria-label="Design name"
            className="h-5 w-64 rounded border border-accent bg-surface-0 px-1 text-[12px] outline-none"
          />
        )}

        <span className="text-[11px] text-text-muted">
          {diagram.boxes.length} table{diagram.boxes.length === 1 ? "" : "s"} ·{" "}
          {diagram.edges.length} relation{diagram.edges.length === 1 ? "" : "s"} ·{" "}
          <span className="font-mono">{design.driver}</span>
        </span>

        <div className="flex-1" />

        <Button
          variant="ghost"
          className="h-5"
          busy={busy}
          onClick={() => void toFile()}
          title={design.path ? `Last saved to ${design.path}` : "Save this design as a .erd file"}
        >
          {design.path ? "Save to file" : "Save to file…"}
        </Button>
        <Button variant="ghost" className="h-5" busy={busy} onClick={() => void script()}>
          SQL script
        </Button>
        {/* Only where there is a database to compare with. A design opened from
            a file before connecting to anything has nothing to answer this. */}
        {onSync && (
          <Button variant="ghost" className="h-5" onClick={onSync}>
            Compare with database…
          </Button>
        )}
        {/* Clearing the saved positions rather than computing new ones: the
            automatic layout is what a design with nothing moved already looks
            like, so this is exactly "forget where I put things". */}
        <Button
          variant="ghost"
          className="h-5"
          disabled={design.layout.length === 0}
          onClick={() => persist({ ...design, layout: [] })}
          title="Put every table back where the automatic layout would place it"
        >
          Auto-arrange
        </Button>
        <span className="tabular-nums text-[11px] text-text-muted">
          {Math.round(view.scale * 100)}%
        </span>
        <Button variant="ghost" className="h-5" onClick={() => setView({ scale: 1, x: 0, y: 0 })}>
          Reset view
        </Button>
      </div>

      {diagram.boxes.length === 0 ? (
        <div className="flex flex-1 items-center justify-center px-6 text-center text-[12px] text-text-muted">
          This design has no tables yet. Reverse engineer a schema into it, or add a table.
        </div>
      ) : (
        <div
          ref={surface}
          className="min-h-0 flex-1 cursor-grab overflow-hidden active:cursor-grabbing"
          onPointerDown={(e) => {
            // Only when the press did not land on a box — a box handles its own
            // pointer down and stops it here.
            pan.current = { x: e.clientX, y: e.clientY, ox: view.x, oy: view.y };
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            const held = moving.current;
            if (held) {
              const at = canvasPoint({ x: e.clientX, y: e.clientY }, rect, view);
              setDiagram((was) =>
                was
                  ? {
                      ...was,
                      boxes: was.boxes.map((b) =>
                        b.table === held.table ? { ...b, x: at.x - held.dx, y: at.y - held.dy } : b,
                      ),
                    }
                  : was,
              );
              return;
            }
            const d = pan.current;
            if (!d) return;
            setView((was) => ({
              ...was,
              x: d.ox + (e.clientX - d.x),
              y: d.oy + (e.clientY - d.y),
            }));
          }}
          onPointerUp={() => {
            const held = moving.current;
            if (held) {
              const box = diagram.boxes.find((b) => b.table === held.table);
              // Written on release rather than during the drag: the positions
              // in between are not places anybody chose.
              if (box) place(box.table, box.x, box.y);
              moving.current = null;
            }
            pan.current = null;
          }}
        >
          <svg
            width="100%"
            height="100%"
            role="img"
            aria-label={`Design ${design.name}, ${diagram.boxes.length} tables`}
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
                  key={box.table}
                  transform={`translate(${box.x} ${box.y})`}
                  className="cursor-move"
                  onPointerEnter={() => setFocus(i)}
                  onPointerLeave={() => setFocus(null)}
                  onPointerDown={(e) => {
                    // Stopped here, or the canvas would pan at the same time
                    // and the box would move twice as fast as the pointer.
                    e.stopPropagation();
                    const rect = surface.current?.getBoundingClientRect();
                    if (!rect) return;
                    const at = canvasPoint({ x: e.clientX, y: e.clientY }, rect, view);
                    // The grab offset, so the box does not jump to centre
                    // itself under the pointer.
                    moving.current = { table: box.table, dx: at.x - box.x, dy: at.y - box.y };
                    surface.current?.setPointerCapture(e.pointerId);
                  }}
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
      )}
    </div>
  );
}
