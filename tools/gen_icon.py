#!/usr/bin/python3 -I
"""Draws the Areko GPUPinWatch icon (12V-2x6 plug, one hot pin) and writes PNGs + a multi-size .ico.

Usage: /usr/bin/python3 -I tools/gen_icon.py
Writes assets/icon/icon-<size>.png and assets/icon/icon.ico (PNG-compressed entries).
Small sizes are drawn with fewer details so they stay readable in the taskbar.
"""
import io
import math
import os
import struct

import cairo

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'assets', 'icon')
SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
BG0, BG1 = (0.039, 0.055, 0.102), (0.024, 0.031, 0.059)
CY, MG, TEAL = (0.020, 0.851, 0.910), (1.0, 0.165, 0.427), (0.027, 0.573, 0.627)


def rrect(c, x, y, w, h, r):
    c.new_sub_path()
    c.arc(x + w - r, y + r, r, -math.pi / 2, 0)
    c.arc(x + w - r, y + h - r, r, 0, math.pi / 2)
    c.arc(x + r, y + h - r, r, math.pi / 2, math.pi)
    c.arc(x + r, y + r, r, math.pi, 1.5 * math.pi)
    c.close_path()


def glow_circle(c, x, y, r, col, s, fill=True):
    for k, a in ((2.4, 0.10), (1.7, 0.18)):        # soft glow rings
        c.set_source_rgba(*col, a)
        c.arc(x, y, r * k, 0, 2 * math.pi)
        c.fill()
    c.set_source_rgba(*col, 1)
    c.arc(x, y, r, 0, 2 * math.pi)
    c.fill() if fill else c.stroke()


def draw(n):
    s = cairo.ImageSurface(cairo.FORMAT_ARGB32, n, n)
    c = cairo.Context(s)
    small = n <= 32
    u = n / 256.0                                  # design unit: 256 grid
    # background tile
    g = cairo.LinearGradient(0, 0, 0, n)
    g.add_color_stop_rgb(0, *BG0)
    g.add_color_stop_rgb(1, *BG1)
    rrect(c, 4 * u, 4 * u, 248 * u, 248 * u, 52 * u)
    c.set_source(g)
    c.fill_preserve()
    c.set_source_rgba(*CY, 0.45)
    c.set_line_width(max(1.0, 3 * u))
    c.stroke()
    if not small:                                  # Areko corner brackets: cyan top left, magenta bottom right
        c.set_line_width(7 * u)
        c.set_line_cap(cairo.LINE_CAP_SQUARE)
        c.set_source_rgba(*CY, 1)
        c.move_to(30 * u, 74 * u); c.line_to(30 * u, 30 * u); c.line_to(74 * u, 30 * u); c.stroke()
        c.set_source_rgba(*MG, 1)
        c.move_to(226 * u, 182 * u); c.line_to(226 * u, 226 * u); c.line_to(182 * u, 226 * u); c.stroke()
    # plug housing with latch
    if small:                                      # taskbar sizes: plug fills the tile, bigger pins
        hx, hy, hw, hh = 18 * u, 84 * u, 220 * u, 104 * u
    else:
        hx, hy, hw, hh = 40 * u, 92 * u, 176 * u, 92 * u
    c.set_line_width(max(1.2, 6 * u))
    c.set_source_rgba(*CY, 1)
    rrect(c, hx, hy, hw, hh, 14 * u)
    c.stroke()
    c.set_line_width(max(1.0, 5 * u))
    rrect(c, 98 * u, hy - (26 if small else 20) * u, 60 * u, (26 if small else 20) * u, 5 * u)
    c.stroke()
    # pins: 6 x 12 V on top (pin 4 = the hot one), 6 x GND below
    pitch = (34 if small else 26) * u
    x0 = 128 * u - 2.5 * pitch
    if small:
        r = 13 * u
        y = hy + hh / 2
        for i in range(6):
            col = MG if i == 3 else CY
            c.set_source_rgba(*col, 1)
            c.arc(x0 + i * pitch, y, r, 0, 2 * math.pi)
            c.fill()
    else:
        r = 9 * u
        yt, yb = hy + 28 * u, hy + 64 * u
        for i in range(6):
            x = x0 + i * pitch
            glow_circle(c, x, yt, r, MG if i == 3 else CY, s)
            c.set_source_rgba(*TEAL, 0.9)
            c.set_line_width(3 * u)
            c.arc(x, yb, r * 0.85, 0, 2 * math.pi)
            c.stroke()
    return s


def main():
    os.makedirs(OUT, exist_ok=True)
    pngs = {}
    for n in SIZES:
        surf = draw(n)
        surf.write_to_png(os.path.join(OUT, 'icon-%d.png' % n))
        buf = io.BytesIO()
        surf.write_to_png(buf)
        pngs[n] = buf.getvalue()
    # ICO with PNG entries (Vista+): header, directory, data
    ico_sizes = [n for n in SIZES if n <= 256]
    head = struct.pack('<HHH', 0, 1, len(ico_sizes))
    off = 6 + 16 * len(ico_sizes)
    dirs, data = b'', b''
    for n in ico_sizes:
        d = pngs[n]
        dirs += struct.pack('<BBBBHHII', n % 256, n % 256, 0, 0, 1, 32, len(d), off + len(data))
        data += d
    with open(os.path.join(OUT, 'icon.ico'), 'wb') as f:
        f.write(head + dirs + data)
    print('icon: %s (%s)' % (OUT, ', '.join(map(str, SIZES))))


if __name__ == '__main__':
    main()
