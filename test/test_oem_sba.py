"""OEM scene balance (tools/oem_sba.py): oem/PakonIMAu.dll's SBA/FOS code
under emulation. Skipped when numpy, unicorn or pefile are missing."""

from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
try:
    import numpy as np
    import oem_sba
    AVAILABLE = oem_sba.available()
except ImportError:  # pragma: no cover
    AVAILABLE = False


def synthetic_roll(seed=1, red=0, frames=5):
    """Textured frames in 12-bit RPD with a mask-like R < G < B offset."""
    rng = np.random.default_rng(seed)
    out = []
    for _ in range(frames):
        level = rng.uniform(1100, 1800)
        f = np.empty((128, 192, 3), np.int16)
        tex = rng.normal(0, 250, (128, 192))
        for c, off in enumerate((-300 + red, 0, 200)):
            f[..., c] = np.clip(level + off + tex + rng.normal(0, 40, (128, 192)), 0, 4095)
        out.append(f)
    return out


@unittest.skipUnless(AVAILABLE, "unicorn/pefile or oem/ missing")
class OemSbaTests(unittest.TestCase):
    def test_f80_round_trip(self):
        for x in (0.0, 1.5, -0.33, 1e-300, 12345.678, -2.0 ** 40):
            self.assertEqual(oem_sba._f80_to_float(oem_sba._float_to_f80(x)), x)

    def test_roll_shifts_pinned(self):
        # The emulated OEM output for a fixed input; a change means the
        # emulation (or the structures fed to it) changed.
        got = oem_sba.roll_shifts(synthetic_roll())
        np.testing.assert_array_equal(got, [[506, 189, -19], [362, 45, -163],
                                            [601, 284, 75], [326, 11, -199],
                                            [715, 398, 191]])

    def test_less_red_gets_more_red(self):
        a = oem_sba.roll_shifts(synthetic_roll())
        b = oem_sba.roll_shifts(synthetic_roll(red=-150))
        d = (b - a).mean(axis=0)
        self.assertGreater(d[0] - d[2], 120)
        self.assertLess(abs(d[1] - d[2]), 20)

    def test_short_roll_balances_each_frame(self):
        # Fewer than mff frames: no FOS (the OEM's history needs useHistory,
        # 0 in every shipped .dpi).
        got = oem_sba.roll_shifts(synthetic_roll()[:2])
        np.testing.assert_array_equal(got, [[507, 186, -18], [363, 42, -162]])

    def test_analysis_image_is_landscape(self):
        def rpd(raw, negmat, lut):
            return raw.astype(np.float32) / 16
        tall = np.full((3000, 2000, 3), 32000, np.uint16)
        img = oem_sba.analysis_image(tall, None, None, rpd)
        self.assertEqual(img.shape, (oem_sba.ANALYSIS_H, oem_sba.ANALYSIS_W, 3))
        self.assertTrue(np.all(img == 2000))


    def test_scene_dmin_lowers_the_frame_film_base(self):
        # A film base below every frame's own minimum moves the balance; one
        # above it (a fogged leader) changes nothing.
        roll = synthetic_roll()
        plain = oem_sba.roll_shifts(roll)
        np.testing.assert_array_equal(oem_sba.roll_shifts(roll, dmin=[4000] * 3), plain)
        self.assertFalse(np.array_equal(oem_sba.roll_shifts(roll, dmin=[300, 600, 800]), plain))


@unittest.skipIf("np" not in globals(), "numpy missing")
class SbaInputTests(unittest.TestCase):
    """The inputs the OEM gives Ansel: no emulator needed."""

    def test_dpi_for_follows_sba_map(self):
        cases = {(78, 13): "sba-CN-default-78-13.dpi", (79, 15): "sba-CN-default-79-15.dpi",
                 (96, 1): "sba-CN-default-96-1.dpi", (96, 7): "sba-CN-default-96-1.dpi",
                 (43, 2): "sba-CN-default-43-1.dpi", (78, 2): "sba-CN-default.dpi",
                 (110, 4): "sba-CN-default.dpi", (None, None): "sba-CN-default.dpi",
                 (-1, -1): "sba-CN-default.dpi"}
        for code, name in cases.items():
            self.assertEqual(oem_sba.dpi_for(*code), name, code)
        # APS (sourceType 2) has 78/13 and 79/15 only.
        self.assertEqual(oem_sba.dpi_for(78, 13, source_type=2), "sba-CN-default-78-13.dpi")
        self.assertEqual(oem_sba.dpi_for(96, 1, source_type=2), "sba-CN-default.dpi")

    def test_frame_dmin_rule(self):
        # FUN_10218110: per channel min(scene, frame); a scene value at or
        # below mindmin counts as none.
        s = type("S", (), {"dpi": {"mindmin": ["100", "200", "300"]}})()
        fd = oem_sba.Sba.frame_dmin
        self.assertEqual(fd(s, [500, 600, 700]), [500, 600, 700])
        self.assertEqual(fd(s, [500, 600, 700], [400, 650, 250]), [400, 600, 700])
        self.assertEqual(fd(s, [500, 600, 700], [100, 200, 300]), [500, 600, 700])

    def test_film_base_from_the_leader(self):
        # Open gate, then clear leader (with a few brighter specks of base),
        # then the first frame; rows_per_mm 10.
        rib = np.full((400, 80, 3), 65535, np.uint16)
        rib[30:200] = (48000, 28000, 27000)
        rib[30:200:7, 40] = (48500, 28400, 27300)
        rib[200:] = (20000, 12000, 10000)
        base = oem_sba.film_base(rib, 200, 10.0)
        self.assertTrue(48000 <= base[0] <= 48500 and 28000 <= base[1] <= 28400)
        self.assertIsNone(oem_sba.film_base(rib, 50, 10.0))     # too short


if __name__ == "__main__":
    unittest.main()
