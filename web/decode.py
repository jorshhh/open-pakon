"""
Decode pipeline for the Pakon web service — minilab-style two-stage flow.

Stage 1 (prescan): build the long negative ribbon once, quick-invert a
downscaled copy into a single long PREVIEW strip, and auto-detect the initial
frame centres + pitch. The full-res ribbon is cached to disk (ribbon.npy).

Stage 2 (export): the user confirms/positions each crop in the browser; the
confirmed centres come back and we crop the cached ribbon at full resolution,
run the OEM C-41 inversion + rpd.pf render, and write per-frame TIFF/JPEG.

This mirrors a dedicated minilab: detect → operator confirms frames → export.
"""
import json
import os
import sys
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image

# Pull the decode functions from the CLI tool without installing it.
_TOOLS = Path(__file__).resolve().parent.parent / "tools"
if str(_TOOLS) not in sys.path:
    sys.path.insert(0, str(_TOOLS))

from pakon_image import (  # noqa: E402
    detect_row_layout,
    tlx_frame_grid,
    tlx_geometry,
    FRAME_FORMATS,
    flat_row_layout,
    invert_c41,
    marker_align,
    measure_dmin,
    measure_leads,
    register_zones,
    render_jpeg,
    validated_leads,
    _c41_lut,
    load_neg_matrix,
    render_oem,
    render_slide,
    slide_path,
    measure_slide_flare,
    to_grey,
    RpdLut,
    apply_flat_field,
    find_flat_field,
    load_flat_field,
    oem_roll_balance,
    oem_rpd,
)
import oem_sba  # noqa: E402
import pakon_ice  # noqa: E402

from . import dx, eeprom  # noqa: E402

WORK_DIR = Path("/tmp/pakon_web")
# What an export leaves in WORK_DIR; replaced by the next export or roll.
_FRAME_FILES = ("frame_*", "thumb_*", "contact_sheet.jpg", "export_*.zip")


def clear_work(everything=False):
    """Delete temp files: the exported frames (a new roll or export replaces
    them), or with everything=True all of WORK_DIR, the ribbon cache too (at
    startup: nothing there belongs to this run)."""
    if not WORK_DIR.exists():
        return
    paths = (WORK_DIR.iterdir() if everything else
             (p for pat in _FRAME_FILES for p in WORK_DIR.glob(pat)))
    for p in list(paths):
        if p.is_file():
            try:
                p.unlink()
            except OSError:
                pass
# Row layout is auto-detected per raw (F-135 vs the F-135+ per-resolution
# strides); PAKON_LINEWIDTH + PAKON_IR_LANE=0/1 override the detection.
_LINEWIDTH_OVERRIDE = os.environ.get("PAKON_LINEWIDTH")
_IR_LANE_OVERRIDE = os.environ.get("PAKON_IR_LANE")
_REPO = Path(__file__).resolve().parent.parent
RPD_PROFILE = _REPO / "profiles" / "rpd.pf"


def find_eeprom_dir() -> Path | None:
    """The scanner's EEPROM archive: PAKON_EEPROM_DIR; else, under
    backups/eeprom/, the newest archive for the connected scanner's serial
    (web/eeprom.py reads it automatically); else the newest archive."""
    override = os.environ.get("PAKON_EEPROM_DIR")
    if override:
        p = Path(override).expanduser()
        return p if p.is_dir() else None
    found = eeprom.archives()
    serial = eeprom.connected_serial()
    if serial is not None:
        mine = [d for d in found if eeprom.archive_serial(d) == serial]
        if mine:
            return mine[0]
    return found[0] if found else None


_OEM = {"key": None, "value": None}
_SLIDE = {"key": None, "value": None}


def _oem_path():
    """(NegMatrix, LUT, RpdLut) for the OEM colour path, or None when the
    EEPROM archive or rpd.pf is missing (then the Dmin inversion is used)."""
    eeprom = find_eeprom_dir()
    if eeprom is None or not RPD_PROFILE.exists():
        return None
    key = str(eeprom)
    if _OEM["key"] != key:
        try:
            _OEM["value"] = (load_neg_matrix(key), _c41_lut(), RpdLut(str(RPD_PROFILE)))
        except (OSError, ValueError):
            _OEM["value"] = None
        _OEM["key"] = key
    return _OEM["value"]
