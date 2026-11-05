//! Pixel A/B between the direct tiny-skia backend and the former SVG path.
//!
//! cargo run --release -p ofd-core --example compare_backends -- [DPI] [DIFF_DIR] FILE...
//!
//! Per page: peak single-channel difference, share of pixels whose largest
//! channel difference exceeds 32, and timings of both backends (warm).
//! With DIFF_DIR, writes `<file>-p<page>.png` (old | new | amplified diff).

use std::time::Instant;

use ofd_core::render::RasterBackend;
use ofd_core::{OfdDocument, PageRenderer};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dpi: f32 = match args.first().and_then(|value| value.parse().ok()) {
        Some(dpi) => {
            args.remove(0);
            dpi
        }
        None => 96.0,
    };
    let diff_dir = args
        .first()
        .filter(|value| !value.ends_with(".ofd"))
        .cloned()
        .inspect(|_| {
            args.remove(0);
        });
    let mut worst = 0u8;
    let mut failing = 0;
    let mut pages = 0;
    println!("file page  peak  >32%   svg-ms direct-ms");
    for file in &args {
        let doc = match OfdDocument::open(file) {
            Ok(doc) => doc,
            Err(error) => {
                println!("{file}: open failed: {error}");
                continue;
            }
        };
        let old = PageRenderer::new(&doc).with_backend(RasterBackend::Svg);
        let new = PageRenderer::new(&doc).with_backend(RasterBackend::Direct);
        for page in 0..doc.page_count().min(20) {
            let (Ok(a), Ok(b)) = (
                old.rasterize(&doc, page, dpi),
                new.rasterize(&doc, page, dpi),
            ) else {
                println!("{file} p{page}: render failed");
                continue;
            };
            let t = Instant::now();
            let _ = old.rasterize(&doc, page, dpi);
            let old_ms = t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            let _ = new.rasterize(&doc, page, dpi);
            let new_ms = t.elapsed().as_secs_f64() * 1000.0;
            pages += 1;
            if (a.width, a.height) != (b.width, b.height) {
                println!(
                    "{file} p{page}: SIZE MISMATCH {}x{} vs {}x{}",
                    a.width, a.height, b.width, b.height
                );
                failing += 1;
                continue;
            }
            let mut peak = 0u8;
            let mut over = 0usize;
            let mut diff = vec![0u8; a.bgra.len()];
            for ((pa, pb), pd) in a
                .bgra
                .chunks_exact(4)
                .zip(b.bgra.chunks_exact(4))
                .zip(diff.chunks_exact_mut(4))
            {
                let d = (0..4).map(|i| pa[i].abs_diff(pb[i])).max().unwrap();
                peak = peak.max(d);
                if d > 32 {
                    over += 1;
                }
                let v = d.saturating_mul(4);
                pd.copy_from_slice(&[0, 0, v, 255]);
            }
            let share = over as f64 * 100.0 / (a.width as f64 * a.height as f64);
            let pass = peak <= 32 || share < 1.0;
            if !pass {
                failing += 1;
            }
            worst = worst.max(peak);
            println!(
                "{} p{page} {peak:5} {share:6.3} {old_ms:8.2} {new_ms:9.2}{}",
                std::path::Path::new(file)
                    .file_name()
                    .unwrap()
                    .to_string_lossy(),
                if pass { "" } else { "  <-- FAIL" }
            );
            if let Some(dir) = &diff_dir
                && peak > 0
            {
                write_triptych(dir, file, page, &a, &b, &diff);
            }
        }
    }
    println!("pages {pages}, failing {failing}, worst peak {worst}");
}

fn write_triptych(
    dir: &str,
    file: &str,
    page: usize,
    a: &ofd_core::RasterPage,
    b: &ofd_core::RasterPage,
    diff: &[u8],
) {
    let (w, h) = (a.width as usize, a.height as usize);
    let mut out = resvg::tiny_skia::Pixmap::new((w * 3) as u32, h as u32).unwrap();
    let data = out.data_mut();
    for y in 0..h {
        for (column, source) in [&a.bgra[..], &b.bgra[..], diff].into_iter().enumerate() {
            for x in 0..w {
                let s = &source[(y * w + x) * 4..][..4];
                let d = &mut data[(y * w * 3 + column * w + x) * 4..][..4];
                // BGRA straight → RGBA premultiplied (pages are opaque).
                d.copy_from_slice(&[s[2], s[1], s[0], 255]);
            }
        }
    }
    let name = std::path::Path::new(file)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let _ = std::fs::create_dir_all(dir);
    let _ = out.save_png(format!("{dir}/{name}-p{page}.png"));
}
