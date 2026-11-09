#!/usr/bin/env python3
"""No-EEPROM fallback reference (pakon_image.invert_c41, render_jpeg):
golden_fallback.py OUT_DIR (after golden_colour.py)."""
import json
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO), str(REPO / "tools")]
import pakon_image as pi  # noqa: E402

out = Path(sys.argv[1])
crop = np.load(out / "crop.npy")[:800]
base = np.asarray(json.loads((out / "colour.json").read_text())["dmin"], np.float32)
pos = pi.invert_c41(crop, base, pi._c41_lut())
np.save(out / "invert_c41.npy", pos)
np.save(out / "render_jpeg.npy", pi.render_jpeg(pos, str(REPO / "profiles" / "rpd.pf")))
print("fallback ok")
