#!/usr/bin/env python3
"""End-to-end reference (web/decode.py prescan + export_frames):
golden_roll.py OUT_DIR [RAW] — RAW defaults to OUT_DIR/slice.raw (golden.py
OUT_DIR first). Writes into OUT_DIR/roll-py/ (its own work folder) and
OUT_DIR/roll.json."""
import json
import os
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
os.environ["PAKON_EEPROM_DIR"] = str(REPO / "backups" / "eeprom" / "F135-3054-cycle1")
sys.path[:0] = [str(REPO), str(REPO / "tools")]
import tifffile  # noqa: E402
from web import decode  # noqa: E402

out = Path(sys.argv[1])
work = out / "roll-py"
work.mkdir(exist_ok=True)
decode.WORK_DIR = work
decode.RIBBON_PATH = work / "ribbon.npy"
decode.IR_RIBBON_PATH = work / "ir_ribbon.npy"
decode.PREVIEW_PATH = work / "preview.jpg"

raw = Path(sys.argv[2]) if len(sys.argv) > 2 else out / "slice.raw"
meta = decode.prescan(raw, film="c41")
adj = [None, {"saturation": 2, "red": 25, "sharpness": 67}] + [None] * (len(meta["centres"]) - 2)
frames = decode.export_frames(meta["ribbon"], meta["base"], meta["centres"], frame_w=meta["frame_w"],
                              rpd_balance=meta["rpd_balance"], roll_centres=meta["centres"],
                              film="c41", ice=meta["ice"], sba_dpi=meta["sba_dpi"],
                              scene_dmin=meta["scene_dmin"], adjustments=adj)
for f in frames:
    np.save(work / f"pos_{f['index']:02d}.npy", tifffile.imread(f["tiff"]))
    np.save(work / f"rawt_{f['index']:02d}.npy", tifffile.imread(f["raw_tiff"]))
grey = decode.export_frames(meta["ribbon"], meta["base"], meta["centres"][:2], frame_w=meta["frame_w"],
                            rpd_balance=meta["rpd_balance"], film="bw", ice=False)
for f in grey:
    np.save(work / f"grey_{f['index']:02d}.npy", tifffile.imread(f["tiff"]))
(out / "roll.json").write_text(json.dumps({"raw": str(raw), "meta": meta, "adjust": adj, "n": len(frames)}, indent=1))
print("roll", {k: v for k, v in meta.items() if k not in ("centres",)}, len(frames))
