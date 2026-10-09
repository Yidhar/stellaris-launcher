"""Draws the program icon (the title bar's mark in white on a rounded blue square, the same drawing as theme::icon) at the sizes Windows uses
and writes crates/stl-gui/assets/stellaris-launcher.ico, which build.rs puts into the exes. Run again after changing the drawing:

    python tools/make_icon.py
"""
import math
import os

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'crates', 'stl-gui', 'assets', 'stellaris-launcher.ico')
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
SS = 4  # samples per pixel along each axis, for smooth edges
MARK = 0.64  # the mark's radius, as a part of half the icon's width (theme::MARK_IN_ICON)

# the mark (theme::mark_points): a four-pointed star, tips at 1 and the waist at 3.2/9, from the right-hand tip; the title bar fills it as a
# fan of triangles from that first point, which makes the concave star a ray (theme::in_mark)
V = [(math.cos(k * math.tau / 16) * (1.0 if k % 4 == 0 else 3.2 / 9), math.sin(k * math.tau / 16) * (1.0 if k % 4 == 0 else 3.2 / 9)) for k in range(16)]


def in_mark(x, y):
    def side(p, a, b):
        return (p[0] - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (p[1] - b[1])
    p = (x, y)
    for i in range(1, 15):
        d1, d2, d3 = side(p, V[0], V[i]), side(p, V[i], V[i + 1]), side(p, V[i + 1], V[0])
        if not ((d1 < 0 or d2 < 0 or d3 < 0) and (d1 > 0 or d2 > 0 or d3 > 0)):
            return True
    return False


def pixel(fx, fy):
    """The colour at a point of the square [-1, 1]², or None outside the rounded square."""
    qx, qy = abs(fx) - 0.78, abs(fy) - 0.78
    outside = (max(qx, 0) ** 2 + max(qy, 0) ** 2) ** 0.5 + min(max(qx, qy), 0) - 0.2
    if outside > 0:
        return None
    t = (fy + 1) / 2
    r, g, b = 70 + (40 - 70) * t, 120 + (70 - 120) * t, 255 + (190 - 255) * t
    if in_mark(fx / MARK, fy / MARK):
        r, g, b = 255, 255, 255
    return r, g, b


def draw(n):
    img = Image.new('RGBA', (n, n))
    px = img.load()
    for y in range(n):
        for x in range(n):
            acc = [0.0, 0.0, 0.0]
            hits = 0
            for sy in range(SS):
                for sx in range(SS):
                    fx = (x + (sx + 0.5) / SS) / n * 2 - 1
                    fy = (y + (sy + 0.5) / SS) / n * 2 - 1
                    c = pixel(fx, fy)
                    if c:
                        hits += 1
                        for i in range(3):
                            acc[i] += c[i]
            if hits:
                px[x, y] = (round(acc[0] / hits), round(acc[1] / hits), round(acc[2] / hits), round(255 * hits / SS / SS))
    return img


os.makedirs(os.path.dirname(OUT), exist_ok=True)
big = draw(256)
big.save(OUT, format='ICO', sizes=[(s, s) for s in SIZES], append_images=[draw(s) for s in SIZES if s != 256])
print('wrote', OUT, os.path.getsize(OUT), 'bytes')
