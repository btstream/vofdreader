use std::io::Write as _;

use ofd_core::{OfdDocument, PageRenderer, RasterBackend, RasterPage};
use zip::{ZipWriter, write::SimpleFileOptions};

/// Page 120×80 mm; at 254 DPI one millimetre is ten pixels.
const DPI: f32 = 254.0;

const PAGE: &str = r#"<Page><Content><Layer ID="1">
<PathObject ID="2" Fill="true" Boundary="10 10 40 40" Stroke="false"><FillColor Value="255 0 0"/>
  <Clips TransFlag="false"><Clip><Area CTM="0.5 0 0 0.5 0 0"><Path Boundary="0 0 40 40"><AbbreviatedData>M 0 0 L 40 0 L 40 40 L 0 40 C</AbbreviatedData></Path></Area></Clip></Clips>
  <AbbreviatedData>M 0 0 L 40 0 L 40 40 L 0 40 C</AbbreviatedData></PathObject>
<PathObject ID="3" Fill="true" Boundary="60 10 40 40" CTM="40 0 0 40 0 0" Stroke="false"><FillColor Value="0 160 0"/>
  <Clips TransFlag="true"><Clip><Area><Path Boundary="0 0 1 1"><AbbreviatedData>M 0 0 L 0.5 0 L 0.5 0.5 L 0 0.5 C</AbbreviatedData></Path></Area></Clip></Clips>
  <AbbreviatedData>M 0 0 L 1 0 L 1 1 L 0 1 C</AbbreviatedData></PathObject>
<PathObject ID="4" Fill="true" Boundary="60 10 40 40" Stroke="false"><FillColor Value="0 0 255"/>
  <Clips>
    <Clip><Area><Path><AbbreviatedData>M 25 25 L 40 25 L 40 40 L 25 40 C</AbbreviatedData></Path></Area></Clip>
    <Clip><Area><Path><AbbreviatedData>M 30 30 L 45 30 L 45 45 L 30 45 C</AbbreviatedData></Path></Area></Clip>
  </Clips>
  <AbbreviatedData>M 0 0 L 40 0 L 40 40 L 0 40 C</AbbreviatedData></PathObject>
<PathObject ID="5" Fill="true" Boundary="10 55 100 10" Stroke="false"><FillColor Value="0 0 0">
  <AxialShd StartPoint="20 0" EndPoint="80 0" Extend="0"><Segment Position="0"><Color Value="0 0 255"/></Segment><Segment Position="1"><Color Value="255 0 0"/></Segment></AxialShd></FillColor>
  <AbbreviatedData>M 0 0 L 100 0 L 100 10 L 0 10 C</AbbreviatedData></PathObject>
<PathObject ID="6" Fill="true" Boundary="10 66 100 12" Stroke="false"><FillColor Value="0 0 0">
  <RadialShd StartPoint="50 6" EndPoint="50 6" EndRadius="20" Extend="3"><Segment Position="0"><Color Value="255 255 0"/></Segment><Segment Position="1"><Color Value="0 128 0"/></Segment></RadialShd></FillColor>
  <AbbreviatedData>M 0 0 L 100 0 L 100 12 L 0 12 C</AbbreviatedData></PathObject>
</Layer></Content></Page>"#;

fn document() -> (tempfile::TempDir, OfdDocument) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clips.ofd");
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
        ("Page.xml", PAGE),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let doc = OfdDocument::open(&path).unwrap();
    (dir, doc)
}

/// RGB at a page position in millimetres.
fn rgb(page: &RasterPage, x_mm: f32, y_mm: f32) -> (u8, u8, u8) {
    let x = (x_mm * 10.0) as usize;
    let y = (y_mm * 10.0) as usize;
    let i = (y * page.width as usize + x) * 4;
    (page.bgra[i + 2], page.bgra[i + 1], page.bgra[i])
}

const WHITE: (u8, u8, u8) = (255, 255, 255);

#[test]
fn clips_limit_objects_in_boundary_and_ctm_space() {
    let (_dir, doc) = document();
    let page = PageRenderer::new(&doc)
        .with_backend(RasterBackend::Direct)
        .rasterize(&doc, 0, DPI)
        .unwrap();
    // Area CTM halves the clip: only 10..30 mm of the red square shows.
    assert_eq!(rgb(&page, 20.0, 20.0), (255, 0, 0));
    assert_eq!(rgb(&page, 40.0, 40.0), WHITE);
    // TransFlag="true": the unit-square clip goes through the object CTM.
    assert_eq!(rgb(&page, 70.0, 20.0), (0, 160, 0));
    assert_eq!(rgb(&page, 90.0, 25.0), WHITE);
    // Two Clip elements intersect: 90..100 mm, not their union.
    assert_eq!(rgb(&page, 95.0, 45.0), (0, 0, 255));
    assert_eq!(rgb(&page, 87.0, 37.0), WHITE);
    assert_eq!(rgb(&page, 102.0, 52.0), WHITE);
}

#[test]
fn shadings_fill_paths_and_respect_extend() {
    let (_dir, doc) = document();
    let page = PageRenderer::new(&doc)
        .with_backend(RasterBackend::Direct)
        .rasterize(&doc, 0, DPI)
        .unwrap();
    // Axial blue→red between x=30 and x=90 mm; Extend="0" leaves the rest
    // unpainted instead of falling back to the black Value.
    assert_eq!(rgb(&page, 15.0, 60.0), WHITE);
    assert_eq!(rgb(&page, 105.0, 60.0), WHITE);
    let (r, _, b) = rgb(&page, 31.0, 60.0);
    assert!(b > 230 && r < 25, "start is blue: {r} {b}");
    let (r, _, b) = rgb(&page, 60.0, 60.0);
    assert!(
        (100..156).contains(&r) && (100..156).contains(&b),
        "middle: {r} {b}"
    );
    let (r, _, b) = rgb(&page, 89.0, 60.0);
    assert!(r > 230 && b < 25, "end is red: {r} {b}");
    // Radial yellow centre, green beyond the radius (Extend="3" pads).
    let (r, g, b) = rgb(&page, 60.0, 72.0);
    assert!(r > 240 && g > 240 && b < 15, "centre: {r} {g} {b}");
    assert_eq!(rgb(&page, 12.0, 72.0), (0, 128, 0));
}

#[test]
fn clips_and_shadings_match_svg_backend() {
    let (_dir, doc) = document();
    let old = PageRenderer::new(&doc).with_backend(RasterBackend::Svg);
    let new = PageRenderer::new(&doc).with_backend(RasterBackend::Direct);
    for dpi in [96.0, DPI] {
        let a = old.rasterize(&doc, 0, dpi).unwrap();
        let b = new.rasterize(&doc, 0, dpi).unwrap();
        assert_eq!((a.width, a.height), (b.width, b.height));
        let over = a
            .bgra
            .chunks_exact(4)
            .zip(b.bgra.chunks_exact(4))
            .filter(|(pa, pb)| (0..4).any(|i| pa[i].abs_diff(pb[i]) > 32))
            .count();
        let pixels = (a.width * a.height) as usize;
        assert!(
            over * 100 < pixels,
            "{over} of {pixels} pixels differ at {dpi} DPI"
        );
    }
}
