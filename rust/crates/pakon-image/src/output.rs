//! Export files (the writers in `web/decode.py` and `make_contact_sheet`):
//! 16-bit TIFFs (raw negative and positive) and 8-bit JPEGs with the film's
//! DX metadata, thumbnails, the preview strip and the contact sheet.

use std::io::Write;
use std::path::Path;

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use ndarray::{s, Array3, ArrayView2, ArrayView3, Axis};

use crate::film::{self, Film};

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

/// Samples of a 16-bit picture: (H, W, 3) RGB or (H, W) grey.
pub enum Picture16<'a> {
    Rgb(ArrayView3<'a, u16>),
    Grey(ArrayView2<'a, u16>),
}

/// An uncompressed baseline TIFF, 16 bits per sample, one strip, little
/// endian; with a film, its name as ImageDescription and the XMP packet
/// (tag 700), as `dx.tiff_kwargs` gives tifffile.
pub fn tiff16_bytes(pic: &Picture16, film: Option<&Film>) -> Vec<u8> {
    let (h, w, spp, photometric) = match pic {
        Picture16::Rgb(a) => (a.len_of(Axis(0)), a.len_of(Axis(1)), 3u16, 2u16),
        Picture16::Grey(a) => (a.nrows(), a.ncols(), 1, 1),
    };
    let mut pixels = Vec::with_capacity(2 * h * w * spp as usize);
    match pic {
        Picture16::Rgb(a) => a.iter().for_each(|v| pixels.extend_from_slice(&v.to_le_bytes())),
        Picture16::Grey(a) => a.iter().for_each(|v| pixels.extend_from_slice(&v.to_le_bytes())),
    }
    // ndarray iterates in logical (row-major) order whatever the layout

    // (tag, type, count, value bytes); types 2 ASCII, 3 SHORT, 4 LONG, 7 UNDEFINED / 1 BYTE
    let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    let short = |v: u16| v.to_le_bytes().to_vec();
    let long = |v: u32| v.to_le_bytes().to_vec();
    tags.push((256, 4, 1, long(w as u32)));
    tags.push((257, 4, 1, long(h as u32)));
    tags.push((258, 3, spp as u32, (0..spp).flat_map(|_| 16u16.to_le_bytes()).collect()));
    tags.push((259, 3, 1, short(1)));
    tags.push((262, 3, 1, short(photometric)));
    if let Some(name) = film.and_then(|f| f.name.as_ref()) {
        let mut b = name.as_bytes().to_vec();
        b.push(0);
        tags.push((270, 2, b.len() as u32, b));
    }
    tags.push((273, 4, 1, long(0))); // StripOffsets, patched below
    tags.push((277, 3, 1, short(spp)));
    tags.push((278, 4, 1, long(h as u32)));
    tags.push((279, 4, 1, long(pixels.len() as u32)));
    tags.push((284, 3, 1, short(1)));
    if let Some(f) = film {
        let x = film::xmp(f);
        tags.push((700, 1, x.len() as u32, x));
    }

    let ifd_at = 8usize;
    let ifd_len = 2 + 12 * tags.len() + 4;
    let mut extra = Vec::new(); // values longer than 4 bytes
    let extra_at = ifd_at + ifd_len;
    let mut entries = Vec::new();
    for (tag, ty, count, v) in &tags {
        let mut e = Vec::with_capacity(12);
        e.extend_from_slice(&tag.to_le_bytes());
        e.extend_from_slice(&ty.to_le_bytes());
        e.extend_from_slice(&count.to_le_bytes());
        if v.len() > 4 {
            e.extend_from_slice(&((extra_at + extra.len()) as u32).to_le_bytes());
            extra.extend_from_slice(v);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        } else {
            let mut v = v.clone();
            v.resize(4, 0);
            e.extend_from_slice(&v);
        }
        entries.push((*tag, e));
    }
    let strip_at = (extra_at + extra.len()) as u32;
    for (tag, e) in entries.iter_mut() {
        if *tag == 273 {
            e[8..12].copy_from_slice(&strip_at.to_le_bytes());
        }
    }
    let mut out = Vec::with_capacity(strip_at as usize + pixels.len());
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&(ifd_at as u32).to_le_bytes());
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    for (_, e) in entries {
        out.extend_from_slice(&e);
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&extra);
    out.extend_from_slice(&pixels);
    out
}

pub fn write_tiff16(path: &Path, pic: &Picture16, film: Option<&Film>) -> Result<(), String> {
    let mut f = std::fs::File::create(path).map_err(io_err(path))?;
    f.write_all(&tiff16_bytes(pic, film)).map_err(io_err(path))
}

