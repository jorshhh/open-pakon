//! Strip mode: several cut strips scanned into one raw (`<raw>.strips.json`
//! from the scan). The strips are put in film order from their DX codes,
//! and the pictures are found in each strip on its own, so the open gate
//! between strips is never framed.

use std::io::Write;
use std::ops::Range;
use std::path::Path;

use ndarray::{s, ArrayView2, ArrayView3, Axis};
use pakon_image::framing::{self, Format};
use pakon_proto::dx;
use serde::{Deserialize, Serialize};

/// One strip on the ribbon the app works on: rows `[start, end)` and its
/// place in the feed (1 = fed first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StripInfo {
    pub start: usize,
    pub end: usize,
    pub fed: usize,
}

#[derive(Debug, Deserialize)]
struct FileStrip {
    start: usize,
    /// The first raw row with film in it (files before 1.0.5 may lack it).
    #[serde(default)]
    film: Option<usize>,
    end: usize,
    #[serde(default)]
    dx: Vec<u8>,
}

/// Open gate kept ahead of each strip's film, as the scan keeps between
/// strips (pakon-scan's `STRIP_LEAD_ROWS`).
pub const LEAD_ROWS: usize = 200;

/// One strip as the scan recorded it: raw rows, first film row, DX codes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fed {
    pub rows: Range<usize>,
    pub film: Option<usize>,
    pub dx: Vec<u8>,
}

/// Where a strip's raw rows sit on a ribbon.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    fed: usize,
    raw: Range<usize>,
    at: usize,
}

/// From the ribbon an older build made of the same raw (leader kept) to
/// this one, so boxes saved on it still land on the same pictures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowMap {
    /// Rows of the older ribbon.
    pub old_rows: usize,
    old: Vec<Placed>,
    new: Vec<Placed>,
}

impl RowMap {
    /// The row on this ribbon for `row` on the older one. A row in a
    /// dropped leader goes to the start of its strip.
    pub fn map(&self, row: usize) -> usize {
        let Some(o) = self.old.iter().find(|p| row < p.at + p.raw.len()).or(self.old.last()) else {
            return row;
        };
        let raw = o.raw.start + row.saturating_sub(o.at).min(o.raw.len().saturating_sub(1));
        match self.new.iter().find(|n| n.fed == o.fed) {
            Some(n) => n.at + raw.saturating_sub(n.raw.start).min(n.raw.len().saturating_sub(1)),
            None => row,
        }
    }
}

#[derive(Debug, Deserialize)]
struct StripsFile {
    version: u32,
    row_samples: usize,
    strips: Vec<FileStrip>,
}

/// The strips of a raw, in film order: the raw row ranges to copy, in
/// that order, and the strips on the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Raw rows in film order (the ribbon is rebuilt from these when
    /// `reordered`).
    pub ranges: Vec<Range<usize>>,
    pub strips: Vec<StripInfo>,
    /// The DX codes put the strips in another order than they were fed.
    pub reordered: bool,
    /// Leader before a strip's film was dropped.
    pub trimmed: bool,
    /// Set when trimmed: from the ribbon older builds made (leader kept).
    pub moved: Option<RowMap>,
}

impl Plan {
    /// The ribbon is not the raw's rows as they are: write it from `ranges`.
    pub fn repacked(&self) -> bool {
        self.reordered || self.trimmed
    }
}

/// `<raw>.strips.json` for a ribbon of `rows` rows decoded with
/// `row_samples` samples per row; None for a full-roll raw or a file that
/// does not match the raw.
pub fn plan_for_raw(raw: &Path, rows: usize, row_samples: usize) -> Option<Plan> {
    let mut p = raw.as_os_str().to_owned();
    p.push(".strips.json");
    let text = std::fs::read_to_string(&p).ok()?;
    let f: StripsFile = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("strips.json not used: {e}");
            return None;
        }
    };
    if f.version != 1 || f.row_samples != row_samples {
        eprintln!("strips.json not used: version {}, {} samples per row (the raw has {row_samples})", f.version, f.row_samples);
        return None;
    }
    let fed: Vec<Fed> = f.strips.iter().map(|s| Fed { rows: s.start..s.end, film: s.film, dx: s.dx.clone() }).collect();
    plan(&fed, rows)
}

