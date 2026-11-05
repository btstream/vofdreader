use std::io::Write;
use std::sync::Arc;

use ofd_core::OfdDocument;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

const OFD_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:OFD xmlns:ofd="http://www.ofdspec.org/2016" Version="1.0" DocType="OFD">
  <ofd:DocBody>
    <ofd:DocInfo>
      <ofd:DocID>minimal</ofd:DocID>
      <ofd:Title>最小样本</ofd:Title>
      <ofd:Creator>ofd-core tests</ofd:Creator>
    </ofd:DocInfo>
    <ofd:DocRoot>Doc_0/Document.xml</ofd:DocRoot>
  </ofd:DocBody>
</ofd:OFD>"#;

const DOCUMENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Document xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:CommonData>
    <ofd:PublicRes>PublicRes.xml</ofd:PublicRes>
    <ofd:DocumentRes>DocumentRes.xml</ofd:DocumentRes>
    <ofd:PageArea><ofd:PhysicalBox>0 0 100 50</ofd:PhysicalBox></ofd:PageArea>
    <ofd:TemplatePage ID="1" BaseLoc="Tpls/Tpl_0/Content.xml"/>
  </ofd:CommonData>
  <ofd:Pages>
    <ofd:Page ID="61" BaseLoc="Pages/Page_0/Content.xml"/>
  </ofd:Pages>
  <ofd:Annotations>Annots/Annotations.xml</ofd:Annotations>
</ofd:Document>"#;

const ANNOTATIONS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Annotations xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:Page PageID="61">
    <ofd:FileLoc>Page_0/Annotation.xml</ofd:FileLoc>
  </ofd:Page>
</ofd:Annotations>"#;

const ANNOTATION_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:PageAnnot xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:Annot Type="Watermark" ID="99">
    <ofd:Appearance Boundary="10 10 20 20">
      <ofd:ImageObject ID="100" ResourceID="2 1" Boundary="0 0 20 20" BlendMode="Darken"/>
      <ofd:TextObject ID="101" Boundary="0 0 20 5" Font="1" Size="3">
        <ofd:TextCode X="0" Y="3">水印</ofd:TextCode>
      </ofd:TextObject>
      <ofd:PathObject ID="102" Boundary="0 0 20 20">
        <ofd:FillColor Value="0 0 0">
          <ofd:Pattern Width="10" Height="5" XStep="10" YStep="5" CTM="1 0 0 1 0 0">
            <ofd:CellContent>
              <ofd:TextObject ID="103" Boundary="0 0 10 5" Font="1" Size="3">
                <ofd:TextCode X="0" Y="3">图案</ofd:TextCode>
              </ofd:TextObject>
            </ofd:CellContent>
          </ofd:Pattern>
        </ofd:FillColor>
        <ofd:AbbreviatedData>M 0 0 L 20 0 L 20 20 L 0 20 C</ofd:AbbreviatedData>
      </ofd:PathObject>
    </ofd:Appearance>
  </ofd:Annot>
</ofd:PageAnnot>"#;

const CONTENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Page xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:Area><ofd:PhysicalBox>0 0 100 50</ofd:PhysicalBox></ofd:Area>
  <ofd:Template TemplateID="1" ZOrder="Background"/>
  <ofd:Content>
    <ofd:Layer ID="2">
      <ofd:PathObject ID="3" Boundary="5 5 20 10" Fill="true">
        <ofd:FillColor Value="219 241 245"/>
        <ofd:AbbreviatedData>M 0 0 L 20 0 L 20 10 C</ofd:AbbreviatedData>
      </ofd:PathObject>
      <ofd:ImageObject ID="4" ResourceID="2 1" Boundary="30 5 10 10"/>
      <ofd:TextObject ID="5" Boundary="5 25 60 6" Font="1" Size="4">
        <ofd:CGTransform CodePosition="0" CodeCount="4" GlyphCount="4">
          <ofd:Glyphs>1 2 3 4</ofd:Glyphs>
        </ofd:CGTransform>
        <ofd:TextCode X="0" Y="4">发票测试</ofd:TextCode>
      </ofd:TextObject>
      <ofd:PageBlock ID="6">
        <ofd:TextObject ID="7" Boundary="5 35 60 6" Font="1" Size="4">
          <ofd:TextCode X="0" Y="4">嵌套文本</ofd:TextCode>
        </ofd:TextObject>
      </ofd:PageBlock>
    </ofd:Layer>
  </ofd:Content>
</ofd:Page>"#;

const PUBLIC_RES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Res xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:DrawParams>
    <ofd:DrawParam ID="9">
      <ofd:FillColor Value="0 128 0" ColorSpace="1"/>
    </ofd:DrawParam>
  </ofd:DrawParams>
  <ofd:Fonts>
    <ofd:Font ID="1" FontName="Fake Sans" FamilyName="Fake Sans">
      <ofd:FontFile>font_0.ttf</ofd:FontFile>
    </ofd:Font>
  </ofd:Fonts>