/// An 8-bit (H, W, 3) or (H, W, 1) picture as JPEG at `quality` (4:2:0
/// like Pillow), with the film's EXIF and XMP.
pub fn jpeg_bytes(img: ArrayView3<u8>, quality: u8, film: Option<&Film>) -> Result<Vec<u8>, String> {
    let (h, w, c) = img.dim();
    if w > 65535 || h > 65535 {
        return Err(format!("{w}x{h} is too large for JPEG"));
    }
    let data: Vec<u8> = img.iter().copied().collect();
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, quality);
    enc.set_sampling_factor(SamplingFactor::F_2_2);
    if let Some(f) = film {
        enc.add_app_segment(1, &film::exif(f)).map_err(|e| format!("{e:?}"))?;
        let mut x = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        x.extend_from_slice(&film::xmp(f));
        enc.add_app_segment(1, &x).map_err(|e| format!("{e:?}"))?;
    }
    let ct = if c == 1 { ColorType::Luma } else { ColorType::Rgb };
    enc.encode(&data, w as u16, h as u16, ct).map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

pub fn write_jpeg(path: &Path, img: ArrayView3<u8>, quality: u8, film: Option<&Film>) -> Result<(), String> {
    std::fs::write(path, jpeg_bytes(img, quality, film)?).map_err(io_err(path))
}

/// 16-bit positive -> 8-bit (`pos >> 8`), (H, W, 3) or (H, W, 1).
pub fn to_u8(pic: &Picture16) -> Array3<u8> {
    match pic {
        Picture16::Rgb(a) => a.mapv(|v| (v >> 8) as u8),
        Picture16::Grey(a) => a.mapv(|v| (v >> 8) as u8).insert_axis(Axis(2)),
    }
}

/// The export thumbnail: every `step`-th pixel, `step` keeping both sides
/// near 600 (`rendered[::step, ::step]`).
pub fn thumbnail(img: ArrayView3<u8>) -> Array3<u8> {
    let (h, w, _) = img.dim();
    let step = (h / 600).max(w / 600).max(1);
    img.slice(s![..;step, ..;step, ..]).to_owned()
}

/// The preview strip: the ribbon's long axis horizontal (`np.rot90(k=1)`).
pub fn preview_strip(img: ArrayView3<u8>) -> Array3<u8> {
    let mut v = img;
    v.swap_axes(0, 1);
    v.invert_axis(Axis(0));
    v.as_standard_layout().into_owned()
}

/// A tiny 5x7 font for the contact-sheet labels when no TrueType font is
/// found ("Frame 0".."9").
fn bitmap_glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch {
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'm' => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x15, 0x15],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ' ' => [0; 7],
        _ => return None,
    })
}

const FONTS: &[&str] = &["/System/Library/Fonts/Supplemental/Arial.ttf", "C:\\Windows\\Fonts\\arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"];

/// Draw `text` at (x, y) top-left onto an RGB canvas, 16 px.
fn draw_text(canvas: &mut image::RgbImage, x: i32, y: i32, text: &str, fg: [u8; 3], font: Option<&ab_glyph::FontVec>) {
    use ab_glyph::{Font, ScaleFont};
    let (cw, ch) = (canvas.width() as i32, canvas.height() as i32);
    let mut blend = |px: i32, py: i32, a: f32| {
        if px >= 0 && py >= 0 && px < cw && py < ch && a > 0.0 {
            let p = canvas.get_pixel_mut(px as u32, py as u32);
            for c in 0..3 {
                p[c] = (p[c] as f32 * (1.0 - a) + fg[c] as f32 * a).round() as u8;
            }
        }
    };
    match font {
        Some(f) => {
            let sf = f.as_scaled(ab_glyph::PxScale::from(16.0));
            let mut pen = x as f32;
            let base = y as f32 + sf.ascent();
            for c in text.chars() {
                let mut g = sf.scaled_glyph(c);
                g.position = ab_glyph::point(pen, base);
                pen += sf.h_advance(g.id);
                if let Some(o) = f.outline_glyph(g) {
                    let b = o.px_bounds();
                    o.draw(|gx, gy, a| blend(b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, a));
                }
            }
        }
        None => {
            let mut pen = x;
            for c in text.chars() {
                if let Some(rows) = bitmap_glyph(c) {
                    for (ry, bits) in rows.iter().enumerate() {
                        for rx in 0..5 {
                            if bits >> (4 - rx) & 1 == 1 {
                                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                                    blend(pen + 2 * rx + dx, y + 2 * ry as i32 + dy, 1.0);
                                }
                            }
                        }
                    }
                }
                pen += 12;
            }
        }
    }
}

