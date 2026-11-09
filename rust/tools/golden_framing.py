#!/usr/bin/env python3
"""Framing reference: golden_framing.py RIBBON.npy OUT.json — the TLX block
profile and frame grid for every format, negative and positive."""
import json
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tools"))
import pakon_image as pi  # noqa: E402

rgb = np.load(sys.argv[1], mmap_mode="r")
out = {"profile": pi.tlx_profile(rgb, 8).tolist()}
for fmt in pi.FRAME_FORMATS:
    for pos in (False, True):
        s, l, p = pi.tlx_frame_grid(rgb, positive=pos, fmt=fmt)
        out[f"{fmt}-{int(pos)}"] = [s, l, p]
Path(sys.argv[2]).write_text(json.dumps(out))
print({k: (len(v[0]), v[1], v[2]) for k, v in out.items() if k != "profile"})
