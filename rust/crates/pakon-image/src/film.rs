//! The film a DX code names, and the metadata written into the exports
//! (port of `web/dx.py`). The OEM product-code table
//! (`oem/ansel/common/common-ProdCodeTable.dpi`) gives each code an ISO and,
//! per product, a product-line label. A code the table does not know gives
//! nothing.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

/// Spelling fixes for the table's labels (after title case).
const LABEL_FIXES: &[(&str, &str)] = &[("Illford", "Ilford"), ("Fujicolohr", "Fujicolor"), ("Kodakcolgold", "Kodacolor Gold")];

/// product -> (label, 16 ISO speeds by generation).
#[derive(Debug, Clone, Default)]
pub struct ProdTable(HashMap<i32, (String, [u32; 16])>);

/// A film the table knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Film {
    pub name: Option<String>,
    pub iso: u32,
    pub product: i32,
    pub generation: i32,
}

/// Python's `str.title()`.
fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for ch in s.chars() {
        if ch.is_alphabetic() {
            if prev_cased {
                out.extend(ch.to_lowercase());
            } else {
                out.extend(ch.to_uppercase());
            }
            prev_cased = true;
        } else {
            out.push(ch);
            prev_cased = false;
        }
    }
    out
}

impl ProdTable {
    /// `oem/ansel/common/common-ProdCodeTable.dpi` under the repo.
    pub fn path(repo: &Path) -> std::path::PathBuf {
        repo.join("oem").join("ansel").join("common").join("common-ProdCodeTable.dpi")
    }

    /// The file is a comment `#  <product> <label>` before each row of 16
    /// speeds. A missing file gives an empty table.
    pub fn read(path: &Path) -> ProdTable {
        let Ok(d) = std::fs::read(path) else { return ProdTable::default() };
        ProdTable::parse(&d.iter().map(|&b| b as char).collect::<String>())
    }

    pub fn parse(text: &str) -> ProdTable {
        let mut out = HashMap::new();
        let (mut product, mut label): (Option<i32>, String) = (None, String::new());
        let digits = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit());
        for line in text.lines() {
            let s = line.trim();
            if let Some(rest) = s.strip_prefix('#') {
                let mut words: Vec<&str> = rest.split_whitespace().collect();
                if words.first() == Some(&"ProdCode") {
                    words.remove(0);
                }
                if let Some(w) = words.first().filter(|w| digits(w)) {
                    product = w.parse().ok();
                    label = words[1..].join(" ");
                }
                continue;
            }
            let speeds: Vec<&str> = s.split_whitespace().collect();
            if let Some(p) = product {
                if speeds.len() == 16 && speeds.iter().all(|w| digits(w)) {
                    let v: [u32; 16] = std::array::from_fn(|i| speeds[i].parse().unwrap_or(0));
                    out.insert(p, (label.clone(), v));
                    product = None;
                }
            }
        }
        ProdTable(out)
    }

    /// The film for a code the table knows; `name` is None when the product
    /// has no label.
    pub fn lookup(&self, product: i32, generation: i32) -> Option<Film> {
        let (label, speeds) = self.0.get(&product)?;
        if !(0..16).contains(&generation) || speeds[generation as usize] == 0 {
            return None;
        }
        let iso = speeds[generation as usize];
        let mut name = (!label.is_empty()).then(|| title(label));
        if let Some(n) = name.as_mut() {
            for (wrong, right) in LABEL_FIXES {
                *n = n.replace(wrong, right);
            }
            if !n.split_whitespace().any(|w| w == iso.to_string()) {
                *n = format!("{n} {iso}");
            }
        }
        Some(Film { name, iso, product, generation })
    }
}

/// (product, generation) read during the scan of `raw` (`<raw>.dx.json`).
pub fn code_for_raw(raw: &Path) -> Option<(i32, i32)> {
    let mut p = raw.as_os_str().to_owned();
    p.push(".dx.json");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    let n = |k: &str| v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_str()?.parse().ok())).map(|x| x as i32);
    Some((n("product")?, n("generation")?))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// An XMP packet: the film name as description and keyword, the ISO.
pub fn xmp(film: &Film) -> Vec<u8> {
    let mut props = Vec::new();
    if let Some(n) = &film.name {
        let n = escape(n);
        props.push(format!("<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{n}</rdf:li></rdf:Alt></dc:description>"));
        props.push(format!("<dc:subject><rdf:Bag><rdf:li>{n}</rdf:li></rdf:Bag></dc:subject>"));
    }
    props.push(format!("<exif:ISOSpeedRatings><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></exif:ISOSpeedRatings>", film.iso));
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
         xmlns:exif=\"http://ns.adobe.com/exif/1.0/\">\n    \
         {}\n  \
         </rdf:Description>\n \
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>",
        props.join("\n    ")
    )
    .into_bytes()
}

