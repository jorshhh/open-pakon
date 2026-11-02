"""src/pakon_render.c against the numpy render_oem in tools/pakon_image.py
(no hardware): the output must match bit for bit. Skipped when numpy or
build/libpakon_render is missing (cmake --build build)."""

import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
try:
    import numpy as np
    spec = importlib.util.spec_from_file_location("pakon_image", ROOT / "tools/pakon_image.py")
    img = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(img)
except ImportError:  # pragma: no cover
    img = None


class _Lut:
    """An RpdLut stand-in: a smooth random table, no profile needed."""
    def __init__(self, n=52):
        rng = np.random.default_rng(7)
        g = np.linspace(0.0, 1.0, n, dtype=np.float32)
        base = np.stack(np.meshgrid(g, g, g, indexing="ij"), -1) ** 0.8
        self.table = np.clip(base + rng.normal(0, 0.02, base.shape), 0, 1).astype(np.float32)
        self.n, self.adjusted, self.path = n, {}, None


@unittest.skipIf(img is None or img._native_lib() is None,
                 "numpy or build/libpakon_render missing")
class NativeRenderTests(unittest.TestCase):
    def setUp(self):
        rng = np.random.default_rng(3)
        m = np.zeros((3, 10), np.float32)        # a NegMatrix-like polynomial
        m[:, :3] = np.eye(3) * 0.9 + 0.05
        m[:, 3:9] = rng.normal(0, 2e-5, (3, 6))
        m[:, 9] = [40, -20, 15]
        self.negmat = m
        self.lut = img._c41_lut()
        self.rpd_lut = _Lut()

    def frame(self, h, w, seed=0):
        rng = np.random.default_rng(seed)
        f = rng.integers(0, 65536, (h, w, 3)).astype(np.uint16)
        f[0, 0] = 0
        if h * w > 1:
            f[-1, -1] = 65535
        return f

    def check(self, part, **kw):
        native = img.render_oem(part, self.negmat, self.lut, self.rpd_lut, **kw)
        img._NATIVE["lib"], lib = None, img._NATIVE["lib"]
        try:
            ref = img.render_oem(part, self.negmat, self.lut, self.rpd_lut, **kw)
        finally:
            img._NATIVE["lib"] = lib
        self.assertEqual(native.dtype, ref.dtype)
        np.testing.assert_array_equal(native, ref)

    def test_plain(self):
        self.check(self.frame(130, 97))          # bands of 64 plus a partial one

    def test_balance_and_adjustments(self):
        bal = (np.array([1.03, 0.98, 1.0], np.float32), np.array([12, -7, 3], np.float32))
        self.check(self.frame(70, 41, 1), balance=bal,
                   adjust={"contrast": 75, "red": 25, "blue": -50, "brightness": 10})

    def test_sharpening_across_bands(self):
        for sharp in (67, 133, 200):
            self.check(self.frame(129, 33, sharp), adjust={"sharpness": sharp})

    def test_grey(self):
        self.check(self.frame(65, 20, 2), grey=True)
        self.check(self.frame(65, 20, 2), grey=True, adjust={"sharpness": 133, "contrast": -40})

    def test_tiny_frames(self):
        for h, w in ((1, 1), (1, 5), (5, 1), (2, 2)):
            self.check(self.frame(h, w, h * 10 + w), adjust={"sharpness": 200})

    def test_non_contiguous_input(self):
        self.check(np.rot90(self.frame(40, 66, 5)), adjust={"sharpness": 133})


if __name__ == "__main__":
    unittest.main()
