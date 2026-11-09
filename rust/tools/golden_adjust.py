#!/usr/bin/env python3
"""Saturation / tone reference (pakon_image.adjusted_table, render_slide and
render_oem with adjustments): golden_adjust.py OUT_DIR (after golden_colour.py)."""
import json
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO), str(REPO / "tools")]
import pakon_image as pi  # noqa: E402
from web import decode  # noqa: E402

out = Path(sys.argv[1])
crop = np.load(out / "crop.npy")
negmat, lut, rpd = decode._oem_path()
cases = [(2, "none"), (-3, "none"), (0, "sepia"), (5, "warm"), (-5, "cold")]
for s, t in cases:
    np.save(out / f"adj_rpd_{s}_{t}.npy", pi.adjusted_table(rpd, s, t))
posmat, slut, link, romm = pi.slide_path(None)
adj = {"contrast": -50, "green": 25, "brightness": -20, "sharpness": 67}
flare = np.array([10.5, 7.25, 3.0], np.float32)
np.save(out / "render_slide_adj.npy",
        pi.render_slide(crop[:600], posmat, slut, link, romm, flare=flare, adjust=adj))
adj2 = {"saturation": -3, "tone": "sepia", "red": 25}
np.save(out / "render_oem_sat.npy", pi.render_oem(crop[:600], negmat, lut, rpd, adjust=adj2))
np.save(out / "render_grey_tone.npy", pi.render_oem(crop[:600], negmat, lut, rpd, grey=True,
                                                    adjust={"tone": "warm"}))
(out / "adjust.json").write_text(json.dumps({"cases": cases, "slide": adj, "oem": adj2}))
print("adjust ok")
