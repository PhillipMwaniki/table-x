#!/usr/bin/env python3
"""Generate the source app icon as a 1024x1024 PNG.

Written with only the standard library so the icon can be regenerated on any
machine without pulling in an image toolchain. `pnpm tauri icon` takes the
output of this script and produces every platform-specific size and format.

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


def build_rows():
    """Rasterize the icon into raw RGBA scanlines."""
    # A stylized data table: a header bar plus alternating rows, the classic
    # shape of the result grid the app is built around.
    margin = SIZE * 0.20
    table_w = SIZE - 2 * margin
    header_h = SIZE * 0.105
    row_h = SIZE * 0.088
    gap = SIZE * 0.030
    top = margin + SIZE * 0.045

    bands = [(top, header_h, ACCENT)]
    y = top + header_h + gap
    for i in range(3):
        bands.append((y, row_h, ROW_LIT if i % 2 == 0 else ROW_DIM))
        y += row_h + gap

    rows = bytearray()
    for py in range(SIZE):
        rows.append(0)  # PNG filter type 0 (None)
        yc = py + 0.5
        # Which band, if any, this scanline falls in.
        band = None
        for by, bh, color in bands:
            if by <= yc < by + bh:
                band = (by, bh, color)
                break

        for px in range(SIZE):
            xc = px + 0.5
            a = rounded_alpha(xc, yc)
            if a <= 0.0:
                rows.extend((0, 0, 0, 0))
                continue

            rgb = BG
            if band is not None:
                by, bh, color = band
                if margin <= xc < margin + table_w:
                    # Vertical antialiasing at the band's top and bottom edges.
                    cov = min(yc - by, by + bh - yc, 1.0)
                    # Horizontal antialiasing at the band's left and right edges.
                    cov = min(cov, xc - margin, margin + table_w - xc, 1.0)
                    if cov > 0:
                        rgb = blend(BG, color, max(0.0, min(1.0, cov)))

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
