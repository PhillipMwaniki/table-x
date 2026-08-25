import { describe, expect, it } from "vitest";
import { canvasPoint, clampScale, linkPath, loopPath } from "./erd";
import type { DiagramBox } from "./types";

function box(overrides: Partial<DiagramBox> = {}): DiagramBox {
  return {
    schema: null,
    table: "users",
    columns: [],
    x: 0,
    y: 0,
    width: 100,
    height: 60,
    ...overrides,
  };
}

describe("canvasPoint", () => {
  const surface = { left: 40, top: 20 } as DOMRect;

  it("undoes the pan", () => {
    const point = canvasPoint({ x: 140, y: 120 }, surface, { x: 10, y: 5, scale: 1 });
    expect(point).toEqual({ x: 90, y: 95 });
  });

  it("undoes the zoom as well", () => {
    // The failure this prevents is not subtle: forget the scale and the box
    // leaps away from the pointer the moment the zoom is not 100%.
    const point = canvasPoint({ x: 140, y: 120 }, surface, { x: 0, y: 0, scale: 2 });
    expect(point).toEqual({ x: 50, y: 50 });
  });

  it("is the identity at the origin, unpanned and unzoomed", () => {
    expect(canvasPoint({ x: 40, y: 20 }, surface, { x: 0, y: 0, scale: 1 })).toEqual({
      x: 0,
      y: 0,
    });
  });
});

describe("clampScale", () => {
  it("keeps a zoom inside what the canvas can draw", () => {
    expect(clampScale(1)).toBe(1);
    expect(clampScale(50)).toBe(2.5);
    expect(clampScale(0.001)).toBe(0.2);
  });
});

describe("linkPath", () => {
  it("leaves the bottom of the child and arrives at the top of the parent", () => {
    const child = box({ x: 0, y: 200, width: 100, height: 60 });
    const parent = box({ table: "orders", x: 200, y: 0, width: 100, height: 60 });
    // Starts at the child's bottom edge centre, ends at the parent's top edge
    // centre. Read the other way round, the diagram would claim the parent
    // points at the child.
    expect(linkPath(child, parent)).toMatch(/^M 50 260 C /);
    expect(linkPath(child, parent)).toMatch(/, 250 0$/);
  });
});

describe("loopPath", () => {
  it("leaves and returns to the same box", () => {
    // A table that references itself has no second point to draw to, so the
    // line has to be a shape rather than a segment.
    const path = loopPath(box({ x: 10, y: 10 }));
    expect(path).toMatch(/^M 110 32 /);
    expect(path).toMatch(/110 48$/);
  });
});
