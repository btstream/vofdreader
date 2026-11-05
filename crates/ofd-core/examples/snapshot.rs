//! Raster snapshots of the default backend, to compare across dependency or
//! renderer changes.
//!
//! cargo run --release -p ofd-core --example snapshot -- save  DIR DPI FILE...
//! cargo run --release -p ofd-core --example snapshot -- check DIR DPI FILE...
//!
//! `check` reports per page the peak single-channel difference and the share
//! of pixels whose largest channel difference exceeds 32 (fail at >= 1%).

use ofd_core::{OfdDocument, PageRenderer};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, dir, dpi, files @ ..] = args.as_slice() else {
        eprintln!("usage: snapshot save|check DIR DPI FILE...");
        std::process::exit(2);
    };
    let dpi: f32 = dpi.parse().expect("DPI");
    let save = match mode.as_str() {
        "save" => true,
        "check" => false,
        _ => panic!("mode must be save or check"),
    };
    std::fs::create_dir_all(dir).unwrap();
    let (mut pages, mut failing, mut worst) = (0, 0, 0u8);
    for file in files {
        let Ok(doc) = OfdDocument::open(file) else {
            println!("{file}: open failed");
            continue;
        };
        let renderer = PageRenderer::new(&doc);
        let stem = std::path::Path::new(file)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        for page in 0..doc.page_count().min(20) {
            let Ok(raster) = renderer.rasterize(&doc, page, dpi) else {
                println!("{stem} p{page}: render failed");
                continue;
            };
            let path = format!("{dir}/{stem}-p{page}-{dpi}.bgra");
            let mut bytes = Vec::with_capacity(raster.bgra.len() + 8);
            bytes.extend_from_slice(&raster.width.to_le_bytes());
            bytes.extend_from_slice(&raster.height.to_le_bytes());
            bytes.extend_from_slice(&raster.bgra);
            pages += 1;
            if save {
                std::fs::write(&path, bytes).unwrap();
                continue;
            }
            let Ok(old) = std::fs::read(&path) else {
                println!("{stem} p{page}: no snapshot");
                failing += 1;
                continue;
            };
            if old[..8] != bytes[..8] {
                println!("{stem} p{page}: SIZE MISMATCH");
                failing += 1;
                continue;
            }
            let mut peak = 0u8;
            let mut over = 0usize;
            for (a, b) in old[8..].chunks_exact(4).zip(bytes[8..].chunks_exact(4)) {
                let d = (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap();
                peak = peak.max(d);
                over += usize::from(d > 32);
            }
            let share = over as f64 * 100.0 / (raster.width as f64 * raster.height as f64);
            worst = worst.max(peak);
            let fail = share >= 1.0;
            failing += usize::from(fail);
            if peak > 0 {
                println!(
                    "{stem} p{page} peak {peak:3} >32 {share:.3}%{}",
                    if fail { "  <-- FAIL" } else { "" }
                );
            }
        }
    }
    println!("pages {pages}, failing {failing}, worst peak {worst}");
}
