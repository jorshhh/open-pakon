"""Infrared dust removal in tools/pakon_ice.py on synthetic frames (no
hardware, no scans). Needs numpy; skipped when it is missing."""

from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
try:
    import numpy as np
    import pakon_ice as ice
except ImportError:  # pragma: no cover
    ice = None

H = W = 256
IR_CLEAR = 58000.0          # clear film in the IR plane (~0.89 of full scale)


def frame(seed=1):
    """A clean linear frame (black off): smooth picture detail in R/G/B, the
    IR flat at the clear-film level with a little noise."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:H, 0:W].astype(np.float32)
    pic = 20000 + 8000 * np.sin(x / 17) * np.cos(y / 23)
    rgb = np.stack([pic, 0.8 * pic, 0.6 * pic], axis=-1).astype(np.float32)
    ir = (IR_CLEAR + rng.normal(0, 60, (H, W))).astype(np.float32)
    return rgb, ir


def blob(cy, cx, r):
    y, x = np.mgrid[0:H, 0:W]
    return (y - cy) ** 2 + (x - cx) ** 2 <= r * r


@unittest.skipIf(ice is None, "numpy not installed")
class CleanTests(unittest.TestCase):
    def test_clean_frame_untouched(self):
        rgb, ir = frame()
        out, mask, cal = ice.clean(rgb, ir)
        self.assertIsNotNone(cal)
        self.assertLess(mask.mean(), 0.001)
        np.testing.assert_array_equal(out[~mask], rgb[~mask])

    def test_no_clear_film_returns_input(self):
        rgb, ir = frame()
        out, mask, cal = ice.clean(rgb, ir * 0.1)      # IR below TAU everywhere
        self.assertIsNone(cal)
        self.assertFalse(mask.any())
        np.testing.assert_array_equal(out, rgb)

    def test_dust_and_hair_rebuilt(self):
        clean_rgb, ir = frame()
        rgb = clean_rgb.copy()
        speck = blob(100, 100, 3)
        hair = np.zeros((H, W), bool)
        hair[180, 30:220] = True
        for m, ir_k, vis_k in ((speck, 0.3, 0.5), (hair, 0.5, 0.7)):
            ir[m] *= ir_k
            rgb[m] *= vis_k
        out, mask, _ = ice.clean(rgb, ir)
        self.assertGreater(mask[speck].mean(), 0.95)
        self.assertGreater(mask[hair].mean(), 0.95)
        for m in (speck, hair):
            err = np.abs(out[m] - clean_rgb[m]) / clean_rgb[m]
            self.assertLess(float(np.median(err)), 0.05)
        # Away from the defects nothing moves.
        far = ~(blob(100, 100, 12) | np.isin(np.arange(H), range(170, 191))[:, None])
        np.testing.assert_array_equal(out[far], rgb[far])

    def test_clean_frame_keeps_black(self):
        rgb, ir = frame()
        raw = (rgb + 300).astype(np.uint16)
        out, mask = ice.clean_frame(raw, ir, np.ones(W, np.float32), ice.PAKON_BASE16_DPI)
        self.assertEqual(out.dtype, np.uint16)
        self.assertLess(mask.mean(), 0.001)
        self.assertLessEqual(int(np.abs(out.astype(int) - raw.astype(int))[~mask].max()), 1)


@unittest.skipIf(ice is None, "numpy not installed")
class PakonIrTests(unittest.TestCase):
    def flat(self, w, rel=None):
        rel = np.ones(w) if rel is None else rel
        return {"px0": 0, "row_samples": 4 * w,
                "bright": [[0] * w] * 3 + [list(300 + 40000 * rel)]}

    def test_ir_plane_gain_and_duty(self):
        w = 64
        img = np.full((10, 4 * w), 300 + 20000, np.uint16)
        flat = self.flat(w)
        ir = ice.ir_plane(img, flat, np.arange(10), 0)
        self.assertEqual(ir.shape, (10, w))
        self.assertAlmostEqual(float(ir[0, 0]), 0.45 * ice.M, delta=1)
        flat.update(duties=[1, 1, 1, 1000], scan_duties=[1, 1, 1, 1200])
        ir = ice.ir_plane(img, flat, np.arange(10), 0)
        self.assertAlmostEqual(float(ir[0, 0]), 0.45 * ice.M * 1000 / 1200, delta=1)

    def test_ir_plane_refuses_no_ir_lane(self):
        w = 64
        img = np.zeros((10, 3 * w), np.uint16)
        flat = self.flat(w)
        flat.update(row_samples=3 * w, ir_lane=False)
        with self.assertRaises(ValueError):
            ice.ir_plane(img, flat, np.arange(10), 0)

    def test_noise_factor_follows_gate_shading(self):
        # The gate shades the line start: 0.26x the light at column 0.
        w = 400
        rel = np.ones(w)
        rel[:60] = np.linspace(0.26, 1.0, 60)
        k = ice.ir_noise_factor(self.flat(w, rel), w)
        self.assertAlmostEqual(float(k[0]), 0.9 / 0.26, places=2)
        self.assertTrue((k[100:] == 1.0).all())
        self.assertTrue((k >= 1.0).all())

    def test_tail_factor_edge_not_scratch(self):
        rng = np.random.default_rng(2)
        ir = (IR_CLEAR + rng.normal(0, 60, (300, W))).astype(np.float32)
        # Heavy dark tail in the first 40 columns (as on every Pakon line)...
        edge = rng.random((300, 40)) < 0.02
        ir[:, :40][edge] -= 2000
        # ...and specks along one column in the middle (a scratch along the scan).
        ir[::20, 128] -= 2000
        k = ice.ir_tail_factor(ir)
        self.assertGreater(float(k[:20].min()), 1.5)
        self.assertLess(float(k[100:156].max()), 1.2)


if __name__ == "__main__":
    unittest.main()