def _slide_path():
    """(PosMatrix, LUT, ColRevLink, romm.pf table) for slides: the unit's
    PosMatrix when its EEPROM archive is found, else the factory one."""
    eeprom_dir = find_eeprom_dir()
    key = str(eeprom_dir)
    if _SLIDE["key"] != key:
        try:
            _SLIDE["value"] = slide_path(None if eeprom_dir is None else key)
        except (OSError, ValueError):
            _SLIDE["value"] = slide_path(None)
        _SLIDE["key"] = key
    return _SLIDE["value"]


def _sba_on():
    """The OEM scene balance (tools/oem_sba.py) runs when its emulator and
    oem/ are available; PAKON_OEM_SBA=0 turns it off."""
    return os.environ.get("PAKON_OEM_SBA", "1") != "0" and oem_sba.available()


def _sba_inputs(raw_path, rgb, first_frame, oem):
    """What the OEM gives Ansel besides the pictures (docs/TLB_FINDINGS.md
    "Roll balance"): the SBA parameter file sba.map picks for the raw's DX
    code, and the strip's film base in RPD (None when the leader is too
    short to measure; PAKON_SBA_DMIN=0 leaves it out)."""
    code = dx.code_for_raw(Path(raw_path))
    sba_dpi = oem_sba.dpi_for(*code) if code else oem_sba.dpi_for()
    dmin = None
    if (oem is not None and first_frame is not None and
            os.environ.get("PAKON_SBA_DMIN", "1") != "0"):
        rows_per_mm = tlx_geometry(rgb.shape[1], "full")[1] / 36.0
        base = oem_sba.film_base(rgb, int(first_frame), rows_per_mm)
        if base is not None:
            dmin = oem_sba.film_base_rpd(base, oem[0], oem[1], oem_rpd)
    return sba_dpi, dmin


def _sba_shifts(crops, oem, roll=None, sba_dpi=None, dmin=None):
    """OEM SBA per-frame RPD shifts (n, 3) for raw frame crops, balanced
    against `roll` (raw crops of the whole roll; default the crops), with
    the parameter file `sba_dpi` and the strip's film base `dmin` (RPD), or
    None when it is unavailable or fails (the caller falls back to the roll
    balance)."""
    if not crops or oem is None or not _sba_on():
        return None
    try:
        def analysis(c):
            return oem_sba.analysis_image(c, oem[0], oem[1], oem_rpd)
        imgs = [analysis(c) for c in crops]
        return oem_sba.roll_shifts(
            imgs, dpi=sba_dpi or oem_sba.dpi_for(),
            roll=None if roll is None else [analysis(c) for c in roll], dmin=dmin)
    except Exception as exc:                       # never block an export
        print(f"OEM SBA failed, using the roll balance: {exc}", file=sys.stderr)
        return None


RIBBON_PATH = WORK_DIR / "ribbon.npy"
# The IR plane of the ribbon (row for row), flat-fielded, for dust removal
# at export (tools/pakon_ice.py); only for raws scanned with IR, with the
# calibration's IR table.
IR_RIBBON_PATH = WORK_DIR / "ir_ribbon.npy"
PREVIEW_PATH = WORK_DIR / "preview.jpg"
_PREVIEW_H = 500               # target short-axis (frame height) of the preview
_FRAME_W = 3000                # fixed crop width (long axis): 3000x2000, 3:2


