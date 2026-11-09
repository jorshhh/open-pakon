#!/usr/bin/env python3
"""OEM scene balance reference (tools/oem_sba.py) on the full roll:
golden_sba.py FULL_DIR GOLDEN_DIR (FULL_DIR holds ribbon.npy + framing.json,
GOLDEN_DIR negmat.npy + lut.npy)."""
import json
import sys
import time
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "tools")]
import oem_sba  # noqa: E402
import pakon_image as pi  # noqa: E402

full, gold = Path(sys.argv[1]), Path(sys.argv[2])
rgb = np.load(full / "ribbon.npy", mmap_mode="r")
starts, length, _ = json.loads((full / "framing.json").read_text())["full-0"]
negmat, lut = np.load(gold / "negmat.npy"), np.load(gold / "lut.npy")
crops = [rgb[s:s + length] for s in starts]
t0 = time.perf_counter()
imgs = [oem_sba.analysis_image(c, negmat, lut, pi.oem_rpd) for c in crops]
t1 = time.perf_counter()
rows_per_mm = pi.tlx_geometry(rgb.shape[1], "full")[1] / 36.0
base = oem_sba.film_base(rgb, int(starts[0]), rows_per_mm)
dmin = oem_sba.film_base_rpd(base, negmat, lut, pi.oem_rpd) if base is not None else None
dpi = oem_sba.dpi_for(96, 14)
t2 = time.perf_counter()
shifts = oem_sba.roll_shifts(imgs[:5], dpi=dpi, roll=imgs, dmin=dmin)
t3 = time.perf_counter()
solo = oem_sba.roll_shifts(imgs[:3], dpi=dpi)
np.save(gold / "sba_analysis.npy", np.stack(imgs))
(gold / "sba.json").write_text(json.dumps({
    "starts": starts, "length": length, "rows_per_mm": rows_per_mm,
    "base": base, "dmin": dmin, "dpi": dpi,
    "dpi_none": oem_sba.dpi_for(), "dpi_78_13": oem_sba.dpi_for(78, 13), "dpi_43": oem_sba.dpi_for(43, 2),
    "shifts": shifts.tolist(), "solo": solo.tolist()}))
print(f"sba: {len(crops)} frames, analysis {t1 - t0:.2f}s, roll {t3 - t2:.2f}s; base {base} dmin {dmin} dpi {dpi}")
print(shifts, solo)
