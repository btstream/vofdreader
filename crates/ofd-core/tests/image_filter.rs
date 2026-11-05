use std::io::Write as _;

use ofd_core::{ImageFilter, OfdDocument, PageRenderer, RasterBackend, RasterPage};
use resvg::tiny_skia;
use zip::{ZipWriter, write::SimpleFileOptions};

/// A 2×2 black/white checkerboard PNG stretched over a 40×40 mm page.
fn document() -> (tempfile::TempDir, OfdDocument) {
    let mut checker = tiny_skia::Pixmap::new(2, 2).unwrap();
    for (i, px) in checker.pixels_mut().iter_mut().enumerate() {
        let v = if (i % 2) == (i / 2) { 0 } else { 255 };
        *px = tiny_skia::ColorU8::from_rgba(v, v, v, 255).premultiply();
    }
    let png = checker.encode_png().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filter.ofd");
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    let entries: [(&str, &[u8]); 5] = [
        (
            "OFD.xml",
            br#"<OFD><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
        ),
        (
            "Doc/Document.xml",
            br#"<Document><CommonData><PublicRes>Res.xml</PublicRes><PageArea><PhysicalBox>0 0 40 40</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/></Pages></Document>"#,
        ),
        (
            "Doc/Res.xml",
            br#"<Res><MultiMedias><MultiMedia ID="2" Type="Image" Format="PNG"><MediaFile>checker.png</MediaFile></MultiMedia></MultiMedias></Res>"#,
        ),
        (
            "Doc/Page.xml",
            br#"<Page><Content><Layer ID="1"><ImageObject ID="3" ResourceID="2" Boundary="0 0 40 40" CTM="40 0 0 40 0 0"/></Layer></Content></Page>"#,
        ),
        ("Doc/checker.png", &png),
    ];
    for (name, data) in entries {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
    let doc = OfdDocument::open(&path).unwrap();
    (dir, doc)
}

/// Grey level along the middle row, at `x_mm`. 25.4 DPI: 1 px per mm.
fn grey(page: &RasterPage, x_mm: usize) -> u8 {
    let y = page.height as usize / 4;
    page.bgra[(y * page.width as usize + x_mm) * 4]
}

#[test]
fn image_filter_selects_sampling_and_switches_without_reparse() {
    let (_dir, doc) = document();
    let renderer = PageRenderer::new(&doc).with_backend(RasterBackend::Direct);
    assert_eq!(renderer.image_filter(), ImageFilter::Bicubic, "default");
    let mut rows = Vec::new();
    for filter in [
        ImageFilter::Nearest,
        ImageFilter::Bilinear,
        ImageFilter::Bicubic,
    ] {
        // Same renderer: the parsed page is reused, only sampling changes.
        renderer.set_image_filter(filter);
        let page = renderer.rasterize(&doc, 0, 25.4).unwrap();
        rows.push((0..40).map(|x| grey(&page, x)).collect::<Vec<_>>());
    }
    let [nearest, bilinear, bicubic] = [&rows[0], &rows[1], &rows[2]];
    // Nearest keeps the two source pixels: only pure black and white.
    assert!(nearest.iter().all(|v| *v == 0 || *v == 255), "{nearest:?}");
    // Smoothing filters blend across the cell boundary at 20 mm.
    for row in [bilinear, bicubic] {
        assert!(row.iter().any(|v| (40..215).contains(v)), "{row:?}");
    }
    assert_ne!(bilinear, bicubic);
}