/// Tile the rendered frame JPEGs (index, path) into one labelled sheet
/// (`make_contact_sheet`): 6 columns of 480 px cells at the first frame's
/// aspect, Lanczos-resized, "Frame NN" under each.
pub fn contact_sheet(frames: &[(usize, &Path)]) -> Result<Option<image::RgbImage>, String> {
    let (cols, cell_w, pad, label_h) = (6u32, 480u32, 10u32, 26u32);
    let (bg, fg) = ([22u8, 22, 22], [226u8, 226, 226]);
    let items: Vec<&(usize, &Path)> = frames.iter().filter(|(_, p)| p.is_file()).collect();
    let Some(first) = items.first() else { return Ok(None) };
    let open = |p: &Path| image::open(p).map(|i| i.to_rgb8()).map_err(|e| format!("{}: {e}", p.display()));
    let im0 = open(first.1)?;
    let cell_h = (cell_w as f64 * (im0.height() as f64 / im0.width() as f64)) as u32;
    let rows = (items.len() as u32).div_ceil(cols);
    let (sw, sh) = (cols * cell_w + (cols + 1) * pad, rows * (cell_h + label_h) + (rows + 1) * pad);
    let mut sheet = image::RgbImage::from_pixel(sw, sh, image::Rgb(bg));
    let font = FONTS.iter().find_map(|p| ab_glyph::FontVec::try_from_vec(std::fs::read(p).ok()?).ok());
    for (k, (index, path)) in items.iter().enumerate() {
        let (r, c) = (k as u32 / cols, k as u32 % cols);
        let x = pad + c * (cell_w + pad);
        let y = pad + r * (cell_h + label_h + pad);
        let im = if k == 0 { im0.clone() } else { open(path)? };
        let thumb = image::imageops::resize(&im, cell_w, cell_h, image::imageops::FilterType::Lanczos3);
        image::imageops::replace(&mut sheet, &thumb, x as i64, y as i64);
        draw_text(&mut sheet, x as i32 + 2, (y + cell_h + 4) as i32, &format!("Frame {index:02}"), fg, font.as_ref());
    }
    Ok(Some(sheet))
}

/// `contact_sheet` saved as a quality-90 JPEG; false when there are no frames.
pub fn write_contact_sheet(path: &Path, frames: &[(usize, &Path)]) -> Result<bool, String> {
    let Some(sheet) = contact_sheet(frames)? else { return Ok(false) };
    let (w, h) = sheet.dimensions();
    let a = Array3::from_shape_vec((h as usize, w as usize, 3), sheet.into_raw()).map_err(|e| e.to_string())?;
    write_jpeg(path, a.view(), 90, None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array2;

    #[test]
    fn tiff_layout() {
        let a = Array3::<u16>::from_shape_fn((3, 4, 3), |(y, x, c)| (y * 100 + x * 10 + c) as u16);
        let f = Film { name: Some("Kodak Gold 400 Gen 9".into()), iso: 400, product: 96, generation: 1 };
        let b = tiff16_bytes(&Picture16::Rgb(a.view()), Some(&f));
        assert_eq!(&b[..4], b"II*\0");
        let n = u16::from_le_bytes([b[8], b[9]]) as usize;
        let tag = |t: u16| {
            (0..n).map(|i| 10 + 12 * i).find(|&e| u16::from_le_bytes([b[e], b[e + 1]]) == t).map(|e| u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()))
        };
        let off = tag(273).unwrap() as usize;
        assert_eq!(tag(279), Some(72));
        assert_eq!(u16::from_le_bytes([b[off + 2 * 7], b[off + 2 * 7 + 1]]), 21); // (0, 2, 1)
        assert_eq!(b.len(), off + 72);
        assert!(tag(700).is_some() && tag(270).is_some());
        let g = Array2::<u16>::zeros((2, 2));
        let b = tiff16_bytes(&Picture16::Grey(g.view()), None);
        assert_eq!(b.len(), 8 + 2 + 12 * 10 + 4 + 8);
    }

    #[test]
    fn jpeg_roundtrip_and_strip() {
        let a = Array3::<u8>::from_shape_fn((40, 60, 3), |(y, x, c)| (y * 3 + x * 2 + c * 40) as u8);
        let f = Film { name: Some("Kodak Gold 400 Gen 9".into()), iso: 400, product: 96, generation: 1 };
        let j = jpeg_bytes(a.view(), 92, Some(&f)).unwrap();
        let im = image::load_from_memory(&j).unwrap().to_rgb8();
        assert_eq!(im.dimensions(), (60, 40));
        assert!(j.windows(4).any(|w| w == b"Exif"));
        let p = preview_strip(a.view());
        assert_eq!(p.dim(), (60, 40, 3));
        assert_eq!(p[(0, 0, 0)], a[(0, 59, 0)]); // rot90: out[i, j] = a[j, W - 1 - i]
        assert_eq!(thumbnail(a.view()).dim(), (40, 60, 3));
    }
}
