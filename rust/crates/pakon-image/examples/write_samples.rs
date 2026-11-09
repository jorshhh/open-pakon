//! Writes sample export files into DIR for checking with other readers:
//! cargo run -p pakon-image --example write_samples DIR
use ndarray::{Array2, Array3};
use pakon_image::film::Film;
use pakon_image::output::{self, Picture16};
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("DIR"));
    let a = Array3::<u16>::from_shape_fn((300, 450, 3), |(y, x, c)| (y * 200 + x * 100 + c * 7) as u16);
    let f = Film { name: Some("Kodak Gold Max Vr B&W Pj 400".into()), iso: 400, product: 78, generation: 13 };
    output::write_tiff16(&dir.join("s_rgb.tif"), &Picture16::Rgb(a.view()), Some(&f)).unwrap();
    let g = Array2::<u16>::from_shape_fn((300, 450), |(y, x)| (y * 100 + x) as u16);
    output::write_tiff16(&dir.join("s_grey.tif"), &Picture16::Grey(g.view()), None).unwrap();
    let r = output::to_u8(&Picture16::Rgb(a.view()));
    output::write_jpeg(&dir.join("s.jpg"), r.view(), 92, Some(&f)).unwrap();
    let paths: Vec<PathBuf> = (0..8).map(|i| { let p = dir.join(format!("f{i}.jpg")); output::write_jpeg(&p, r.view(), 92, None).unwrap(); p }).collect();
    let items: Vec<(usize, &std::path::Path)> = paths.iter().enumerate().map(|(i, p)| (i + 1, p.as_path())).collect();
    output::write_contact_sheet(&dir.join("sheet.jpg"), &items).unwrap();
}