def _build_ribbon(raw_path, emit):
    """Marker-align rows → deinterleave (R,G,B triplets at stride 3, IR trailing)
    → trilinear registration. Returns (rgb, ice): the FULL assembled negative
    ribbon (rows=long axis, cols=width), and for dust removal (ir plane
    uint16 row for row with rgb, flat-field dict), or None when the raw has
    no IR lane or its flat field no IR table.

    Aligning to the per-scanline marker bit fixes the triplet phase regardless of
    where the 0x86 capture started, so the colour cast is consistent across scans
    (otherwise it flips purple/turquoise). IR is a single trailing line, so there
    is one visible zone — no wrap-split.

    Autocrop is intentionally NOT applied: it kept only the single largest run
    of image rows, which silently dropped every frame past an interior dark
    exposure / dense rebate (a 36-exp roll came out as ~13). Frames are now
    placed manually in the web UI, so we show the whole ribbon end to end."""
    emit("Loading", 0.0)
    raw = np.memmap(str(raw_path), dtype="<u2", mode="r")

    emit("Detecting row layout", 0.05)
    if _LINEWIDTH_OVERRIDE:
        lw = int(_LINEWIDTH_OVERRIDE)
        has_ir = (_IR_LANE_OVERRIDE != "0") if _IR_LANE_OVERRIDE is not None \
            else (lw == 8000)
    else:
        # pakon_replay --scan-code records the layout next to the raw; the
        # stride alone is ambiguous (F-135 Base 8 + IR = F-135+ Base 16 = 6000).
        layout = flat_row_layout(str(raw_path)) or detect_row_layout(raw)
        if layout is None:
            raise ValueError(
                "could not detect the row layout of this raw (no known stride "
                "matched); set PAKON_LINEWIDTH (+ PAKON_IR_LANE=0/1) to force")
        lw, has_ir = layout
    width = lw // 4 if has_ir else lw // 3
    emit(f"Layout: {lw} samples/row, {width} px "
         f"({'RGB+IR' if has_ir else 'RGB'})", 0.08)

    lines = raw.size // lw
    img = raw[: lines * lw].reshape(lines, lw)

    emit("Aligning rows", 0.10)
    img, _ = marker_align(img)
    nvis = width * 3
    chans = {"r": img[:, 0:nvis:3], "g": img[:, 1:nvis:3], "b": img[:, 2:nvis:3]}

    emit("Registering channels", 0.30)
    leads, _ = validated_leads(measure_leads(chans, 0, width), width)
    rgb = register_zones(chans, [(0, width)], [leads],
                         correct_seam=False, zone_perms=[None])

    # Per-column flat field (the OEM's fixed-pattern correction): the scan's
    # own RAW.flat.json, else the unit's flatfield.json from --light-cal.
    # PAKON_FLAT=0 turns it off.
    flat_path = (None if os.environ.get("PAKON_FLAT", "1") == "0"
                 else find_flat_field(str(raw_path), find_eeprom_dir(), width))
    if flat_path:
        try:
            apply_flat_field(rgb, load_flat_field(flat_path, width))
            emit(f"Flat field: {Path(flat_path).name}", 0.55)
        except (OSError, ValueError, KeyError) as exc:
            print(f"flat field not applied: {exc}", file=sys.stderr)

    # IR plane for dust removal: register_zones keeps red as the row
    # reference, so ribbon row y is raw row y (IR/visible offset measured 0).
    ice = None
    if has_ir and flat_path:
        try:
            with open(flat_path) as fh:
                flat = json.load(fh)
            ir = pakon_ice.ir_plane(img, flat, np.arange(rgb.shape[0]), 0)
            ice = ((ir + 0.5).astype(np.uint16), flat)
        except (OSError, ValueError, KeyError, IndexError) as exc:
            print(f"no IR plane for dust removal: {exc}", file=sys.stderr)
    return rgb, ice


FILMS = ("c41", "bw", "bwc41", "slide")   # colour negative, silver B&W, B&W C-41, slide (E-6)
GREY_FILMS = ("bw", "bwc41")


