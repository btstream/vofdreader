use std::io::Write;

use ofd_core::OfdDocument;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

const OFD_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:OFD xmlns:ofd="http://www.ofdspec.org/2016" Version="1.0" DocType="OFD">
  <ofd:DocBody>
    <ofd:DocInfo><ofd:DocID>composite</ofd:DocID></ofd:DocInfo>
    <ofd:DocRoot>Doc_0/Document.xml</ofd:DocRoot>
  </ofd:DocBody>
</ofd:OFD>"#;

const DOCUMENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Document xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:CommonData>
    <ofd:PublicRes>PublicRes.xml</ofd:PublicRes>
    <ofd:PageArea><ofd:PhysicalBox>0 0 100 50</ofd:PhysicalBox></ofd:PageArea>
  </ofd:CommonData>
  <ofd:Pages>
    <ofd:Page ID="1" BaseLoc="Pages/Page_0/Content.xml"/>
  </ofd:Pages>
</ofd:Document>"#;

const PAGE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Page xmlns:ofd="http://www.ofdspec.org/2016">
  <ofd:Area><ofd:PhysicalBox>0 0 100 50</ofd:PhysicalBox></ofd:Area>
  <ofd:Content>
    <ofd:Layer ID="1">
      <ofd:CompositeObject ID="2" Boundary="10 10 30 10" CTM="1 0 0 1 0 0" ResourceID="3"/>
    </ofd:Layer>
  </ofd:Content>
</ofd:Page>"#;

const PUBLIC_RES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ofd:Res xmlns:ofd="http://www.ofdspec.org/2016" BaseLoc="Res">
  <ofd:Fonts>
    <ofd:Font ID="1" FontName="SimSun" FamilyName="宋体"/>
  </ofd:Fonts>
  <ofd:CompositeGraphicUnits>
    <ofd:CompositeGraphicUnit ID="3" Width="100" Height="50">
      <ofd:Content ID="4">
        <ofd:TextObject ID="5" Boundary="0 0 20 5" Font="1" Size="4">
          <ofd:TextCode X="0" Y="4">复合文字</ofd:TextCode>
        </ofd:TextObject>
      </ofd:Content>
    </ofd:CompositeGraphicUnit>
  </ofd:CompositeGraphicUnits>
</ofd:Res>"#;

fn write_entry(zip: &mut ZipWriter<std::fs::File>, name: &str, data: &[u8]) {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file(name, options).unwrap();
    zip.write_all(data).unwrap();
}

#[test]
fn renders_composite_graphic_units() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("composite.ofd");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = ZipWriter::new(file);
    write_entry(&mut zip, "OFD.xml", OFD_XML.as_bytes());
    write_entry(&mut zip, "Doc_0/Document.xml", DOCUMENT_XML.as_bytes());
    write_entry(
        &mut zip,
        "Doc_0/Pages/Page_0/Content.xml",
        PAGE_XML.as_bytes(),
    );
    write_entry(&mut zip, "Doc_0/PublicRes.xml", PUBLIC_RES.as_bytes());
    zip.finish().unwrap();

    let doc = OfdDocument::open(&path).unwrap();
    assert_eq!(doc.resources.composite_units.len(), 1);
    assert_eq!(doc.resources.composite_units[0].id, "3");

    let text = doc.page_display_text(0).unwrap();
    assert!(
        text.iter().any(|line| line.contains("复合文字")),
        "{text:?}"
    );

    let svg = doc.page_svg(0).unwrap();
    assert!(svg.contains("复合文字"), "{svg}");
}
