#!/usr/bin/env python3
"""Reference outputs from the Python pipeline for the Rust parity tests.

    golden.py RAW OUT_DIR [--rows A:B]

Cuts rows A:B of RAW (8000-sample rows by default; the slice keeps the raw's
.flat.json) into OUT_DIR/slice.raw, then runs web/decode.py's pieces on it
and saves what the Rust tests compare: ribbon.npy, info.json. The Rust tests
read OUT_DIR from PAKON_GOLDEN and are skipped without it.
"""
import argparse
import json
import shutil
import sys
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "tools"))

import pakon_image as pi  # noqa: E402
from web import decode  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("raw")
    ap.add_argument("out")
    ap.add_argument("--rows", default="20000:32000")
    ap.add_argument("--stride", type=int, default=8000)
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    r0, r1 = (int(x) for x in a.rows.split(":"))
    raw = np.memmap(a.raw, dtype="<u2", mode="r")
    sl = out / "slice.raw"
    raw[r0 * a.stride:r1 * a.stride].tofile(sl)
    flat = Path(a.raw + ".flat.json")
    if flat.exists():
        shutil.copy(flat, str(sl) + ".flat.json")

    info = {"layout_full": pi.detect_row_layout(raw)}
    rgb, _ = decode._build_ribbon(sl, lambda *_: None)
    lw = a.stride
    img = np.fromfile(sl, "<u2")
    img = img[: img.size // lw * lw].reshape(-1, lw)
    al, cm = pi.marker_align(img)
    width = lw // 4
    nvis = width * 3
    chans = {"r": al[:, 0:nvis:3], "g": al[:, 1:nvis:3], "b": al[:, 2:nvis:3]}
    m = pi.measure_leads(chans, 0, width)
    info.update(marker=cm, measured={"g": m["g"], "b": m["b"]},
                leads=pi.validated_leads(m, width)[0], shape=list(rgb.shape))
    np.save(out / "ribbon.npy", rgb)
    (out / "info.json").write_text(json.dumps(info, indent=1))
    print(json.dumps(info))


if __name__ == "__main__":
    main()
