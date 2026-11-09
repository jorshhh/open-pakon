#!/usr/bin/env python3
"""Dust-removal reference (our ICE port, tools/pakon_ice.py):
golden_ice.py OUT_DIR (after golden.py OUT_DIR)."""
import json
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO), str(REPO / "tools")]
import pakon_ice as ice  # noqa: E402
from web import decode  # noqa: E402

out = Path(sys.argv[1])
rgb, ic = decode._build_ribbon(out / "slice.raw", lambda *_: None)
ir, flat = ic
np.save(out / "ir_plane.npy", ir)
cols = rgb.shape[1]
noise = ice.ir_noise_factor(flat, cols)
part, irc = np.ascontiguousarray(rgb[2000:5000]), np.ascontiguousarray(ir[2000:5000])
tail = ice.ir_tail_factor(irc.astype(np.float32))
f = np.clip(part.astype(np.float32) - 300.0, 0, ice.M)
cal = ice.calibrate(ice.reduce(f[..., 0]), ice.reduce(irc.astype(np.float32)))
cleaned, mask = ice.clean_frame(part, irc, noise, ice.pakon_dpi(cols))
np.save(out / "ice_noise.npy", noise)
np.save(out / "ice_tail.npy", tail)
np.save(out / "ice_out.npy", cleaned)
np.save(out / "ice_mask.npy", mask)
(out / "ice.json").write_text(json.dumps({"cal": list(cal), "rebuilt": int(mask.sum())}))
print("ice", cal, int(mask.sum()))
