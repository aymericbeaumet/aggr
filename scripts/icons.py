#!/usr/bin/env python3
"""Generate the aggr PWA icons: a teal tile with a white RSS glyph. Stdlib only.

usage: scripts/icons.py themes/default/static
"""
import math
import struct
import sys
import zlib

TEAL = (0x0F, 0x76, 0x6E)
WHITE = (0xFF, 0xFF, 0xFF)


def png(size, pixels):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(row) for row in pixels)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def sd_round_rect(x, y, half, r):
    qx, qy = abs(x) - half + r, abs(y) - half + r
    return math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - r


def sd_circle(x, y, cx, cy, r):
    return math.hypot(x - cx, y - cy) - r


def sd_arc(x, y, ox, oy, radius, width):
    """Quarter arc centred on (ox, oy), sweeping right and up, with round caps."""
    dx, dy = x - ox, y - oy
    if dx >= 0 and dy <= 0:
        return abs(math.hypot(dx, dy) - radius) - width / 2
    return min(math.hypot(dx - radius, dy), math.hypot(dx, dy + radius)) - width / 2


def coverage(d):
    return min(1.0, max(0.0, 0.5 - d))


def icon(size, rounded, glyph_scale):
    """`rounded`: transparent corners (a launcher tile); otherwise an opaque square."""
    half = size / 2
    corner = size * 0.22
    # The glyph lives in a box of side `g`, centred; the arc origin is its bottom-left.
    g = size * glyph_scale
    ox, oy = half - g * 0.42, half + g * 0.42
    dot_r = g * 0.11
    stroke = g * 0.17
    arcs = (g * 0.40, g * 0.70)
    rows = []
    for py in range(size):
        row = bytearray()
        y = py + 0.5
        for px in range(size):
            x = px + 0.5
            bg = coverage(sd_round_rect(x - half, y - half, half, corner)) if rounded else 1.0
            d = sd_circle(x, y, ox, oy, dot_r)
            for radius in arcs:
                d = min(d, sd_arc(x, y, ox, oy, radius, stroke))
            fg = coverage(d)
            row.extend(round(t + (w - t) * fg) for t, w in zip(TEAL, WHITE))
            row.append(round(255 * bg))
        rows.append(row)
    return png(size, rows)


def main(out):
    files = {
        "icon-192.png": icon(192, True, 0.62),
        "icon-512.png": icon(512, True, 0.62),
        # Maskable: the launcher crops to a circle, so keep the glyph inside the 80% safe zone.
        "icon-maskable-512.png": icon(512, False, 0.48),
        # iOS rounds the corners itself.
        "apple-touch-icon.png": icon(180, False, 0.58),
    }
    for name, data in files.items():
        with open(f"{out}/{name}", "wb") as f:
            f.write(data)
        print(f"{name}\t{len(data)} bytes")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else ".")
