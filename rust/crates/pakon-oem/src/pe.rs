//! Just enough of the PE32 format to map a DLL at its preferred base: the
//! headers, the sections and the import address table (what `pefile` gives
//! `tools/oem_*.py`).

/// A PE32 image, ready to map.
#[derive(Debug, Clone)]
pub struct Pe {
    pub image_base: u32,
    pub size_of_image: u32,
    /// The first SizeOfHeaders bytes of the file.
    pub headers: Vec<u8>,
    /// (virtual address, raw data) per section.
    pub sections: Vec<(u32, Vec<u8>)>,
    /// Every imported function in descriptor order: (name, absolute IAT
    /// slot address). Ordinal imports are named `ord<N>`.
    pub imports: Vec<(String, u32)>,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, String> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| format!("PE truncated at {o:#x}"))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("PE truncated at {o:#x}"))
}

impl Pe {
    pub fn parse(d: &[u8]) -> Result<Pe, String> {
        if d.get(0..2) != Some(b"MZ") {
            return Err("not an MZ file".into());
        }
        let pe = u32_at(d, 0x3C)? as usize;
        if d.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err("no PE signature".into());
        }
        let coff = pe + 4;
        let nsec = u16_at(d, coff + 2)? as usize;
        let opt_size = u16_at(d, coff + 16)? as usize;
        let opt = coff + 20;
        if u16_at(d, opt)? != 0x10B {
            return Err("not a PE32 image".into());
        }
        let image_base = u32_at(d, opt + 28)?;
        let size_of_image = u32_at(d, opt + 56)?;
        let size_of_headers = u32_at(d, opt + 60)? as usize;
        let import_rva = u32_at(d, opt + 104)?;

        let mut sec_hdrs = Vec::new();
        let mut sections = Vec::new();
        for i in 0..nsec {
            let s = opt + opt_size + 40 * i;
            let (vsize, va) = (u32_at(d, s + 8)?, u32_at(d, s + 12)?);
            let (rsize, rptr) = (u32_at(d, s + 16)? as usize, u32_at(d, s + 20)? as usize);
            let end = (rptr + rsize).min(d.len());
            sections.push((va, d.get(rptr.min(end)..end).unwrap_or_default().to_vec()));
            sec_hdrs.push((va, vsize.max(rsize as u32), rptr));
        }
        // file offset of an RVA
        let off = |rva: u32| -> Result<usize, String> {
            sec_hdrs
                .iter()
                .find(|(va, sz, _)| rva >= *va && rva < va + sz)
                .map(|(va, _, rp)| (rva - va) as usize + rp)
                .ok_or_else(|| format!("RVA {rva:#x} outside the sections"))
        };
        let cstr = |o: usize| -> String { d[o..].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect() };

        let mut imports = Vec::new();
        if import_rva != 0 {
            let mut desc = off(import_rva)?;
            loop {
                let (oft, name_rva, ft) = (u32_at(d, desc)?, u32_at(d, desc + 12)?, u32_at(d, desc + 16)?);
                if oft == 0 && name_rva == 0 && ft == 0 {
                    break;
                }
                let mut k = 0usize;
                let lookup = if oft != 0 { oft } else { ft };
                let lt = off(lookup)?;
                loop {
                    let thunk = u32_at(d, lt + 4 * k)?;
                    if thunk == 0 {
                        break;
                    }
                    let name = if thunk & 0x8000_0000 != 0 { format!("ord{}", thunk & 0xFFFF) } else { cstr(off(thunk)? + 2) };
                    imports.push((name, image_base + ft + 4 * k as u32));
                    k += 1;
                }
                desc += 20;
            }
        }
        Ok(Pe { image_base, size_of_image, headers: d[..size_of_headers.min(d.len())].to_vec(), sections, imports })
    }

    pub fn read(path: impl AsRef<std::path::Path>) -> Result<Pe, String> {
        let p = path.as_ref();
        Pe::parse(&std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?).map_err(|e| format!("{}: {e}", p.display()))
    }
}

pub fn align(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}
