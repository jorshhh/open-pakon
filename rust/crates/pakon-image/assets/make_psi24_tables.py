#!/usr/bin/env python3
"""Provenance for the two Kodak-CMM tables the PSI 2.4 render uses (cmm65.u16, cmm65_u8.u8).

Both are the real Kodak CMM (`SpCombineXforms(rpd.pf, srgb.pf)` + `SpEvaluate`) sampled on a 65^3
grid with input levels round(i*4095/64), run by the Windows harness in
~/pakon-re/windows_test/psi24-cmm (README there).  The harness writes planar output
(channel, b, g, r; R varies fastest); this script transposes it to R-major (r, g, b, channel):

  out/C65_u16_type5_16bit.bin  (SpEvaluate output type 5, u16 LE, 0..65535)  -> cmm65.u16
  out/C65_u8.bin               (SpEvaluate output type 1, u8, what the render uses) -> cmm65_u8.u8
                                (stored as raw bytes)

usage: make_psi24_tables.py <psi24-cmm/out dir> [<output dir>]
Without an output dir it only checks the result against the committed tables (next to this script)
and exits 1 on a difference; with one, it writes the tables there.
"""
import os
import sys
import numpy as np

N = 65
here = os.path.dirname(os.path.abspath(__file__))
src = sys.argv[1]
dst = sys.argv[2] if len(sys.argv) > 2 else None
bad = False
for name, dtype, out in (("C65_u16_type5_16bit.bin", "<u2", "cmm65.u16"), ("C65_u8.bin", "u1", "cmm65_u8.u8")):
    a = np.fromfile(os.path.join(src, name), dtype=dtype).reshape(3, N, N, N)  # (c, b, g, r)
    t = np.ascontiguousarray(a.transpose(3, 2, 1, 0))  # (r, g, b, c)
    same = open(os.path.join(here, out), "rb").read() == t.tobytes()
    print(out, "identical to the committed table" if same else "DIFFERS from the committed table")
    bad |= not same
    if dst:
        t.tofile(os.path.join(dst, out))
sys.exit(1 if bad else 0)
