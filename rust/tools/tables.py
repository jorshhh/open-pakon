#!/usr/bin/env python3
"""Write the rpd.pf and romm.pf sRGB tables the Rust pipeline embeds.

pakon_image.RpdLut evaluates a profile -> sRGB LittleCMS transform (Pillow's
ImageCms, 8-bit) on 52^3 nodes. The Rust port does not link LittleCMS: it
ships these 8-bit results (rust/crates/pakon-image/assets/*.srgb52.u8, C
order, n^3 x 3 bytes) and divides by 255 as RpdLut does, so its tables are
identical. Rerun when profiles/ changes:

    .venv/bin/python rust/tools/tables.py
"""
from pathlib import Path

import numpy as np
from PIL import Image, ImageCms

REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "rust/crates/pakon-image/assets"


def table_u8(profile):
    nodes = np.arange(0, 256, 5, dtype=np.uint8)
    grid = np.stack(np.meshgrid(nodes, nodes, nodes, indexing="ij"), -1)
    img = Image.fromarray(grid.reshape(1, -1, 3), "RGB")
    tr = ImageCms.buildTransform(ImageCms.getOpenProfile(str(profile)),
                                 ImageCms.createProfile("sRGB"), "RGB", "RGB",
                                 renderingIntent=0)
    return np.asarray(ImageCms.applyTransform(img, tr), dtype=np.uint8).reshape(-1)


for name in ("rpd", "romm"):
    t = table_u8(REPO / "profiles" / f"{name}.pf")
    assert t.size == 52 ** 3 * 3
    (OUT / f"{name}.srgb52.u8").write_bytes(t.tobytes())
    print(name, t.size)
