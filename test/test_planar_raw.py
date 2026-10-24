"""The TLX planar .raw export (web/decode.py write_planar_raw): the layout of
TLB.dll FN_bSaveToMemory, planar 16 with SAV_FileHeader (docs/TLB_FINDINGS.md,
"Planar .raw"). Skips itself when numpy/tifffile or the web decoder's
imports are missing from the interpreter."""

from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

try:
    import numpy as np
    import tifffile
    from web.decode import write_planar_raw
except ImportError as exc:  # pragma: no cover
    print(f"skip: {exc}")
    raise SystemExit(0)


class PlanarRawTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        d = Path(self.tmp.name)
        self.tif, self.raw = d / "frame_raw.tif", d / "frame.raw"
        # 2 x 3 frame: black (300), open gate (300 + 64000), and in between.
        self.rgb = np.array([[[300, 300, 300], [64300, 64300, 64300], [65535, 0, 700]],
                             [[1300, 2300, 3300], [4300, 5300, 6300], [7300, 8300, 9300]]],
                            dtype=np.uint16)
        tifffile.imwrite(str(self.tif), self.rgb, photometric="rgb")

    def tearDown(self):
        self.tmp.cleanup()

    def test_header_and_planes(self):
        write_planar_raw(self.tif, self.raw)
        data = self.raw.read_bytes()
        hdr = np.frombuffer(data[:16], dtype="<u4")
        self.assertEqual(hdr.tolist(), [16, 3, 2, 48])        # size, width, height, bpp
        self.assertEqual(len(data), 16 + 3 * 2 * 3 * 2)
        planes = np.frombuffer(data[16:], dtype="<u2").reshape(3, 2, 3)
        # R plane first, row by row; 14-bit, (raw - 300) / 4, rounded
        self.assertEqual(planes[0].tolist(), [[0, 16000, 16309], [250, 1000, 1750]])
        self.assertEqual(planes[1, 0].tolist(), [0, 16000, 0])
        self.assertEqual(planes[2, 1].tolist(), [750, 1500, 2250])

    def test_no_header(self):
        write_planar_raw(self.tif, self.raw, header=False)
        self.assertEqual(self.raw.stat().st_size, 3 * 2 * 3 * 2)


if __name__ == "__main__":
    unittest.main()
