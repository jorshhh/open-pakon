#!/usr/bin/env python3
"""OEM Digital ICE reference (tools/oem_dice.py):
golden_dice.py OUT_DIR (after golden.py and golden_ice.py OUT_DIR)."""
import json
import sys
import time
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "tools")]
import oem_dice  # noqa: E402

out = Path(sys.argv[1])
rgb = np.load(out / "ribbon.npy", mmap_mode="r")
ir = np.load(out / "ir_plane.npy", mmap_mode="r")
part, irc = np.ascontiguousarray(rgb[2000:5000]), np.ascontiguousarray(ir[2000:5000])
t0 = time.perf_counter()
d = oem_dice.Dice()
t1 = time.perf_counter()
cleaned, mask = oem_dice.clean_frame(d, part, irc)
t2 = time.perf_counter()
_, counts, _ = d.process(oem_dice.to14(part), oem_dice.to14(irc.astype(np.float32) + 300.0))
np.save(out / "dice_out.npy", cleaned)
np.save(out / "dice_mask.npy", mask)
(out / "dice.json").write_text(json.dumps({"rebuilt": int(mask.sum()), "counts": list(counts)}))
print(f"dice: init {t1 - t0:.2f}s, frame {t2 - t1:.2f}s, {int(mask.sum())} rebuilt, counts {counts}")