def prescan(raw_path, progress=None, film="c41", fmt="full") -> dict:
    """Stage 1. Build + cache the ribbon and render the long preview strip.
    The frames found here only feed the roll's colour balance; the boxes are
    placed in Frames (by hand, or autoframe()). Returns a metadata dict (all
    JSON-able).
    film="bw" (silver) and "bwc41" (chromogenic) render B&W: the same
    inversion, then grey; "slide"
    renders reversal film through the OEM positive path (no inversion).
    fmt (full | half | pano) is the picture format the frames are found in
    (the raw's last auto-framing)."""
    grey = film in GREY_FILMS
    WORK_DIR.mkdir(parents=True, exist_ok=True)
    clear_work()                          # the last roll's frames

    def emit(step, pct):
        if progress:
            progress(step, round(pct, 3))

    rgb, ice = _build_ribbon(raw_path, emit)
    rows, cols = rgb.shape[:2]

    emit("Measuring film base", 0.62)
    base = measure_dmin(rgb, pct=99.5)

    emit("Detecting frames", 0.68)
    # Box length per format (rows) for this resolution.
    box_w = {f: tlx_geometry(cols, f)[1] for f in FRAME_FORMATS}
    # OEM framing (docs/TLB_FINDINGS.md "Picture framing").
    starts, frame_w, pitch = tlx_frame_grid(rgb, positive=film == "slide", fmt=fmt)
    centres = [int(s + frame_w // 2) for s in starts]
    if not centres:                       # fallback: single image
        centres = [rows // 2]
        pitch = rows

    emit("Caching ribbon", 0.75)
    np.save(str(RIBBON_PATH), rgb)        # full-res cache for export
    if ice is not None:
        np.save(str(IR_RIBBON_PATH), ice[0])
        # The gate-margin factor from the calibration's IR (per column).
        np.save(str(WORK_DIR / "ir_noise.npy"), pakon_ice.ir_noise_factor(ice[1], cols))
    else:
        for p in (IR_RIBBON_PATH, WORK_DIR / "ir_noise.npy"):
            p.unlink(missing_ok=True)

    emit("Rendering preview", 0.85)
    ds = max(1, int(round(cols / _PREVIEW_H)))   # ~_PREVIEW_H px tall frames
    small = np.ascontiguousarray(rgb[::ds, ::ds])
    oem = _oem_path()
    balance = None
    flare = None
    sba_dpi, scene_dmin = (_sba_inputs(raw_path, rgb, starts[0] if starts else None, oem)
                           if film != "slide" else (None, None))
    if film == "slide":
        flare = measure_slide_flare(rgb)                 # stray light, per roll
        pos = render_slide(small, *_slide_path(), flare=flare)
    elif oem is not None:                                # OEM colour path
        balance = oem_roll_balance(small, oem[0], oem[1])  # once per roll
        fw = int(min(frame_w, pitch))
        shifts = _sba_shifts([rgb[max(0, c - fw // 2):c + fw // 2] for c in centres], oem,
                             sba_dpi=sba_dpi, dmin=scene_dmin)
        # The strip gets the roll's mean OEM shift; the export applies each
        # frame's own.
        pos = render_oem(small, *oem, balance=(
            balance if shifts is None else
            (np.ones(3, np.float32), shifts.mean(axis=0))), grey=grey)
    else:
        pos = invert_c41(small, base, _c41_lut())        # quick sRGB positive
        if grey:
            pos = to_grey(pos)
    strip = np.rot90((pos >> 8).astype(np.uint8), k=1)  # long axis -> horizontal x
    Image.fromarray(strip).save(str(PREVIEW_PATH), "JPEG", quality=85)
    prev_w, prev_h = strip.shape[1], strip.shape[0]     # PIL (w,h)
    scale = prev_w / rows                                # full_row -> preview_x

    emit("Done", 1.0)
    return {
        "ribbon": str(RIBBON_PATH),
        "base": [float(x) for x in base],
        "rpd_balance": (None if balance is None else
                        [[float(x) for x in balance[0]],
                         [float(x) for x in balance[1]]]),
        "rows": int(rows), "cols": int(cols),
        "pitch": int(pitch),
        "frame_w": int(frame_w),
        "box_w": box_w,                    # rows per format: full, half, pano
        "format": fmt,
        "centres": centres,                # initial detected centres (full rows)
        "preview_w": int(prev_w), "preview_h": int(prev_h),
        "scale": float(scale),             # preview_x = full_row * scale
        "film": film,
        "flare": None if flare is None else [float(x) for x in flare],
        "ice": ice is not None,            # an IR plane to remove dust with
        "sba_dpi": sba_dpi,                # OEM scene balance parameters (DX code)
        "scene_dmin": scene_dmin,          # the strip's film base, RPD
    }


def autoframe(ribbon_path, film="c41", fmt="full") -> dict:
    """The OEM framing on the cached ribbon, run when the operator asks for it
    in Frames. Returns the box centres (full rows), the picture length and
    pitch, and the format."""
    rgb = np.load(str(ribbon_path), mmap_mode="r")
    starts, frame_w, pitch = tlx_frame_grid(rgb, positive=film == "slide", fmt=fmt)
    return {"centres": [int(s + frame_w // 2) for s in starts],
            "frame_w": int(frame_w), "pitch": int(pitch), "format": fmt}


def export_frames(ribbon_path, base, centres, widths=None, rotate=90,
                  frame_w=_FRAME_W, progress=None, rpd_balance=None,
                  roll_centres=None, film="c41", flare=None,
                  dx_film=None, ice=False, sba_dpi=None, scene_dmin=None) -> list[dict]:
    """Stage 2. Crop the cached ribbon at each confirmed centre, invert and
    render, write TIFF/JPEG. With the scanner's EEPROM archive and rpd.pf this
    is the OEM F-135 colour path (density LUT -> NegMatrix -> rpd.pf), for both
    the TIFF and the JPEG; otherwise the Dmin inversion + rpd.pf JPEG render.

    `widths` is an optional per-frame crop width (full-res rows) parallel to
    `centres` — lets the operator mix full, half and panoramic boxes. When omitted,
    every crop uses `frame_w`.

    `roll_centres` are the prescan's detected frames: the OEM scene balance
    weighs the exported frames against the whole roll, so a few similar
    frames exported alone are not pushed toward their own average.

    film="bw" / "bwc41" (B&W) writes 16-bit grey TIFFs and grey JPEGs; "slide"
    renders through the OEM positive path, with no scene balance, minus
    `flare` (the prescan's measure_slide_flare).

    `dx_film` (web/dx.py lookup) puts the film's name and ISO in every file's
    metadata; None writes none. `sba_dpi` and `scene_dmin` are the prescan's
    OEM scene balance inputs.

    `ice` removes dust and scratches from each frame with its IR plane
    (tools/pakon_ice.py) before it is rendered, when the prescan cached one."""
    grey = film in GREY_FILMS
    slide = _slide_path() if film == "slide" else None
    WORK_DIR.mkdir(parents=True, exist_ok=True)
    clear_work()                          # the last export's frames

    def emit(step, pct):
        if progress:
            progress(step, round(pct, 3))

    rgb = np.load(str(ribbon_path), mmap_mode="r")
    rows = rgb.shape[0]
    base = np.asarray(base, dtype=np.float32)
    lut = _c41_lut()
    use_rpd = RPD_PROFILE.exists()
    oem = _oem_path()
    rot_k = (rotate // 90) % 4
    frames = []
    n = len(centres)

    def crop_at(centre, fw):
        r1 = min(rows, max(0, int(centre) - fw // 2) + fw)
        return rgb[max(0, r1 - fw):r1]

    def crop(i):
        return crop_at(centres[i], int(widths[i]) if widths else frame_w)

    ir_rib = ir_noise = None
    if ice and IR_RIBBON_PATH.exists():
        ir_rib = np.load(str(IR_RIBBON_PATH), mmap_mode="r")
        ir_noise = np.load(str(WORK_DIR / "ir_noise.npy"))
    dpi = pakon_ice.pakon_dpi(rgb.shape[1])

    def clean(i, part):
        """The crop with dust and scratches rebuilt from the IR plane (black
        300 off for ICE, back on after); unchanged without an IR plane."""
        if ir_rib is None:
            return part
        fw = int(widths[i]) if widths else frame_w
        r1 = min(rows, max(0, int(centres[i]) - fw // 2) + fw)
        out, mask = pakon_ice.clean_frame(part, ir_rib[max(0, r1 - fw):r1], ir_noise, dpi)
        print(f"ICE frame {i + 1}: {100 * mask.mean():.2f}% rebuilt", flush=True)
        return out

    # OEM scene balance of the confirmed frames against the whole roll (all
    # crops first); falls back to the prescan roll balance.
    shifts = None
    if slide is None:
        if oem is not None and _sba_on():
            emit("OEM scene balance", 0.0)
        roll = [crop_at(c, frame_w) for c in roll_centres] if roll_centres else None
        shifts = _sba_shifts([crop(i) for i in range(n)], oem, roll, sba_dpi, scene_dmin)

    for i in range(n):
        emit(f"Exporting frame {i + 1}/{n}" + (" (removing dust)" if ir_rib is not None
                                                 else ""), (i + 1) / max(1, n))
        part = clean(i, np.ascontiguousarray(crop(i)))
        if rot_k:
            part = np.ascontiguousarray(np.rot90(part, k=rot_k))

        if slide is not None:                         # positive, no inversion
            pos = render_slide(part, *slide, flare=flare)
            rendered = (pos >> 8).astype(np.uint8)
        elif oem is not None:                         # OEM colour path
            if shifts is not None:
                bal = (np.ones(3, np.float32), shifts[i])
            else:
                bal = (None if rpd_balance is None else
                       tuple(np.asarray(v, np.float32) for v in rpd_balance))
            pos = render_oem(part, *oem, balance=bal, grey=grey)
            rendered = (pos >> 8).astype(np.uint8)
        else:
            pos = invert_c41(part, base, lut)         # 16-bit sRGB positive
            if grey:
                pos = to_grey(pos)
            rendered = (render_jpeg(pos, str(RPD_PROFILE)) if use_rpd and not grey
                        else (pos >> 8).astype(np.uint8))

        raw_tiff = WORK_DIR / f"frame_{i + 1:02d}_raw.tif"
        pos_tiff = WORK_DIR / f"frame_{i + 1:02d}.tif"
        jpg_path = WORK_DIR / f"frame_{i + 1:02d}.jpg"
        thumb_path = WORK_DIR / f"thumb_{i + 1:02d}.jpg"
        # raw negative (uninverted, 16-bit) — for inverting in another tool
        tifffile.imwrite(str(raw_tiff), part, photometric="rgb",
                         **dx.tiff_kwargs(dx_film))
        tifffile.imwrite(str(pos_tiff), pos,
                         photometric="minisblack" if grey else "rgb",
                         **dx.tiff_kwargs(dx_film))
        Image.fromarray(rendered).save(str(jpg_path), "JPEG", quality=92,
                                       **dx.jpeg_kwargs(dx_film))
        step = max(1, max(rendered.shape[0] // 600, rendered.shape[1] // 600))
        Image.fromarray(rendered[::step, ::step]).save(str(thumb_path),
                                                       "JPEG", quality=85)
        frames.append({"index": i + 1, "raw_tiff": raw_tiff, "tiff": pos_tiff,
                       "jpg": jpg_path, "thumb": thumb_path})

    emit("Done", 1.0)
    return frames


def make_contact_sheet(frames, cols: int = 6, cell_w: int = 480,
                       pad: int = 10, bg=(22, 22, 22), fg=(226, 226, 226)):
    """Tile the rendered frame JPEGs into one labelled contact-sheet image."""
    from PIL import ImageDraw, ImageFont

    items = [f for f in frames if f.get("jpg") and Path(f["jpg"]).exists()]
    if not items:
        return None
    with Image.open(items[0]["jpg"]) as im0:
        aspect = im0.height / im0.width
    cell_h = int(cell_w * aspect)
    label_h = 26
    rows = (len(items) + cols - 1) // cols
    W = cols * cell_w + (cols + 1) * pad
    H = rows * (cell_h + label_h) + (rows + 1) * pad
    sheet = Image.new("RGB", (W, H), bg)
    draw = ImageDraw.Draw(sheet)
    try:
        font = ImageFont.truetype(
            "/System/Library/Fonts/Supplemental/Arial.ttf", 16)
    except OSError:
        font = ImageFont.load_default()
    for k, f in enumerate(items):
        r, c = divmod(k, cols)
        x = pad + c * (cell_w + pad)
        y = pad + r * (cell_h + label_h + pad)
        with Image.open(f["jpg"]) as im:
            thumb = im.convert("RGB").resize((cell_w, cell_h), Image.LANCZOS)
        sheet.paste(thumb, (x, y))
        draw.text((x + 2, y + cell_h + 4), f"Frame {f['index']:02d}",
                  fill=fg, font=font)
    return sheet
