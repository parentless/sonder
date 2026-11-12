#!/usr/bin/env python3
"""Draws the Sonder ripple icon with a transparent background, no dependencies.

Writes assets/AppIcon.ico (small sizes as BMP, 256 as PNG), assets/sonder-mark.png and, on macOS,
assets/AppIcon.icns. The tile is full-bleed: the rounded square touches every
edge and the corners are transparent. Keep assets/*.svg in step with SHAPES.
"""
import math, os, struct, subprocess, sys, tempfile, zlib

TILE = (0x15, 0x16, 0x19)
INK = (0xF7, 0xF5, 0xEF)
# Everything below is on a 1024 grid.
RADIUS = 232
# A brushed-chrome rim: bright at the top left, a dark band through the middle
# and a second highlight at the bottom right (position 0..1 along the diagonal).
CHROME_WIDTH = 30
CHROME = [(0.0, (252, 253, 255)), (0.22, (160, 166, 175)), (0.45, (74, 78, 86)),
          (0.62, (150, 156, 165)), (0.82, (236, 239, 243)), (1.0, (112, 117, 126))]
RINGS = [(138, 0.85), (235, 0.55), (331, 0.30)]  # (radius, opacity)
RING_WIDTH = 46
DOT = 56


def coverage_pixel(x, y):
    """Returns (r, g, b, a) in 0..1 for a point on the 1024 grid."""
    # Signed distance to the rounded square: negative inside.
    qx = abs(x - 512) - (512 - RADIUS)
    qy = abs(y - 512) - (512 - RADIUS)
    edge = math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - RADIUS
    if edge > 0:
        return (0, 0, 0, 0)
    if -edge < CHROME_WIDTH:
        t = min(max((x + y) / 2048, 0), 1)
        for (a, ca), (b, cb) in zip(CHROME, CHROME[1:]):
            if t <= b:
                f = (t - a) / (b - a)
                return tuple((ca[i] + (cb[i] - ca[i]) * f) / 255 for i in range(3)) + (1.0,)
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
    return rows


def bmp(size, rows):
    """An ICO image entry stored as a 32-bit DIB with an empty AND mask.

    Windows loads small icons most reliably this way; only the 256 px entry
    may be PNG, which matches how the previous icon was built."""
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(
        b"".join(bytes((row[i + 2], row[i + 1], row[i], row[i + 3])) for i in range(0, size * 4, 4))
        for row in reversed(rows)
    )
    mask = bytes(((size + 31) // 32 * 4) * size)
    return header + pixels + mask


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
    def get_rows(size):
        if size not in cache:
            cache[size] = render(size)
        return cache[size]

    def get(size):
        return png(size, get_rows(size))

    sizes = [16, 24, 32, 48, 64, 128, 256]
    out = struct.pack("<HHH", 0, 1, len(sizes)); body = b""; offset = 6 + 16 * len(sizes)
    for s in sizes:
        data = get(s) if s == 256 else bmp(s, get_rows(s))
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
