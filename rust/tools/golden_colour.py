#!/usr/bin/env python3
"""Colour-path reference outputs for the Rust parity tests.

    golden_colour.py OUT_DIR          (after golden.py OUT_DIR)

Takes a frame-sized crop of OUT_DIR/ribbon.npy and saves the OEM render
(plain, balanced + adjusted, grey), the roll balance, Dmin, the exact
(slide) density and the tables, using the scanner's EEPROM archive.
"""
import json
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO), str(REPO / "tools")]

import pakon_image as pi  # noqa: E402
from web import decode  # noqa: E402

out = Path(sys.argv[1])
rgb = np.load(out / "ribbon.npy")
crop = np.ascontiguousarray(rgb[2000:5000])
negmat, lut, rpd = decode._oem_path()
assert pi._native_lib() is not None, "build/libpakon_render needed"

np.save(out / "lut.npy", lut)
np.save(out / "rpd_table.npy", rpd.table)
np.save(out / "negmat.npy", negmat)
np.save(out / "crop.npy", crop)
np.save(out / "render_plain.npy", pi.render_oem(crop, negmat, lut, rpd))

ds = max(1, int(round(rgb.shape[1] / 500)))
small = np.ascontiguousarray(rgb[::ds, ::ds])
gain, off = pi.oem_roll_balance(small, negmat, lut)
dmin = pi.measure_dmin(rgb)
adj = {"contrast": 75, "red": 25, "blue": -50, "brightness": 10, "sharpness": 133}
np.save(out / "render_adj.npy",
        pi.render_oem(crop, negmat, lut, rpd, balance=(gain, off), adjust=adj))
np.save(out / "render_grey.npy", pi.render_oem(crop, negmat, lut, rpd, grey=True))
np.save(out / "rpd_exact.npy",
        pi.oem_rpd(crop[:300].astype(np.float32) - np.float32(12.5),
                   pi.POS_MATRIX_DEFAULT, lut, exact=True))
(out / "colour.json").write_text(json.dumps({
    "gain": gain.tolist(), "offset": off.tolist(), "dmin": dmin.tolist(),
    "ds": ds, "adjust": adj}, indent=1))
print("ok", crop.shape, gain, off, dmin)

# Slide path: the link on a grid, render_slide with a fixed flare, gaps.
posmat, slut, link, romm = pi.slide_path(None)
g = np.linspace(0.0, 1.0, 23, dtype=np.float32)
grid = np.stack(np.meshgrid(g, g, g, indexing="ij"), -1).reshape(1, -1, 3)
np.save(out / "link_grid.npy", link.apply(grid))
np.save(out / "romm_table.npy", romm.table)
flare = np.array([10.5, 7.25, 3.0], np.float32)
np.save(out / "render_slide.npy", pi.render_slide(crop[:600], posmat, slut, link, romm, flare=flare))
gaps = {str(k): pi.find_slide_gaps(rgb, k=k) for k in (3.0, 6.0, 12.0)}
fl = pi.measure_slide_flare(rgb, gaps=gaps["12.0"]) if gaps["12.0"] else None
(out / "slide.json").write_text(json.dumps({
    "gaps": gaps, "flare12": None if fl is None else fl.tolist()}, indent=1))
print("slide", {k: len(v) for k, v in gaps.items()}, fl)
