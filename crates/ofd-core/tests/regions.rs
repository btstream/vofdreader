use ofd_core::{OfdDocument, PageRenderer, RasterRegion};
use std::{io::Write, path::Path};
use zip::{ZipWriter, write::SimpleFileOptions};

#[test]
fn aligned_shapes_match_full_page_exactly_across_tile_seams() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("regions.ofd");
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, text) in [
        (
            "OFD.xml",
            r#"<OFD><DocBody><DocRoot>Document.xml</DocRoot></DocBody></OFD>"#,
        ),
        (
            "Document.xml",
            r#"<Document><CommonData><PageArea><PhysicalBox>0 0 254 127</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/></Pages></Document>"#,
        ),
        (
            "Page.xml",
            r#"<Page><Content><Layer ID="1"><PathObject ID="2" Boundary="0 0 254 127" Fill="true" Stroke="false"><FillColor Value="70 120 230"/><AbbreviatedData>M 0 0 L 254 0 L 254 127 L 0 127 C</AbbreviatedData></PathObject></Layer></Content></Page>"#,
        ),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let doc = OfdDocument::open(path).unwrap();
    let renderer = PageRenderer::new(&doc);
    let full = renderer.rasterize(&doc, 0, 192.0).unwrap();
    for x in [0, 510, 1022, 1534] {
        let r = RasterRegion {
            x,
            y: 210,
            width: 300,
            height: 300,
        };
        let tile = renderer.rasterize_region(&doc, 0, 192.0, r).unwrap();
        for y in 0..r.height {
            let begin = (((r.y + y) * full.width + x) * 4) as usize;
            assert_eq!(
                &tile.bgra[(y * r.width * 4) as usize..((y + 1) * r.width * 4) as usize],
                &full.bgra[begin..begin + (r.width * 4) as usize]
            );
        }
    }
}

#[test]
#[ignore = "explicit local corpus validation"]
fn corpus_regions_render_at_high_density() {
    fn visit(path: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, files);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ofd"))
            {
                files.push(path);
            }
        }
    }
    let mut files = vec![];
    visit(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources"),
        &mut files,
    );
    assert!(!files.is_empty());
    let mut pages = 0;
    for file in &files {
        let doc = OfdDocument::open(file).unwrap();
        let renderer = PageRenderer::new(&doc);
        for page in 0..doc.page_count() {
            for (x, y) in [(0, 0), (510, 510), (1022, 1534)] {
                let raster = renderer
                    .rasterize_region(
                        &doc,
                        page,
                        768.0,
                        RasterRegion {
                            x,
                            y,
                            width: 516,
                            height: 516,
                        },
                    )
                    .unwrap_or_else(|e| panic!("{} page {}: {e}", file.display(), page + 1));
                assert_eq!(raster.bgra.len(), 516 * 516 * 4);
            }
            pages += 1;
        }
    }
    eprintln!(
        "region corpus: {} documents, {pages} pages, {} tiles at 768 DPI",
        files.len(),
        pages * 3
    );
}

/// The direct tiny-skia backend must match the former SVG/resvg path within
/// anti-aliasing tolerance: fills, even-odd, arcs, dashed strokes, alpha.
#[test]
fn direct_backend_matches_svg_backend() {
    use ofd_core::RasterBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backends.ofd");
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, text) in [
        (
            "OFD.xml",
            r#"<OFD><DocBody><DocRoot>Document.xml</DocRoot></DocBody></OFD>"#,
        ),
        (
            "Document.xml",
            r#"<Document><CommonData><PageArea><PhysicalBox>0 0 120 80</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/></Pages></Document>"#,
        ),
        (
            "Page.xml",
            r#"<Page><Content><Layer ID="1">
<PathObject ID="2" Boundary="5 5 50 40" Fill="true" Stroke="true" LineWidth="1.5" Rule="Even-Odd"><FillColor Value="70 120 230" Alpha="180"/><StrokeColor Value="200 30 30"/><AbbreviatedData>M 0 0 L 50 0 L 50 40 L 0 40 C M 10 10 L 40 10 L 40 30 L 10 30 C</AbbreviatedData></PathObject>
<PathObject ID="3" Boundary="60 5 55 40" Fill="false" Stroke="true" LineWidth="0.8" Cap="Round" Join="Round" DashPattern="3 1.5"><StrokeColor Value="0 120 0"/><AbbreviatedData>M 5 20 A 20 15 30 1 1 45 25 B 50 0 30 40 5 35</AbbreviatedData></PathObject>
<PathObject ID="4" Boundary="10 50 100 25" Fill="true" Stroke="false" Alpha="128"><FillColor Value="250 180 0"/><AbbreviatedData>M 0 0 Q 50 50 100 0 L 100 25 L 0 25 C</AbbreviatedData></PathObject>
</Layer></Content></Page>"#,
        ),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let doc = OfdDocument::open(path).unwrap();
    let old = PageRenderer::new(&doc).with_backend(RasterBackend::Svg);
    let new = PageRenderer::new(&doc).with_backend(RasterBackend::Direct);
    for dpi in [96.0, 216.0] {
        let a = old.rasterize(&doc, 0, dpi).unwrap();
        let b = new.rasterize(&doc, 0, dpi).unwrap();
        assert_eq!((a.width, a.height), (b.width, b.height));
        let mut over = 0;
        for (pa, pb) in a.bgra.chunks_exact(4).zip(b.bgra.chunks_exact(4)) {
            if (0..4).any(|i| pa[i].abs_diff(pb[i]) > 32) {
                over += 1;
            }
        }
        let pixels = (a.width * a.height) as usize;
        assert!(
            over * 100 < pixels,
            "{over} of {pixels} pixels differ at {dpi} DPI"
        );
    }
}
