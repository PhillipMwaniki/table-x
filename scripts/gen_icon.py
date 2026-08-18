#!/usr/bin/env python3
"""Generate the source app icon as a 1024x1024 PNG.

Written with only the standard library so the icon can be regenerated on any
machine without pulling in an image toolchain. `pnpm tauri icon` takes the
output of this script and produces every platform-specific size and format.

The mark is a monospace `[ ]` around a 2x2 grid whose diagonal is lit: a table
that has rows and columns, an X implied by the diagonal, and a nod to `tablex`,
the CLI that ships the same drivers without a window. It replaces a header bar
over three full-width rows, which had no vertical division anywhere and so read
as a hamburger menu rather than as a table.

Shapes are drawn from signed distances rather than scanline bands, because
brackets have round caps and joins that a band cannot express. Coordinates
below are in DESIGN UNITS on a 0-100 grid, matching the SVG the mark was
designed in, and are scaled to pixels once.

Usage:  python scripts/gen_icon.py [out.png]
"""

import struct
import sys
import zlib

SIZE = 1024

# Matches --color-accent / --color-surface-1 from src/styles.css.
BG = (0x1A, 0x1D, 0x24)
ACCENT = (0x4D, 0x8D, 0xF5)
ROW_LIT = (0xE8, 0xEC, 0xF2)
ROW_DIM = (0x6B, 0x74, 0x86)

CORNER_RADIUS = SIZE * 0.22

# Design units -> pixels.
U = SIZE / 100.0

# The brackets, as stroked polylines with round caps and joins. Drawing each
# leg as a capsule and taking the union gives the joins for free.
BRACKET_STROKE = 7.0
BRACKETS = [
    ((36, 22), (22, 22)),
    ((22, 22), (22, 78)),
    ((22, 78), (36, 78)),
    ((64, 22), (78, 22)),
    ((78, 22), (78, 78)),
    ((78, 78), (64, 78)),
]

# The 2x2 grid between them. The lit diagonal is what carries the X.
CELL = 13.0
CELL_RADIUS = 2.0
CELLS = [
    (35, 35, ACCENT, 1.0),
    (52, 35, ROW_DIM, 0.45),
    (35, 52, ROW_DIM, 0.45),
    (52, 52, ACCENT, 1.0),
]


def rounded_alpha(x: float, y: float) -> float:
    """Coverage of the rounded-square mask at a point, antialiased at the edge."""
    r = CORNER_RADIUS
    # Distance outside the rounded rect (0 when inside).
    dx = max(r - x, x - (SIZE - r), 0.0)
    dy = max(r - y, y - (SIZE - r), 0.0)
    dist = (dx * dx + dy * dy) ** 0.5
    if dx == 0.0 or dy == 0.0:
        # Straight edges: fully inside within the bounds.
        return 1.0 if 0 <= x < SIZE and 0 <= y < SIZE else 0.0
    return max(0.0, min(1.0, r - dist + 0.5))


def blend(dst, src, a):
    return tuple(int(round(d + (s - d) * a)) for d, s in zip(dst, src))


def coverage(distance: float) -> float:
    """Antialiased coverage from a signed distance in pixels, negative inside."""
    return max(0.0, min(1.0, 0.5 - distance))


def capsule_distance(px, py, x1, y1, x2, y2, half):
    """Signed distance to a round-capped line segment, all in pixels."""
    dx, dy = x2 - x1, y2 - y1
    length2 = dx * dx + dy * dy
    if length2 == 0.0:
        t = 0.0
    else:
        t = ((px - x1) * dx + (py - y1) * dy) / length2
        t = max(0.0, min(1.0, t))
    cx, cy = x1 + t * dx, y1 + t * dy
    return ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5 - half


def rrect_distance(px, py, x0, y0, w, h, r):
    """Signed distance to a rounded rectangle, all in pixels."""
    # Nearest point on the inner rectangle the corner radius is swept around.
    cx = min(max(px, x0 + r), x0 + w - r)
    cy = min(max(py, y0 + r), y0 + h - r)
    return ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5 - r


def build_rows():
    """Rasterize the icon into raw RGBA scanlines."""
    half = BRACKET_STROKE * U / 2.0
    # Everything in pixels once, rather than per pixel.
    brackets = [
        (x1 * U, y1 * U, x2 * U, y2 * U) for (x1, y1), (x2, y2) in BRACKETS
    ]
    cells = [(x * U, y * U, CELL * U, CELL_RADIUS * U, c, o) for x, y, c, o in CELLS]

    rows = bytearray()
    for py in range(SIZE):
        rows.append(0)  # PNG filter type 0 (None)
        yc = py + 0.5

        # Only the shapes this scanline can touch, so the inner loop stays
        # short: a pure-Python rasterizer cannot afford every shape per pixel.
        near_brackets = [
            b for b in brackets if min(b[1], b[3]) - half - 1 <= yc <= max(b[1], b[3]) + half + 1
        ]
        near_cells = [c for c in cells if c[1] - 1 <= yc <= c[1] + c[2] + 1]

        for px in range(SIZE):
            xc = px + 0.5
            a = rounded_alpha(xc, yc)
            if a <= 0.0:
                rows.extend((0, 0, 0, 0))
                continue

            rgb = BG

            for x0, y0, size, radius, color, opacity in near_cells:
                cov = coverage(rrect_distance(xc, yc, x0, y0, size, size, radius))
                if cov > 0.0:
                    rgb = blend(rgb, color, cov * opacity)

            # Union of the legs, so overlapping caps do not darken the joint.
            bracket_cov = 0.0
            for x1, y1, x2, y2 in near_brackets:
                cov = coverage(capsule_distance(xc, yc, x1, y1, x2, y2, half))
                if cov > bracket_cov:
                    bracket_cov = cov
                    if bracket_cov >= 1.0:
                        break
            if bracket_cov > 0.0:
                rgb = blend(rgb, ROW_LIT, bracket_cov)

            rows.extend((rgb[0], rgb[1], rgb[2], int(round(a * 255))))
    return bytes(rows)


def chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "app-icon.png"
    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)  # 8-bit RGBA
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(build_rows(), 9))
        + chunk(b"IEND", b"")
    )
    with open(out, "wb") as fh:
        fh.write(png)
    print(f"wrote {out} ({len(png):,} bytes, {SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
