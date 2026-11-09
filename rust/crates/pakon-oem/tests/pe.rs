use pakon_oem::pe::Pe;

#[test]
fn oem_dlls_parse() {
    let oem = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oem");
    for f in ["DMLDICELib.dll", "PakonIMAu.dll"] {
        let pe = Pe::read(oem.join(f)).unwrap();
        println!("{f} {:#x} {} {:?} {:?} {} {:?}", pe.image_base, pe.imports.len(), &pe.imports[..2], pe.imports.last(), pe.headers.len(),
            pe.sections.iter().map(|(va, d)| (format!("{va:#x}"), d.len())).collect::<Vec<_>>());
    }
}