</ofd:Res>"#;

const TEMPLATE_CONTENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Page xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:Area><ofd:PhysicalBox>0 0 100 50</ofd:PhysicalBox></ofd:Area>
  <ofd:Content>
    <ofd:Layer ID="20" DrawParam="9">
      <ofd:TextObject ID="21" Boundary="5 5 60 6" Font="1" Size="4">
        <ofd:TextCode X="0" Y="4">模板继承色</ofd:TextCode>
      </ofd:TextObject>
    </ofd:Layer>
  </ofd:Content>
</ofd:Page>"#;

const DOCUMENT_RES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Res xmlns:ofd="http://www.ofdspec.org/2016" BaseLoc="Res">
  <ofd:MultiMedias>
    <ofd:MultiMedia ID="2" Type="Image" Format="PNG">
      <ofd:MediaFile>image_0.png</ofd:MediaFile>
    </ofd:MultiMedia>
  </ofd:MultiMedias>
</ofd:Res>"#;

fn write_entry(zip: &mut ZipWriter<std::fs::File>, name: &str, data: &[u8]) {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file(name, options).unwrap();
    zip.write_all(data).unwrap();
}

#[test]
fn parses_minimal_ofd_package() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minimal.ofd");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = ZipWriter::new(file);

    write_entry(&mut zip, "OFD.xml", OFD_XML.as_bytes());
    write_entry(&mut zip, "Doc_0/Document.xml", DOCUMENT_XML.as_bytes());
    write_entry(
        &mut zip,
        "Doc_0/Pages/Page_0/Content.xml",
        CONTENT_XML.as_bytes(),
    );
    write_entry(&mut zip, "Doc_0/PublicRes.xml", PUBLIC_RES.as_bytes());
    write_entry(
        &mut zip,
        "Doc_0/Tpls/Tpl_0/Content.xml",
        TEMPLATE_CONTENT.as_bytes(),
    );
    write_entry(&mut zip, "Doc_0/DocumentRes.xml", DOCUMENT_RES.as_bytes());
    write_entry(
        &mut zip,
        "Doc_0/Annots/Annotations.xml",
        ANNOTATIONS_XML.as_bytes(),
    );
    write_entry(
        &mut zip,
        "Doc_0/Annots/Page_0/Annotation.xml",
        ANNOTATION_XML.as_bytes(),
    );
    // FontFile casing differs from the zip entry; resolution is case-insensitive.
    write_entry(&mut zip, "Doc_0/FONT_0.TTF", b"not-a-real-font");
    write_entry(&mut zip, "Doc_0/Res/image_0.png", b"not-a-real-image");
    zip.finish().unwrap();

    let doc = OfdDocument::open(&path).unwrap();
    check_region_rasterization(&doc);
    assert_eq!(doc.page_count(), 1);
    assert_eq!(doc.ofd.version, "1.0");
    assert_eq!(doc.ofd.info.title.as_deref(), Some("最小样本"));
    assert_eq!(doc.resources.fonts.len(), 1);
    assert_eq!(doc.resources.embedded_font_count(), 1);
    assert_eq!(doc.resources.images.len(), 1);
    assert_eq!(
        doc.resources.images[0].file.as_deref(),
        Some("Doc_0/Res/image_0.png")
    );

    let page = doc.page_summary(0).unwrap();
    assert_eq!(page.size.map(|s| (s.width, s.height)), Some((100.0, 50.0)));
    assert_eq!(doc.page_size_only(0).unwrap(), page.size);
    let first = doc.package.read_shared("OFD.xml").unwrap();
    let second = doc.package.read_shared("OFD.xml").unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(page.text_objects, 2);
    assert_eq!(page.path_objects, 1);
    assert_eq!(page.image_objects, 1);
    assert_eq!(page.page_blocks, 1);
    assert_eq!(page.cg_transforms, 1);
    assert_eq!(page.templates, vec!["1(Background)"]);
    assert_eq!(page.text, vec!["发票测试", "嵌套文本"]);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);

    assert_eq!(doc.annotations.len(), 1);
    assert_eq!(doc.annotations[0].annots.len(), 1);
    assert_eq!(doc.annotations[0].annots[0].kind, "Watermark");
    assert_eq!(doc.annotations[0].annots[0].image_resources, vec!["2 1"]);
    assert!(
        doc.annotations[0].annots[0]
            .text
            .iter()
            .any(|text| text == "水印")
    );
    assert!(
        doc.annotations[0].annots[0]
            .text
            .iter()
            .any(|text| text == "图案")
    );

    assert!(!doc.has_signature_material());

    let rects = doc.page_search_rects(0, "发票测试").unwrap();
    assert_eq!(rects.len(), 1);
    assert!((rects[0].x - 5.0).abs() < 1.0);
    assert!((rects[0].y - 25.5).abs() < 2.0);
    assert!(rects[0].width > 10.0 && rects[0].width < 20.0);
    assert!(rects[0].height > 3.0 && rects[0].height < 5.0);
    assert!(rects[0].x + rects[0].width <= 100.0);
    assert!(rects[0].y + rects[0].height <= 50.0);
    assert!(doc.page_search_rects(0, "不存在").unwrap().is_empty());

    let layout = doc.page_text_layout(0).unwrap();
    assert!(!layout.is_empty());
    let all_text = layout.text_range(0, layout.len());
    assert!(all_text.contains("发票测试"));
    assert!(all_text.contains('\n'));
    let first = layout.index_at(6.0, 27.0, 5.0).unwrap();
    let last = layout.index_at(18.0, 27.0, 5.0).unwrap();
    let selected = layout.text_range(first, last + 1);
    assert!(selected.contains("发票测试"), "{selected}");
    assert!(layout.rects(first, last + 1).len() >= 4);
    assert!(layout.index_at(-50.0, -50.0, 0.1).is_none());

    let display_text = doc.page_display_text(0).unwrap();
    assert!(display_text.iter().any(|text| text.contains("模板继承色")));
    assert!(display_text.iter().any(|text| text.contains("发票测试")));

    let svg = doc.page_svg(0).unwrap();
    // Embedded outlines are self-contained paths; invalid fixture fonts fall
    // back to text without advertising an unusable browser @font-face.
    assert!(!svg.contains("@font-face"));
    assert!(svg.contains("<text"));
    let render_svg = doc.page_svg_for_render(0).unwrap();
    assert!(!render_svg.contains("@font-face"));
    assert!(render_svg.contains("<text x="));
    // Text colors inherit from the template layer's DrawParam FillColor.
    assert!(svg.contains(r##"fill="#008000""##), "{svg}");
    assert!(svg.contains("发"));
    assert!(svg.contains("嵌"));
    assert!(svg.contains("<path"));
    assert!(svg.contains("<image"));
    assert!(svg.contains("mix-blend-mode:darken"));
    assert!(
        svg.matches("图案").count() >= 4,
        "pattern cells should be expanded"
    );
    assert!(svg.contains("width=\"100mm\""));
}

fn check_region_rasterization(doc: &OfdDocument) {
    use ofd_core::{PageRenderer, RasterRegion};
    let renderer = PageRenderer::new(doc);
    for dpi in [96.0, 216.0, 384.0] {
        let full = renderer.rasterize(doc, 0, dpi).unwrap();
        // Multiple crops cross vector, text, pattern and annotation boundaries.
        for (x, y, width, height) in [(0, 0, 120, 80), (73, 51, 177, 97), (251, 99, 100, 80)] {
            let region = RasterRegion {
                x,
                y,
                width,
                height,
            };
            let crop = renderer.rasterize_region(doc, 0, dpi, region).unwrap();
            assert_eq!((crop.width, crop.height), (width, height));
            let mut max_delta = 0;
            // tiny-skia may round antialiased edges differently after an
            // integer translation. Check a bounded peak AND mean error, not
            // false bit-exact equality for text/pattern edges.
            let mut max_at = (0, 0);
            let mut total_delta = 0u64;
            for cy in 2..height - 2 {
                for cx in 2..width - 2 {
                    if x + cx >= full.width || y + cy >= full.height {
                        continue;
                    }
                    let a = ((cy * width + cx) * 4) as usize;
                    let b = (((y + cy) * full.width + x + cx) * 4) as usize;
                    for channel in 0..4 {
                        let delta = crop.bgra[a + channel].abs_diff(full.bgra[b + channel]);
                        total_delta += delta as u64;
                        if delta > max_delta {
                            max_delta = delta;
                            max_at = (cx, cy);
                        }
                    }
                }
            }
            assert!(
                max_delta <= 32 && total_delta < (width * height) as u64,
                "dpi={dpi}, region={region:?}, delta={max_delta}, at={max_at:?}, total={total_delta}"
            );
        }
    }
    let valid = RasterRegion {
        x: 0,
        y: 0,
        width: 32,
        height: 32,
    };
    for dpi in [0.0, -1.0, f32::NAN, f32::INFINITY, 2401.0] {
        assert!(renderer.rasterize_region(doc, 0, dpi, valid).is_err());
    }
    for region in [
        RasterRegion { width: 0, ..valid },
        RasterRegion {
            height: 4097,
            ..valid
        },
        RasterRegion {
            x: u32::MAX,
            ..valid
        },
    ] {
        assert!(renderer.rasterize_region(doc, 0, 96.0, region).is_err());
    }
    let outside = renderer
        .rasterize_region(
            doc,
            0,
            96.0,
            RasterRegion {
                x: 2000,
                y: 2000,
                ..valid
            },
        )
        .unwrap();
    assert!(outside.bgra.iter().all(|b| *b == 0));
}
