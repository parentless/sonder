#!/usr/bin/env python3
"""Draws the Sonder ripple icon with a transparent background, no dependencies.

Writes assets/AppIcon.ico, assets/sonder-mark.png and, on macOS,
assets/AppIcon.icns. The tile is full-bleed: the rounded square touches every
edge and the corners are transparent. Keep assets/*.svg in step with SHAPES.
"""
import math, os, struct, subprocess, sys, tempfile, zlib

TILE = (0x26, 0x27, 0x2B)
INK = (0xF7, 0xF5, 0xEF)
# Everything below is on a 1024 grid.
RADIUS = 232
RINGS = [(138, 0.85), (235, 0.55), (331, 0.30)]  # (radius, opacity)
RING_WIDTH = 46
DOT = 56


def coverage_pixel(x, y):
    """Returns (r, g, b, a) in 0..1 for a point on the 1024 grid."""
    # Rounded square.
    dx = max(RADIUS - x, 0, x - (1024 - RADIUS))
    dy = max(RADIUS - y, 0, y - (1024 - RADIUS))
    if x < 0 or y < 0 or x > 1024 or y > 1024 or dx * dx + dy * dy > RADIUS * RADIUS:
        return (0, 0, 0, 0)
    colour = list(TILE)
    d = math.hypot(x - 512, y - 512)
    ink = 0.0
    if d <= DOT:
        ink = 1.0
    else:
        for radius, opacity in RINGS:
            if abs(d - radius) <= RING_WIDTH / 2:
                ink = opacity
    return tuple((TILE[i] * (1 - ink) + INK[i] * ink) / 255 for i in range(3)) + (1.0,)


def render(size):
    ss = 4 if size <= 256 else 2
    scale = 1024 / size
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            r = g = b = a = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    pr, pg, pb, pa = coverage_pixel(
                        (px + (sx + 0.5) / ss) * scale, (py + (sy + 0.5) / ss) * scale
                    )
                    r += pr * pa; g += pg * pa; b += pb * pa; a += pa
            n = ss * ss
            if a:
                row += bytes((round(r / a * 255), round(g / a * 255), round(b / a * 255), round(a / n * 255)))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return png(size, rows)


def png(size, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    raw = b"".join(b"\0" + row for row in rows)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    assets = os.path.join(root, "assets")
    cache = {}
    def get(size):
        if size not in cache:
            cache[size] = render(size)
        return cache[size]

    sizes = [16, 24, 32, 48, 64, 128, 256]
    out = struct.pack("<HHH", 0, 1, len(sizes)); body = b""; offset = 6 + 16 * len(sizes)
    for s in sizes:
        data = get(s)
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset + len(body))
        body += data
    open(os.path.join(assets, "AppIcon.ico"), "wb").write(out + body)
    open(os.path.join(assets, "sonder-mark.png"), "wb").write(get(128))

    if sys.platform == "darwin":
        with tempfile.TemporaryDirectory() as work:
            iconset = os.path.join(work, "AppIcon.iconset"); os.mkdir(iconset)
            for name, s in [("16x16", 16), ("16x16@2x", 32), ("32x32", 32), ("32x32@2x", 64),
                            ("128x128", 128), ("128x128@2x", 256), ("256x256", 256),
                            ("256x256@2x", 512), ("512x512", 512), ("512x512@2x", 1024)]:
                open(os.path.join(iconset, f"icon_{name}.png"), "wb").write(get(s))
            subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(assets, "AppIcon.icns")], check=True)


main()
