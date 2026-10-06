"""Draws the program icon (a white four-pointed star on a rounded blue square, the same shape as theme::icon) at the sizes Windows uses
and writes crates/stl-gui/assets/stellaris-launcher.ico, which build.rs puts into the exes. Run again after changing the drawing:

    python tools/make_icon.py
"""
import os

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'crates', 'stl-gui', 'assets', 'stellaris-launcher.ico')
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
SS = 4  # samples per pixel along each axis, for smooth edges


def pixel(fx, fy):
    """The colour at a point of the square [-1, 1]², or None outside the rounded square."""
    qx, qy = abs(fx) - 0.78, abs(fy) - 0.78
    outside = (max(qx, 0) ** 2 + max(qy, 0) ** 2) ** 0.5 + min(max(qx, qy), 0) - 0.2
    if outside > 0:
        return None
    t = (fy + 1) / 2
    r, g, b = 70 + (40 - 70) * t, 120 + (70 - 120) * t, 255 + (190 - 255) * t
    if abs(fx) ** (2 / 3) + abs(fy) ** (2 / 3) <= 0.62 ** (2 / 3):
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