/// A little-endian EXIF block (`Exif\0\0` + TIFF): ImageDescription in
/// IFD0 when the film has a name, ISOSpeedRatings in the Exif IFD.
pub fn exif(film: &Film) -> Vec<u8> {
    let mut ifd0: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new(); // tag, type, count, value bytes
    if let Some(n) = &film.name {
        let mut b = n.as_bytes().to_vec();
        b.push(0);
        ifd0.push((0x010E, 2, b.len() as u32, b));
    }
    ifd0.push((0x8769, 4, 1, vec![0; 4])); // Exif IFD pointer, patched below
    let iso = (film.iso.min(65535) as u16).to_le_bytes().to_vec();
    let exif_ifd = [(0x8827u16, 3u16, 1u32, iso)];

    let mut t = b"II*\0\x08\0\0\0".to_vec();
    let ifd_len = |n: usize| 2 + 12 * n + 4;
    let ifd0_end = 8 + ifd_len(ifd0.len());
    // IFD0 data (long values) after IFD0, then the Exif IFD
    let mut data = Vec::new();
    let mut entries = Vec::new();
    for (tag, ty, count, v) in &ifd0 {
        let mut e = Vec::new();
        e.extend_from_slice(&tag.to_le_bytes());
        e.extend_from_slice(&ty.to_le_bytes());
        e.extend_from_slice(&count.to_le_bytes());
        if v.len() > 4 {
            e.extend_from_slice(&((ifd0_end + data.len()) as u32).to_le_bytes());
            data.extend_from_slice(v);
            if data.len() % 2 == 1 {
                data.push(0);
            }
        } else {
            let mut v = v.clone();
            v.resize(4, 0);
            e.extend_from_slice(&v);
        }
        entries.push(e);
    }
    let exif_at = (ifd0_end + data.len()) as u32;
    entries.last_mut().unwrap()[8..12].copy_from_slice(&exif_at.to_le_bytes());
    t.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in entries {
        t.extend_from_slice(&e);
    }
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&data);
    t.extend_from_slice(&(exif_ifd.len() as u16).to_le_bytes());
    for (tag, ty, count, v) in &exif_ifd {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&ty.to_le_bytes());
        t.extend_from_slice(&count.to_le_bytes());
        let mut v = v.clone();
        v.resize(4, 0);
        t.extend_from_slice(&v);
    }
    t.extend_from_slice(&0u32.to_le_bytes());
    let mut out = b"Exif\0\0".to_vec();
    out.extend_from_slice(&t);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case_like_python() {
        assert_eq!(title("KODAK GOLD 400"), "Kodak Gold 400");
        assert_eq!(title("FUJI NPH400 PRO"), "Fuji Nph400 Pro");
        assert_eq!(title("3M COLOR"), "3M Color");
    }

    #[test]
    fn table_lookup() {
        let t = ProdTable::read(&ProdTable::path(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."))));
        let name = |p, g| t.lookup(p, g).and_then(|f| f.name);
        assert_eq!(name(96, 1).as_deref(), Some("Kodak Gold 400 Gen 9"));
        assert_eq!(name(78, 13).as_deref(), Some("Kodak Gold Max Vr B&W Pj 400"));
        assert_eq!(name(43, 5).as_deref(), Some("Kodak Max/Supra 800"));
        assert_eq!(name(26, 2).as_deref(), Some("Konica Centuria 100"));
        assert_eq!(name(36, 8).as_deref(), Some("Fujicolor Reala Ace Super Npz 400"));
        assert_eq!(t.lookup(96, 14), None);
        assert_eq!(t.lookup(0, 0), None);
        let f = t.lookup(96, 1).unwrap();
        let want = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\"\n    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    xmlns:exif=\"http://ns.adobe.com/exif/1.0/\">\n    <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">Kodak Gold 400 Gen 9</rdf:li></rdf:Alt></dc:description>\n    <dc:subject><rdf:Bag><rdf:li>Kodak Gold 400 Gen 9</rdf:li></rdf:Bag></dc:subject>\n    <exif:ISOSpeedRatings><rdf:Seq><rdf:li>400</rdf:li></rdf:Seq></exif:ISOSpeedRatings>\n  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";
        assert_eq!(String::from_utf8(xmp(&f)).unwrap(), want);
        assert_eq!(t.lookup(96, 16), None);
    }
}