/// The plan from each strip as fed, in feed order. Each strip starts
/// `LEAD_ROWS` before its film: the first one otherwise carries everything
/// read from the motor start until the film reached the gate.
pub fn plan(strips: &[Fed], rows: usize) -> Option<Plan> {
    let mut out = layout(strips, rows, true)?;
    if out.trimmed {
        let old = layout(strips, rows, false)?;
        let placed = |p: &Plan| -> Vec<Placed> {
            p.ranges.iter().zip(&p.strips).map(|(r, s)| Placed { fed: s.fed, raw: r.clone(), at: s.start }).collect()
        };
        let old_rows = if old.reordered { old.strips.last().map_or(0, |s| s.end) } else { rows };
        out.moved = Some(RowMap { old_rows, old: placed(&old), new: placed(&out) });
    }
    Some(out)
}

fn layout(strips: &[Fed], rows: usize, trim: bool) -> Option<Plan> {
    // clamp to the ribbon; drop what is left empty
    let mut trimmed = false;
    let kept: Vec<(usize, Range<usize>, Vec<u8>)> = strips
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut start = f.rows.start;
            if let Some(film) = f.film.filter(|_| trim) {
                let from = film.saturating_sub(LEAD_ROWS).min(f.rows.end);
                if from > start {
                    start = from;
                    trimmed = true;
                }
            }
            (i, start.min(rows)..f.rows.end.min(rows), f.dx.clone())
        })
        .filter(|(_, r, _)| r.end > r.start)
        .collect();
    if kept.is_empty() {
        return None;
    }
    let codes: Vec<Vec<u8>> = kept.iter().map(|k| k.2.clone()).collect();
    let (order, reordered) = dx::strip_order(&codes);
    let mut out = Plan { ranges: Vec::new(), strips: Vec::new(), reordered, trimmed, moved: None };
    let mut at = 0;
    for &k in &order {
        let (fed, r, _) = &kept[k];
        // unmoved, the ribbon keeps its rows; moved or trimmed, the strips
        // are packed
        let start = if out.repacked() { at } else { r.start };
        let len = r.end - r.start;
        out.ranges.push(r.clone());
        out.strips.push(StripInfo { start, end: start + len, fed: fed + 1 });
        at = start + len;
    }
    Some(out)
}

/// The .npy v1 header for a little-endian u16 array of `shape`.
fn npy_header(shape: &[usize]) -> Vec<u8> {
    let dims: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    let shape = if dims.len() == 1 { format!("({},)", dims[0]) } else { format!("({})", dims.join(", ")) };
    let mut dict = format!("{{'descr': '<u2', 'fortran_order': False, 'shape': {shape}, }}");
    // magic (6) + version (2) + length (2) + dict + '\n', to a multiple of 64
    let total = (10 + dict.len() + 1).div_ceil(64) * 64;
    dict.push_str(&" ".repeat(total - 10 - dict.len() - 1));
    dict.push('\n');
    let mut h = b"\x93NUMPY\x01\x00".to_vec();
    h.extend_from_slice(&(dict.len() as u16).to_le_bytes());
    h.extend_from_slice(dict.as_bytes());
    h
}

fn write_u16s<'a>(w: &mut impl Write, it: impl Iterator<Item = &'a u16>) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1 << 16);
    for &v in it {
        buf.extend_from_slice(&v.to_le_bytes());
        if buf.len() >= 1 << 16 {
            w.write_all(&buf)?;
            buf.clear();
        }
    }
    w.write_all(&buf)
}

/// Write the rows `ranges` of `a`, one after another, as a .npy file,
/// without a copy of the array in memory.
pub fn write_rows3(path: &Path, a: ArrayView3<u16>, ranges: &[Range<usize>]) -> Result<(), String> {
    let (_, c, k) = a.dim();
    let rows: usize = ranges.iter().map(|r| r.len()).sum();
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut w = std::io::BufWriter::new(std::fs::File::create(path).map_err(err)?);
    w.write_all(&npy_header(&[rows, c, k])).map_err(err)?;
    for r in ranges {
        let part = a.slice(s![r.clone(), .., ..]);
        match part.as_slice() {
            Some(sl) => write_u16s(&mut w, sl.iter()),
            None => write_u16s(&mut w, part.iter()),
        }
        .map_err(err)?;
    }
    w.flush().map_err(err)
}

