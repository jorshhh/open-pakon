"""OEM dust removal (tools/oem_dice.py): oem/DMLDICELib.dll under emulation.
Skipped when numpy, unicorn or pefile are missing."""

from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
try:
    import numpy as np
    import oem_dice
    AVAILABLE = oem_dice.available()
except ImportError:  # pragma: no cover
    AVAILABLE = False


def dusty_frame(h=300, w=400, seed=0):
    """A smooth picture (16-bit, black at 300) and its IR plane, with round
    specks dark in both, like dust; returns (raw, ir, dust mask, clean raw)."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w]
    clean = np.stack([20000 + 8000 * np.sin(x / 97.0 + c) * np.cos(y / 131.0)
                      for c in range(3)], -1)
    clean += rng.normal(0, 150, clean.shape)
    ir = 48000 + rng.normal(0, 150, (h, w))
    dust = np.zeros((h, w), bool)
    for _ in range(30):
        cy, cx, r = rng.integers(8, h - 8), rng.integers(8, w - 8), rng.integers(1, 4)
        dust |= (y - cy) ** 2 + (x - cx) ** 2 <= r * r
    raw = clean.copy()
    raw[dust] = 300 + (raw[dust] - 300) * 0.3
    ir[dust] *= 0.3
    return (raw.clip(0, 65535).astype(np.uint16), ir.astype(np.float32), dust,
            clean.clip(0, 65535))


@unittest.skipUnless(AVAILABLE, "unicorn/pefile or oem/DMLDICELib.dll missing")
class OemDiceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dice = oem_dice.Dice()

    def test_removes_dust(self):
        raw, ir, dust, clean = dusty_frame()
        out, mask = oem_dice.clean_frame(self.dice, raw, ir)
        before = np.abs(raw.astype(np.float32) - clean)[dust].mean()
        after = np.abs(out.astype(np.float32) - clean)[dust].mean()
        self.assertLess(after, before / 10)
        self.assertGreater(mask[dust].mean(), 0.9)       # it found the specks
        self.assertLess(mask[~dust].mean(), 0.02)        # and little else

    def test_untouched_pixels_keep_16_bits(self):
        raw, ir, _, _ = dusty_frame(seed=3)
        out, mask = oem_dice.clean_frame(self.dice, raw, ir)
        np.testing.assert_array_equal(out[~mask], raw[~mask])

    def test_clean_frame_unchanged(self):
        raw, ir, _, _ = dusty_frame(seed=4)
        rng = np.random.default_rng(4)
        ir = (48000 + rng.normal(0, 150, ir.shape)).astype(np.float32)   # no dust in IR
        _, mask = oem_dice.clean_frame(self.dice, raw, ir)
        self.assertLess(mask.mean(), 0.001)


if __name__ == "__main__":
    unittest.main()
