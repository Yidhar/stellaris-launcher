"""Draws the program icon — the title bar's mark in white on a rounded blue square — and writes it twice:

- crates/stl-gui/assets/stellaris-launcher.ico, at the sizes Windows uses, which build.rs puts into the exes;
- crates/stl-gui/assets/window-icon-64.rgba, raw 64x64 RGBA, the window's icon (theme::icon).

The mark is drawn from crates/stl-gui/assets/mark-mesh.json: the triangles egui makes of the title bar's mark, with their soft edge (written
by the stl-gui test `dump_the_mark`). egui fills the concave star as a fan from its right-hand tip; the fan overlaps itself, and as the
mark is white at 225/255 the overlaps are brighter: a ray's body on a dimmer star. The icons paint the same layers, so they match it.

    STL_DUMP_MARK=crates/stl-gui/assets/mark-mesh.json cargo test -p stl-gui dump_the_mark -- --ignored   (after changing the mark)
    python tools/make_icon.py
"""
import json
import os

import numpy as np
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, 'crates', 'stl-gui', 'assets')
OUT = os.path.join(ASSETS, 'stellaris-launcher.ico')
WINDOW = os.path.join(ASSETS, 'window-icon-64.rgba')
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
SS = 4  # samples per pixel along each axis, for smooth edges
MARK = 0.64  # the mark's radius, as a part of half the icon's width
WHITE = 225 / 255  # the title bar paints the mark in white at this opacity

MESH = json.load(open(os.path.join(ASSETS, 'mark-mesh.json'), encoding='utf-8'))
VERTS = np.array(MESH['vertices'], dtype=np.float64)
TRIS = np.array(MESH['indices'], dtype=np.int64).reshape(-1, 3)


def layers(x, y):
    """How many layers of the mark are over each point (x, y in units of its radius, y down), as the title bar paints it: each fill
    triangle is one (the fan of a concave star overlaps itself: those parts get two), and the soft edge one where it is at least half."""
    px, py = x * MESH['radius'], y * MESH['radius']
    fills = np.zeros_like(px)
    edge = np.zeros(px.shape, dtype=bool)
    for a, b, c in TRIS:
        (ax, ay, aa), (bx, by, ba), (cx, cy, ca) = VERTS[a], VERTS[b], VERTS[c]
        det = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy)
        if abs(det) < 1e-12:
            continue
        l1 = ((by - cy) * (px - cx) + (cx - bx) * (py - cy)) / det
        l2 = ((cy - ay) * (px - cx) + (ax - cx) * (py - cy)) / det
        l3 = 1.0 - l1 - l2
        inside = (l1 >= 0) & (l2 >= 0) & (l3 >= 0)
        if aa == ba == ca == 1:
            fills += inside
        else:
            edge |= inside & (l1 * aa + l2 * ba + l3 * ca >= 0.5)
    return np.where(fills > 0, fills, edge.astype(np.float64))


def draw(n):
    s = n * SS
    f = (np.arange(s) + 0.5) / s * 2 - 1
    fx, fy = np.meshgrid(f, f)
    # the rounded square, its blue darkening downwards
    qx, qy = np.abs(fx) - 0.78, np.abs(fy) - 0.78
    outside = np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - 0.2
    tile = outside <= 0
    t = (fy + 1) / 2
    rgb = np.stack([70 + (40 - 70) * t, 120 + (70 - 120) * t, 255 + (190 - 255) * t], axis=-1)
    # the mark: white at the title bar's 225/255, once per layer (where the fan overlaps itself it is brighter: the ray's body)
    k = layers(fx / MARK, fy / MARK)[..., None]
    rgb = 255.0 - (255.0 - rgb) * (1.0 - WHITE) ** k
    # each pixel: the mean of its samples
    rgb = (rgb * tile[..., None]).reshape(n, SS, n, SS, 3).sum(axis=(1, 3))
    hits = tile.reshape(n, SS, n, SS).sum(axis=(1, 3))
    out = np.zeros((n, n, 4), dtype=np.uint8)
    seen = hits > 0
    out[seen, :3] = np.round(rgb[seen] / hits[seen][:, None]).astype(np.uint8)
    out[..., 3] = np.round(255 * hits / (SS * SS)).astype(np.uint8)
    return Image.fromarray(out)


os.makedirs(ASSETS, exist_ok=True)
images = {n: draw(n) for n in SIZES}
images[256].save(OUT, format='ICO', sizes=[(n, n) for n in SIZES], append_images=[images[n] for n in SIZES if n != 256])
open(WINDOW, 'wb').write(images[64].tobytes())
print('wrote', OUT, os.path.getsize(OUT), 'bytes and', WINDOW)
