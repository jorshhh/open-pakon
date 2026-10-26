"""The film's DX code in the exports (web/dx.py): table lookup, <raw>.dx.json,
and the metadata written into JPEG and TIFF files. Skips itself when
numpy/Pillow/tifffile are missing from the interpreter."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

try:
    import numpy as np
    import tifffile
    from PIL import Image
except ImportError as exc:  # pragma: no cover
    print(f"skip: {exc}")
    raise SystemExit(0)

spec = importlib.util.spec_from_file_location("dx", ROOT / "web/dx.py")
dx = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dx)


class LookupTest(unittest.TestCase):
    def test_known_codes(self):
        # pakon_scan.pcapng's strip (serial 3054).
        self.assertEqual(dx.lookup(82, 3), {"name": "Kodak Gold 400", "iso": 400,
                                            "product": 82, "generation": 3})
        # The label already carries the speed: not repeated.
        self.assertEqual(dx.lookup(96, 1)["name"], "Kodak Gold 400 Gen 9")

    def test_label_typos_fixed(self):
        # The OEM table's own spelling: ILLFORD, FUJICOLOHR, KODAKCOLGOLD.
        self.assertEqual(dx.lookup(110, 4)["name"], "Ilford 400")
        self.assertEqual(dx.lookup(10, 2)["name"].split()[:2], ["Fujicolor", "Reala"])
        self.assertTrue(dx.lookup(80, 6)["name"].startswith("Kodacolor Gold"))

    def test_unknown_codes_give_nothing(self):
        # The two full rolls in our captures: after the 2006 table.
        self.assertIsNone(dx.lookup(97, 10))
        self.assertIsNone(dx.lookup(96, 12))
        self.assertIsNone(dx.lookup(0, 0))
        self.assertIsNone(dx.lookup(82, 16))
        self.assertIsNone(dx.lookup(200, 0))


class FilesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.film = dx.lookup(82, 3)

    def tearDown(self):
        self.tmp.cleanup()

    def test_for_raw(self):
        raw = self.dir / "roll.raw"
        self.assertIsNone(dx.for_raw(raw))
        (self.dir / "roll.raw.dx.json").write_text(json.dumps(
            {"version": 1, "product": 82, "generation": 3, "codes": 5,
             "rejected": 0, "reads": 9}))
        self.assertEqual(dx.for_raw(raw)["name"], "Kodak Gold 400")
        (self.dir / "roll.raw.dx.json").write_text('{"product": 97, "generation": 10}')
        self.assertIsNone(dx.for_raw(raw))
        (self.dir / "roll.raw.dx.json").write_text("not json")
        self.assertIsNone(dx.for_raw(raw))

    def test_jpeg(self):
        path = self.dir / "f.jpg"
        Image.new("RGB", (8, 8)).save(path, "JPEG", **dx.jpeg_kwargs(self.film))
        with Image.open(path) as im:
            exif = im.getexif()
            self.assertEqual(exif[0x010E], "Kodak Gold 400")
            self.assertEqual(exif.get_ifd(0x8769)[0x8827], 400)
            xmp = im.info.get("xmp", b"")
            self.assertIn(b"Kodak Gold 400</rdf:li>", xmp)
            self.assertIn(b"<rdf:li>400</rdf:li></rdf:Seq></exif:ISOSpeedRatings>", xmp)

    def test_tiff(self):
        path = self.dir / "f.tif"
        tifffile.imwrite(path, np.zeros((8, 8, 3), np.uint16), photometric="rgb",
                         **dx.tiff_kwargs(self.film))
        with tifffile.TiffFile(path) as tf:
            tags = tf.pages[0].tags
            self.assertEqual(tags["ImageDescription"].value, "Kodak Gold 400")
            xmp = bytes(tags["XMP"].value)
            self.assertIn(b"Kodak Gold 400</rdf:li>", xmp)
            self.assertIn(b"ISOSpeedRatings", xmp)
            np.testing.assert_array_equal(tf.asarray(), 0)

    def test_no_film_no_metadata(self):
        self.assertEqual(dx.jpeg_kwargs(None), {})
        self.assertEqual(dx.tiff_kwargs(None), {})

    def test_xmp_is_well_formed(self):
        import xml.dom.minidom
        packet = dx.xmp({"name": "A & B <test>", "iso": 100})
        body = packet.decode()
        body = body[body.index("<x:xmpmeta"):body.index("<?xpacket end")]
        xml.dom.minidom.parseString(body)


if __name__ == "__main__":
    unittest.main()
