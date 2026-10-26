"""The film's DX code: what the scan read, the film it names, and the
metadata written into the exports.

A --scan-code scan writes <raw>.dx.json when the scanner read the film's DX
edge code (product and generation, docs/PROTOCOL.md "DX sensor block"). The
OEM product-code table (oem/ansel/common/common-ProdCodeTable.dpi, last
revised 2006) gives each code an ISO and, for each product, a product-line
label. A code the table does not know gives nothing: no name, no metadata.
"""
from __future__ import annotations

import json
from functools import lru_cache
from pathlib import Path
from xml.sax.saxutils import escape

from PIL import Image

TABLE_PATH = (Path(__file__).resolve().parent.parent / "oem" / "ansel" / "common"
              / "common-ProdCodeTable.dpi")

# Spelling fixes for the table's labels (after title case).
_LABEL_FIXES = {"Illford": "Ilford", "Fujicolohr": "Fujicolor",
                "Kodakcolgold": "Kodacolor Gold"}

_EXIF_IFD = 0x8769
_IMAGE_DESCRIPTION = 0x010E
_ISO_SPEED = 0x8827


@lru_cache(maxsize=1)
def _table() -> dict[int, tuple[str, list[int]]]:
    """product -> (label, 16 ISO speeds by generation). The file is a comment
    `#  <product> <label>` before each row of 16 speeds."""
    out: dict[int, tuple[str, list[int]]] = {}
    try:
        lines = TABLE_PATH.read_text(encoding="latin-1").splitlines()
    except OSError:
        return out
    product, label = None, ""
    for line in lines:
        s = line.strip()
        if s.startswith("#"):
            words = s[1:].split()
            if words and words[0] == "ProdCode":
                words = words[1:]
            if words and words[0].isdigit():
                product, label = int(words[0]), " ".join(words[1:])
            continue
        speeds = s.split()
        if product is not None and len(speeds) == 16 and all(w.isdigit() for w in speeds):
            out[product] = (label, [int(w) for w in speeds])
            product = None
    return out


def lookup(product: int, generation: int) -> dict | None:
    """{"name", "iso", "product", "generation"} for a code the table knows,
    else None. `name` is None when the product has no label."""
    row = _table().get(product)
    if row is None or not 0 <= generation < 16 or row[1][generation] == 0:
        return None
    label, speeds = row
    iso = speeds[generation]
    name = label.title() if label else None
    for wrong, right in _LABEL_FIXES.items():
        name = name.replace(wrong, right) if name else name
    if name and str(iso) not in name.split():
        name = f"{name} {iso}"
    return {"name": name, "iso": iso, "product": product, "generation": generation}


def code_for_raw(raw: Path) -> tuple[int, int] | None:
    """(product, generation) read during the scan of `raw`, known to the
    table or not, or None when no code was read."""
    try:
        d = json.loads(raw.with_name(raw.name + ".dx.json").read_text())
        return int(d["product"]), int(d["generation"])
    except (OSError, ValueError, KeyError, TypeError):
        return None


def for_raw(raw: Path) -> dict | None:
    """The film read during the scan of `raw`, or None (no code read, or a
    code the table does not know)."""
    try:
        d = json.loads(raw.with_name(raw.name + ".dx.json").read_text())
        return lookup(int(d["product"]), int(d["generation"]))
    except (OSError, ValueError, KeyError, TypeError):
        return None


def xmp(film: dict) -> bytes:
    """An XMP packet: the film name as description and keyword, the ISO."""
    props = []
    if film.get("name"):
        name = escape(film["name"])
        props.append('<dc:description><rdf:Alt><rdf:li xml:lang="x-default">'
                     f'{name}</rdf:li></rdf:Alt></dc:description>')
        props.append(f"<dc:subject><rdf:Bag><rdf:li>{name}</rdf:li></rdf:Bag></dc:subject>")
    props.append("<exif:ISOSpeedRatings><rdf:Seq><rdf:li>"
                 f"{int(film['iso'])}</rdf:li></rdf:Seq></exif:ISOSpeedRatings>")
    body = "\n    ".join(props)
    return (
        '<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>\n'
        '<x:xmpmeta xmlns:x="adobe:ns:meta/">\n'
        ' <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
        '  <rdf:Description rdf:about=""\n'
        '    xmlns:dc="http://purl.org/dc/elements/1.1/"\n'
        '    xmlns:exif="http://ns.adobe.com/exif/1.0/">\n'
        f"    {body}\n"
        "  </rdf:Description>\n"
        " </rdf:RDF>\n"
        "</x:xmpmeta>\n"
        '<?xpacket end="w"?>'
    ).encode("utf-8")


def jpeg_kwargs(film: dict | None) -> dict:
    """Extra Image.save arguments for a JPEG: EXIF description and ISO, XMP."""
    if not film:
        return {}
    exif = Image.Exif()
    if film.get("name"):
        exif[_IMAGE_DESCRIPTION] = film["name"]
    exif.get_ifd(_EXIF_IFD)[_ISO_SPEED] = int(film["iso"])
    return {"exif": exif.tobytes(), "xmp": xmp(film)}


def tiff_kwargs(film: dict | None) -> dict:
    """Extra tifffile.imwrite arguments: ImageDescription and the XMP tag
    (700). TIFF has no Exif IFD here, so the ISO goes in the XMP only."""
    if not film:
        return {}
    packet = xmp(film)
    kw = {"metadata": None, "extratags": [(700, "B", len(packet), packet, True)]}
    if film.get("name"):
        kw["description"] = film["name"]
    return kw