/// [`write_rows3`] for a 2-D plane (the IR plane).
pub fn write_rows2(path: &Path, a: ArrayView2<u16>, ranges: &[Range<usize>]) -> Result<(), String> {
    let (_, c) = a.dim();
    let rows: usize = ranges.iter().map(|r| r.len()).sum();
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut w = std::io::BufWriter::new(std::fs::File::create(path).map_err(err)?);
    w.write_all(&npy_header(&[rows, c])).map_err(err)?;
    for r in ranges {
        let part = a.slice(s![r.clone(), ..]);
        match part.as_slice() {
            Some(sl) => write_u16s(&mut w, sl.iter()),
            None => write_u16s(&mut w, part.iter()),
        }
        .map_err(err)?;
    }
    w.flush().map_err(err)
}

/// The OEM framing run on each strip on its own: picture starts (ribbon
/// rows, in strip order), length and pitch. Without strips, the whole
/// ribbon as one.
pub fn frame_strips(rgb: ArrayView3<u16>, strips: &[StripInfo], positive: bool, fmt: Format) -> (Vec<usize>, usize, usize) {
    if strips.is_empty() {
        return framing::frame_grid(rgb, positive, fmt);
    }
    let rows = rgb.len_of(Axis(0));
    let (_, length, pitch) = framing::geometry(rgb.len_of(Axis(1)), fmt);
    let mut starts = Vec::new();
    for st in strips {
        let r = st.start.min(rows)..st.end.min(rows);
        // a strip shorter than a picture holds none
        if r.len() < length {
            continue;
        }
        let (s, _, _) = framing::frame_grid(rgb.slice(s![r.clone(), .., ..]), positive, fmt);
        starts.extend(s.into_iter().map(|v| v + r.start));
    }
    (starts, length, pitch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::{Array2, Array3};
    use ndarray_npy::ReadNpyExt;

    fn fed(rows: Range<usize>, film: Option<usize>, dx: Vec<u8>) -> Fed {
        Fed { rows, film, dx }
    }

    #[test]
    fn plan_keeps_or_packs_the_strips() {
        let raw = |f: u8| f * 2;
        // in order: rows unchanged
        let p = plan(&[fed(0..100, None, vec![raw(1)]), fed(100..250, None, vec![raw(5)])], 1000).unwrap();
        assert!(!p.repacked());
        assert_eq!(p.strips, vec![StripInfo { start: 0, end: 100, fed: 1 }, StripInfo { start: 100, end: 250, fed: 2 }]);
        // fed 2, 1: strip 2 first, packed from row 0
        let p = plan(&[fed(0..100, None, vec![raw(5)]), fed(100..250, None, vec![raw(1)])], 1000).unwrap();
        assert!(p.reordered);
        assert_eq!(p.ranges, vec![100..250, 0..100]);
        assert_eq!(p.strips, vec![StripInfo { start: 0, end: 150, fed: 2 }, StripInfo { start: 150, end: 250, fed: 1 }]);
        assert!(p.moved.is_none());
        // clamped to the ribbon; an empty strip dropped
        let p = plan(&[fed(0..100, None, vec![]), fed(2000..2100, None, vec![])], 1000).unwrap();
        assert_eq!(p.strips.len(), 1);
        assert!(plan(&[], 10).is_none());
    }

    /// The scan of 2026-11-19: strip 1 read from the motor start, film at
    /// row 7715; strip 2 already 200 rows ahead of its film.
    #[test]
    fn plan_drops_the_leader_before_the_film() {
        let p = plan(&[fed(0..20683, Some(7715), vec![]), fed(20683..33824, Some(20883), vec![])], 33808).unwrap();
        assert!(p.trimmed && !p.reordered && p.repacked());
        assert_eq!(p.ranges, vec![7515..20683, 20683..33808]);
        assert_eq!(p.strips, vec![StripInfo { start: 0, end: 13168, fed: 1 }, StripInfo { start: 13168, end: 26293, fed: 2 }]);
        // boxes saved on the old ribbon (raw rows) move with the pictures
        let m = p.moved.unwrap();
        assert_eq!(m.old_rows, 33808);
        assert_eq!(m.map(9300), 9300 - 7515);
        assert_eq!(m.map(25711), 25711 - 7515);
        assert_eq!(m.map(100), 0, "in the leader: the strip's start");
        // no film row (an older strips.json): as before
        let p = plan(&[fed(0..20683, None, vec![])], 33808).unwrap();
        assert!(!p.repacked() && p.moved.is_none());
    }

    /// Trimmed and reordered: the older ribbon was packed too.
    #[test]
    fn row_map_across_a_reorder() {
        let raw = |f: u8| f * 2;
        let p = plan(&[fed(0..1000, Some(500), vec![raw(5)]), fed(1000..1600, Some(1200), vec![raw(1)])], 2000).unwrap();
        // new: strip 2 rows 1000..1600 at 0, strip 1 rows 300..1000 at 600
        assert_eq!(p.ranges, vec![1000..1600, 300..1000]);
        let m = p.moved.unwrap();
        // old: strip 2 at 0 (600 rows), strip 1 rows 0..1000 at 600
        assert_eq!(m.old_rows, 1600);
        assert_eq!(m.map(50), 50, "strip 2 did not move");
        assert_eq!(m.map(600 + 700), 600 + 400, "strip 1 row 700");
        assert_eq!(m.map(600 + 10), 600, "strip 1 leader");
    }

    /// Two cut strips of two pictures at Base 4 width (picture 1500 rows,
    /// pitch 1603): open gate, film base, dense pictures, film base, gate.
    #[test]
    fn frames_found_per_strip_not_in_the_gap() {
        let (gate, base, pic) = (60000u16, 40000u16, 15000u16);
        let mut rows: Vec<u16> = Vec::new();
        let mut strips = Vec::new();
        let mut pics = Vec::new();
        for _ in 0..2 {
            let start = rows.len();
            rows.extend(std::iter::repeat_n(gate, 200));
            rows.extend(std::iter::repeat_n(base, 100));
            for k in 0..2 {
                pics.push(rows.len());
                rows.extend(std::iter::repeat_n(pic, 1500));
                rows.extend(std::iter::repeat_n(base, if k == 0 { 103 } else { 100 }));
            }
            rows.extend(std::iter::repeat_n(gate, 150));
            strips.push(StripInfo { start, end: rows.len(), fed: strips.len() + 1 });
        }
        let a = Array3::from_shape_fn((rows.len(), 1000, 3), |(r, _, _)| rows[r]);
        let (starts, len, _) = frame_strips(a.view(), &strips, false, Format::Full);
        assert_eq!(len, 1500);
        assert_eq!(starts.len(), 4, "{starts:?}");
        for (s, p) in starts.iter().zip(&pics) {
            assert!(s.abs_diff(*p) <= 8, "picture at {p}, framed at {s}");
        }
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn rows_written_in_order_read_back() {
        let a = Array3::from_shape_fn((6, 2, 3), |(r, c, k)| (r * 100 + c * 10 + k) as u16);
        let dir = std::env::temp_dir().join(format!("pakon-strips-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("r.npy");
        write_rows3(&p, a.view(), &[4..6, 0..2]).unwrap();
        let b = Array3::<u16>::read_npy(std::fs::File::open(&p).unwrap()).unwrap();
        assert_eq!(b.dim(), (4, 2, 3));
        assert_eq!(b.slice(s![0..2, .., ..]), a.slice(s![4..6, .., ..]));
        assert_eq!(b.slice(s![2..4, .., ..]), a.slice(s![0..2, .., ..]));
        let ir = Array2::from_shape_fn((6, 4), |(r, c)| (r * 10 + c) as u16);
        write_rows2(&p, ir.view(), &[3..6]).unwrap();
        let b = Array2::<u16>::read_npy(std::fs::File::open(&p).unwrap()).unwrap();
        assert_eq!(b, ir.slice(s![3..6, ..]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
