use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::document::OfdDocument;
use crate::font::{EmbeddedFace, select_face};
use crate::model::{Color, FontResource};
use crate::xml::parse_xml;

use super::fonts::*;
use super::parse::*;
use super::renderer::*;
use super::svg::*;
use super::text::*;

#[test]
fn configurable_tree_cache_reuses_recent_pages_and_evicts_lru() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.ofd");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, content) in [
        (
            "OFD.xml",
            r#"<OFD><DocBody><DocRoot>Document.xml</DocRoot></DocBody></OFD>"#,
        ),
        (
            "Document.xml",
            r#"<Document><CommonData><PageArea><PhysicalBox>0 0 10 10</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/><Page ID="2" BaseLoc="Page.xml"/><Page ID="3" BaseLoc="Page.xml"/></Pages></Document>"#,
        ),
        ("Page.xml", "<Page><Content/></Page>"),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let doc = OfdDocument::open(path).unwrap();
    let renderer = PageRenderer::with_tree_cache_limit(&doc, 2);
    let page =
        |renderer: &PageRenderer, index| renderer.cached_page(&doc, index, true).unwrap().id();
    let first = page(&renderer, 0);
    let second = page(&renderer, 1);
    assert_eq!(first, page(&renderer, 0));
    page(&renderer, 2);
    assert_eq!(renderer.tree_cache.lock().unwrap().len(), 2);
    assert_eq!(first, page(&renderer, 0));
    // An evicted page is rebuilt; compare while the old one is alive.
    let old = renderer.cached_page(&doc, 1, true).unwrap();
    let _ = second;
    page(&renderer, 0);
    page(&renderer, 2);
    assert_ne!(old.id(), page(&renderer, 1));
    let disabled = PageRenderer::with_tree_cache_limit(&doc, 0);
    let kept = disabled.cached_page(&doc, 0, true).unwrap();
    assert_ne!(kept.id(), page(&disabled, 0));
    assert!(disabled.tree_cache.lock().unwrap().is_empty());
    assert_eq!(
        PageRenderer::with_tree_cache_limit(&doc, usize::MAX).tree_cache_limit,
        256
    );

    // Previews reuse cached pages but never insert or evict.
    let preview = PageRenderer::with_tree_cache_limit(&doc, 2);
    let read = preview.cached_page(&doc, 0, true).unwrap().id();
    preview.rasterize_preview(&doc, 1, 48.0).unwrap();
    preview.rasterize_preview(&doc, 2, 48.0).unwrap();
    preview.rasterize_preview(&doc, 0, 48.0).unwrap();
    assert_eq!(preview.tree_cache.lock().unwrap().len(), 1);
    assert_eq!(read, page(&preview, 0));

    // Parallel tiles of one uncached page share one build and match
    // serial output exactly.
    let parallel = PageRenderer::with_tree_cache_limit(&doc, 4);
    let region = |x| crate::render::RasterRegion {
        x,
        y: 0,
        width: 64,
        height: 64,
    };
    let results: Vec<_> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|i| {
                let parallel = &parallel;
                let doc = &doc;
                scope.spawn(move || {
                    parallel
                        .rasterize_region(doc, 1, 192.0, region(i * 64))
                        .unwrap()
                        .bgra
                })
            })
            .collect();
        jobs.into_iter().map(|job| job.join().unwrap()).collect()
    });
    assert_eq!(parallel.tree_cache.lock().unwrap().len(), 1);
    assert!(parallel.building.lock().unwrap().is_empty());
    for (i, tile) in results.iter().enumerate() {
        let serial = renderer
            .rasterize_region(&doc, 1, 192.0, region(i as u32 * 64))
            .unwrap();
        assert_eq!(tile, &serial.bgra);
    }
}

use super::*;

// Generated two-glyph TrueType fixture: a triangle mapped to U+0041.
// No dependency on installed fonts or private corpus documents.
fn audit_font() -> GlyphFont {
    let data = STANDARD.decode("AAEAAAAKAIAAAwAgT1MvMkE4QXkAAAEoAAAAYGNtYXAADACUAAABkAAAADRnbHlmCHsoEQAAAcwAAAAWaGVhZFlVpJwAAACsAAAANmhoZWEFFgEuAAAA5AAAACRobXR4AfQAAAAAAYgAAAAGbG9jYQALAAAAAAHEAAAABm1heHAABAAFAAABCAAAACBuYW1lLh01yQAAAeQAAABacG9zdAAoAAAAAAJAAAAAJgABAAAAAQAA9LIRg18PPPUAAwPoAAAAAHwlsIAAAAAAfCWwgAAAAAAB9AK8AAAAAwACAAAAAAAAAAEAAAMg/zgAAAH0AAAAAAH0AAEAAAAAAAAAAAAAAAAAAAABAAEAAAACAAMAAQAAAAAAAgAAAAAAAAAAAAAAAAAAAAAAAwH0AZAABQAEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAPz8/PwAAAEEAQQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAB9AAAAAAAAAAAAAIAAAADAAAAFAADAAEAAAAUAAQAIAAAAAQABAABAAAAQf//AAAAQf///8AAAQAAAAAAAAAAAAsAAAABAAAAAAH0ArwAAgAAMSEDAfT6ArwAAAAAAAQANgABAAAAAAABAAUAAAABAAAAAAACAAcABQADAAEECQABAAoADAADAAEECQACAA4AFkF1ZGl0UmVndWxhcgBBAHUAZABpAHQAUgBlAGcAdQBsAGEAcgAAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAACQAAA==").unwrap();
    GlyphFont {
        data: Arc::new(data),
        face_index: 0,
    }
}

fn audit_text(glyphs: Vec<u16>) -> Item {
    Item::Text {
        transform: Matrix::IDENTITY,
        chars: vec![PositionedChar {
            ch: 'A',
            x: 2.0,
            y: 3.0,
        }],
        glyphs,
        style: TextStyle {
            font_key: "Public:1".into(),
            font_family: "not-installed".into(),
            font_size: 10.0,
            color: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            weight: 400,
            italic: false,
            opacity: 1.0,
            h_scale: 1.0,
            char_direction: 0,
            read_direction: 0,
            paint: PathStyle {
                stroke: false,
                fill_color: Some(Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                ..PathStyle::default()
            },
            clusters: Vec::new(),
        },
    }
}

#[test]
fn search_rects_apply_group_transform() {
    let group = Item::Group {
        transform: Matrix {
            a: 0.5,
            b: 0.0,
            c: 0.0,
            d: 0.5,
            e: 10.0,
            f: 20.0,
        },
        items: vec![audit_text(vec![])],
        opacity: 1.0,
    };
    let mut rects = Vec::new();
    collect_text_matches(&[group], "A", Matrix::IDENTITY, &mut rects);
    assert_eq!(rects.len(), 1);
    assert!((rects[0].x - 11.0).abs() < 1e-9);
    assert!((rects[0].y - 17.1).abs() < 1e-9);
    assert!((rects[0].height - 5.0).abs() < 1e-9);
}

#[test]
fn embedded_cmap_and_gid_fonts_draw_inside_nested_groups() {
    for use_gid in [false, true] {
        let font = audit_font();
        let fonts = HashMap::from([("Public:1".into(), font)]);
        let text = audit_text(if use_gid { vec![1] } else { vec![] });
        let group = Item::Group {
            transform: Matrix::translate(10.0, 20.0),
            opacity: 0.4,
            items: vec![Item::Group {
                transform: Matrix::IDENTITY,
                opacity: 1.0,
                items: vec![text],
            }],
        };
        let mut svg = String::new();
        write_item_with_fonts(&mut svg, &group, None, Some(&fonts));
        assert!(svg.contains("<path"), "{svg}");
        assert!(!svg.contains("<text"), "{svg}");
        assert!(svg.contains(r#"opacity="0.4""#), "{svg}");
        assert!(svg.contains("matrix(0.01 0 0 -0.01 2 3)"), "{svg}");
    }
}

#[test]
fn invalid_embedded_outline_falls_back_without_blank_text() {
    let fonts = HashMap::from([("Public:1".into(), audit_font())]);
    let mut svg = String::new();
    write_item_with_fonts(&mut svg, &audit_text(vec![0]), None, Some(&fonts));
    assert!(svg.contains("<text"), "{svg}");
    assert!(svg.contains(">A</text>"), "{svg}");
}

#[test]
fn missing_glyph_does_not_replace_the_rest_of_the_embedded_run() {
    let fonts = HashMap::from([("Public:1".into(), audit_font())]);
    let mut text = audit_text(vec![]);
    if let Item::Text { chars, .. } = &mut text {
        chars.push(PositionedChar {
            ch: 'B',
            x: 7.0,
            y: 3.0,
        });
    }
    let mut svg = String::new();
    write_item_with_fonts(&mut svg, &text, None, Some(&fonts));
    assert_eq!(svg.matches("<path ").count(), 1, "{svg}");
    assert_eq!(svg.matches("<text ").count(), 1, "{svg}");
    assert!(svg.contains(">B</text>"), "{svg}");
    assert!(!svg.contains(">AB</text>"), "{svg}");
}

#[test]
fn raw_cff_uses_font_matrix_and_explicit_glyphs() {
    let font = GlyphFont {
        data: Arc::new(STANDARD.decode("AQAEAQABAQEJQXVkaXRDRkYAAQEBGPgbAvgbA/gYBIuL+Ij5UAXID4vhEssRAAEBAQpBdWRpdCBDRkYAAAAAIgACAQEEEfiIDviIixb4iAb7jvlQBQ4=").unwrap()),
        face_index: 0,
    };
    let fonts = HashMap::from([("Public:1".into(), font)]);
    let paths = text_glyph_items(&audit_text(vec![1]), &fonts).unwrap();
    assert!(matches!(&paths[0], Item::Path { .. }));
    let face = EmbeddedFace::parse(&fonts["Public:1"].data, 0).unwrap();
    assert!((face.matrix().a - 0.001).abs() < 1e-9);
    assert_eq!(face.glyph('A'), Some(1));
}

#[test]
fn raw_cid_cff_maps_sparse_cids_to_dense_charstrings() {
    let data = STANDARD.decode("AQAEAQABAQEJQXVkaXRDSUQAAQEBIvgb+ByLDB6Li/iI+VAF9xAMIuAPi/cXEuMMJfcKDCTrEQADAQEGDhZBZG9iZUlkZW50aXR5QXVkaXRDSUQAAAAAewMAAQAAAAACAAIBAQQR+IgO+IiLFviIBvuO+VAFDgABAQEJ+B0MJov3FxI=").unwrap();
    let face = EmbeddedFace::parse(&data, 0).unwrap();
    assert_eq!(face.explicit_glyph(123), Some(1));
    assert_eq!(face.explicit_glyph(124), None);
    let fonts = HashMap::from([(
        "Public:1".into(),
        GlyphFont {
            data: Arc::new(data),
            face_index: 0,
        },
    )]);
    let paths = text_glyph_items(&audit_text(vec![123]), &fonts).unwrap();
    assert!(matches!(paths[0], Item::Path { .. }));
}

#[test]
fn matrix_composition_agrees_with_successive_point_transforms() {
    let first = Matrix {
        a: 0.5,
        b: 1.0,
        c: -2.0,
        d: 0.3,
        e: 3.0,
        f: 4.0,
    };
    let second = Matrix {
        a: 2.0,
        b: -0.2,
        c: 0.7,
        d: 3.0,
        e: 6.0,
        f: -2.0,
    };
    let intermediate = second.transform_point(1.5, -7.0);
    let expected = first.transform_point(intermediate.0, intermediate.1);
    let actual = first.compose(second).transform_point(1.5, -7.0);
    assert!((actual.0 - expected.0).abs() < 1e-9);
    assert!((actual.1 - expected.1).abs() < 1e-9);
}

#[test]
fn svg_font_matrix_preserves_fractional_em_scale() {
    let matrix = Matrix {
        a: 1.0 / 2048.0,
        d: -1.0 / 2048.0,
        ..Matrix::IDENTITY
    };
    let svg = matrix.svg();
    let roundtrip = Matrix::parse(
        svg.strip_prefix("matrix(")
            .unwrap()
            .strip_suffix(')')
            .unwrap(),
    )
    .unwrap();
    let (x, y) = roundtrip.transform_point(2048.0, 2048.0);
    assert!((x - 1.0).abs() < 1e-6, "{svg}");
    assert!((y + 1.0).abs() < 1e-6, "{svg}");
}

#[test]
fn one_to_one_clusters_do_not_replace_a_run_containing_spaces() {
    let xml = parse_xml(br#"<TextObject><CGTransform CodePosition="0" CodeCount="3" GlyphCount="3"><Glyphs>1 0 1</Glyphs></CGTransform></TextObject>"#, "space.xml").unwrap();
    let mut text = audit_text(vec![]);
    if let Item::Text { chars, style, .. } = &mut text {
        *chars = "A A"
            .chars()
            .enumerate()
            .map(|(index, ch)| PositionedChar {
                ch,
                x: index as f64 * 5.0,
                y: 0.0,
            })
            .collect();
        style.clusters = glyph_clusters(xml.root_element(), chars);
    }
    let fonts = HashMap::from([("Public:1".into(), audit_font())]);
    let converted = text_glyph_items(&text, &fonts).unwrap();
    assert_eq!(converted.len(), 2);
    assert!(
        converted
            .iter()
            .all(|item| matches!(item, Item::Path { .. }))
    );
}

#[test]
fn font_audit_counts_actual_fallbacks_and_caches_page_results() {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("font-audit.ofd");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let entries = [
        (
            "OFD.xml",
            r#"<OFD><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
        ),
        (
            "Doc/Document.xml",
            r#"<Document><CommonData><PublicRes>Res.xml</PublicRes><PageArea><PhysicalBox>0 0 100 50</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/><Page ID="2" BaseLoc="Cluster.xml"/></Pages></Document>"#,
        ),
        (
            "Doc/Res.xml",
            r#"<Res><Fonts><Font ID="1" FontName="Audit"><FontFile>font.ttf</FontFile></Font></Fonts></Res>"#,
        ),
        (
            "Doc/Page.xml",
            r#"<Page><Content><Layer><TextObject Font="1" Size="10" Boundary="0 0 40 15"><TextCode X="0" Y="10" DeltaX="5">AB</TextCode></TextObject></Layer></Content></Page>"#,
        ),
        (
            "Doc/Cluster.xml",
            r#"<Page><Content><Layer><TextObject Font="1" Size="10" Boundary="0 0 40 15" HScale="0.5" CharDirection="90" Weight="700" Italic="true" Fill="false" Stroke="true" LineWidth="0.2"><StrokeColor Value="255 0 0"/><CGTransform CodePosition="0" CodeCount="2" GlyphCount="3"><Glyphs>1 1 1</Glyphs></CGTransform><TextCode X="3" Y="10" DeltaX="2 3">AA</TextCode></TextObject></Layer></Content></Page>"#,
        ),
    ];
    for (name, content) in entries {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.start_file("Doc/font.ttf", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&audit_font().data).unwrap();
    zip.finish().unwrap();
    let doc = OfdDocument::open(path).unwrap();
    let renderer = PageRenderer::new(&doc);
    let report = renderer.font_report(&doc, 0).unwrap();
    assert_eq!(report.embedded_glyphs, 1);
    assert_eq!(report.fallback_chars, 1);
    assert!(report.fallback_codepoints["Public:1"].contains(&('B' as u32)));
    assert!(Arc::ptr_eq(
        &report,
        &renderer.font_report(&doc, 0).unwrap()
    ));
    assert_eq!(
        renderer.export_svg(&doc, 0).unwrap(),
        renderer.render_svg(&doc, 0).unwrap()
    );
    let page = doc.display_list(1).unwrap();
    let paths = text_glyph_items(&page.items[0], &renderer.glyph_fonts).unwrap();
    assert_eq!(paths.len(), 3);
    for (path, x) in paths.iter().zip([3.0, 5.0, 8.0]) {
        let Item::Path {
            transform, style, ..
        } = path
        else {
            panic!("expected glyph")
        };
        assert!((transform.e - x).abs() < 1e-9);
        assert!((transform.f - 10.0).abs() < 1e-9);
        assert!((transform.b - 0.005).abs() < 1e-9);
        assert!(!style.fill && style.stroke);
        assert_eq!(style.stroke_color.unwrap().hex(), "#FF0000");
    }
    let report = renderer.font_report(&doc, 1).unwrap();
    assert_eq!((report.embedded_glyphs, report.fallback_chars), (3, 0));
}

#[test]
fn collection_selects_named_member_instead_of_first_face() {
    let first = (*audit_font().data).clone();
    let mut second = first.clone();
    for (from, to) in [
        (b"Audit".as_slice(), b"Other".as_slice()),
        (b"\0A\0u\0d\0i\0t".as_slice(), b"\0O\0t\0h\0e\0r".as_slice()),
    ] {
        for start in 0..=second.len() - from.len() {
            if &second[start..start + from.len()] == from {
                second[start..start + from.len()].copy_from_slice(to);
            }
        }
    }
    let mut collection = b"ttcf\0\x01\0\0\0\0\0\x02".to_vec();
    collection.extend_from_slice(&20_u32.to_be_bytes());
    collection.extend_from_slice(&(20_u32 + first.len() as u32).to_be_bytes());
    for mut font in [first, second] {
        let base = collection.len() as u32;
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        for index in 0..tables {
            let offset = 12 + index * 16 + 8;
            let original = u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());
            font[offset..offset + 4].copy_from_slice(&(base + original).to_be_bytes());
        }
        collection.extend_from_slice(&font);
    }
    let resource = FontResource {
        id: "1".into(),
        scope: crate::model::ResScope::Public,
        name: "Other".into(),
        family: "".into(),
        file: Some("font.ttc".into()),
        bold: false,
        italic: false,
    };
    assert_eq!(select_face(&collection, &resource), 1);
    assert!(
        EmbeddedFace::parse(&collection, 1)
            .unwrap()
            .glyph('A')
            .is_some()
    );
}

#[test]
fn clusters_handle_ligatures_expansion_and_unmapped_text() {
    let xml = r#"<TextObject><CGTransform CodePosition="0" CodeCount="2"><Glyphs>1</Glyphs></CGTransform><CGTransform CodePosition="2" GlyphCount="2"><Glyphs>1 1</Glyphs></CGTransform></TextObject>"#;
    let xml = parse_xml(xml.as_bytes(), "cluster.xml").unwrap();
    let mut text = audit_text(vec![]);
    if let Item::Text { chars, style, .. } = &mut text {
        *chars = "AAAA"
            .chars()
            .enumerate()
            .map(|(index, ch)| PositionedChar {
                ch,
                x: index as f64 * 10.0,
                y: 0.0,
            })
            .collect();
        style.clusters = glyph_clusters(xml.root_element(), chars);
        assert_eq!(style.clusters.len(), 2);
    }
    let fonts = HashMap::from([("Public:1".into(), audit_font())]);
    let converted = text_glyph_items(&text, &fonts).unwrap();
    // AB -> one ligature, C -> two glyphs, D -> its own cmap glyph.
    assert_eq!(converted.len(), 4);
    assert!(
        converted
            .iter()
            .all(|item| matches!(item, Item::Path { .. }))
    );
    if let Item::Path { transform, .. } = &converted[2] {
        assert!((transform.e - 25.0).abs() < 1e-9);
    }
}

#[test]
fn embedded_style_applies_scale_direction_bold_and_italic() {
    let mut text = audit_text(vec![]);
    if let Item::Text { style, .. } = &mut text {
        style.h_scale = 0.5;
        style.char_direction = 90;
        style.italic = true;
        style.weight = 700;
    }
    let fonts = HashMap::from([("Public:1".into(), audit_font())]);
    let converted = text_glyph_items(&text, &fonts).unwrap();
    let Item::Path {
        transform, style, ..
    } = &converted[0]
    else {
        panic!("expected outline")
    };
    assert!(transform.a.abs() < 1e-9);
    assert!((transform.b - 0.005).abs() < 1e-9);
    assert!(style.stroke && style.line_width > 0.0);
    assert_eq!((transform.e, transform.f), (2.0, 3.0));
    assert!(
        transform.d.abs() > 0.0,
        "italic shear should remain after rotation"
    );
}

#[test]
fn text_code_decodes_utf16_escapes_without_losing_literal_backslashes() {
    assert_eq!(
        decode_text_code(r"A\0020\4E2D\D83D\DE00\path"),
        "A 中😀\\path"
    );
}

#[test]
fn cgtransform_does_not_guess_ligature_or_overflow_mappings() {
    let chars = vec![
        PositionedChar {
            ch: 'A',
            x: 0.0,
            y: 0.0,
        },
        PositionedChar {
            ch: 'B',
            x: 1.0,
            y: 0.0,
        },
    ];
    for (position, count, glyph_count) in [(0, 2, 1), (usize::MAX, 2, 1)] {
        let xml = format!(
            r#"<TextObject><CGTransform CodePosition="{position}" CodeCount="{count}" GlyphCount="{glyph_count}"><Glyphs>1</Glyphs></CGTransform></TextObject>"#
        );
        let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
        assert!(positioned_glyphs(document.root_element(), &chars).is_empty());
    }
}

#[test]
fn translate_then_scale_keeps_page_position() {
    let translate = Matrix::translate(106.0, 32.5);
    let scale = Matrix {
        a: 0.89,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };
    let combined = translate.compose(scale);
    assert!((combined.a - 0.89).abs() < 1e-9);
    assert!((combined.e - 106.0).abs() < 1e-9);
    assert!((combined.f - 32.5).abs() < 1e-9);
}

fn test_char(ch: char, x: f64, line: u32, run: u32) -> PageTextChar {
    PageTextChar {
        ch,
        rect: TextMatchRect {
            x,
            y: 10.0,
            width: 1.0,
            height: 1.0,
        },
        line,
        run,
    }
}

#[test]
fn selection_text_joins_runs_and_lines() {
    let layout = PageTextLayout {
        chars: vec![
            test_char('发', 1.0, 0, 0),
            test_char('票', 2.0, 0, 0),
            test_char('测', 3.0, 0, 1),
            test_char('试', 4.0, 1, 1),
        ],
    };
    assert_eq!(layout.text_range(0, 4), "发票 测\n试");
    assert_eq!(layout.text_range(1, 3), "票 测");
    assert_eq!(layout.rects(1, 3).len(), 2);
}

#[test]
fn selection_hit_tests_rects_and_falls_back_to_distance() {
    let layout = PageTextLayout {
        chars: vec![test_char('发', 1.0, 0, 0), test_char('票', 3.0, 0, 0)],
    };
    assert_eq!(layout.index_at(1.5, 10.5, 0.0), Some(0));
    assert_eq!(layout.index_at(3.5, 10.5, 0.0), Some(1));
    assert_eq!(layout.index_at(2.8, 10.5, 1.0), Some(1));
    assert_eq!(layout.index_at(50.0, 50.0, 1.0), None);
}

#[test]
fn expands_batched_delta_x_groups() {
    let deltas = parse_delta_list("g 3 26.04 25.94 g 2 26.04");
    assert_eq!(deltas, vec![26.04, 26.04, 26.04, 25.94, 26.04, 26.04]);
}

#[test]
fn cgtransform_maps_utf16_positions_to_glyphs() {
    let xml = r#"<TextObject>
        <CGTransform CodePosition="0" CodeCount="1" GlyphCount="1"><Glyphs>7</Glyphs></CGTransform>
        <CGTransform CodePosition="1" CodeCount="2" GlyphCount="2"><Glyphs>8 9</Glyphs></CGTransform>
    </TextObject>"#;
    let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
    let chars = vec![
        PositionedChar {
            ch: '中',
            x: 0.0,
            y: 0.0,
        },
        PositionedChar {
            ch: '国',
            x: 1.0,
            y: 0.0,
        },
        PositionedChar {
            ch: 'A',
            x: 2.0,
            y: 0.0,
        },
    ];
    assert_eq!(
        positioned_glyphs(document.root_element(), &chars),
        vec![7, 8, 9]
    );
}

#[test]
fn cgtransform_handles_surrogate_pairs_and_missing_whitespace() {
    let xml = r#"<TextObject>
        <CGTransform CodePosition="0" CodeCount="2" GlyphCount="1"><Glyphs>11</Glyphs></CGTransform>
    </TextObject>"#;
    let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
    let chars = vec![
        PositionedChar {
            ch: '😀',
            x: 0.0,
            y: 0.0,
        },
        PositionedChar {
            ch: ' ',
            x: 1.0,
            y: 0.0,
        },
    ];
    assert_eq!(
        positioned_glyphs(document.root_element(), &chars),
        vec![11, 0]
    );
}

#[test]
fn cgtransform_rejects_incomplete_mappings() {
    let xml = r#"<TextObject>
        <CGTransform CodePosition="0" CodeCount="1" GlyphCount="1"><Glyphs>7</Glyphs></CGTransform>
    </TextObject>"#;
    let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
    let chars = vec![
        PositionedChar {
            ch: '中',
            x: 0.0,
            y: 0.0,
        },
        PositionedChar {
            ch: '国',
            x: 1.0,
            y: 0.0,
        },
    ];
    assert!(positioned_glyphs(document.root_element(), &chars).is_empty());
}

#[test]
fn parses_plain_delta_x_values() {
    assert_eq!(parse_delta_list("1.5 g 2 3"), vec![1.5, 3.0, 3.0]);
}

#[test]
fn parses_ofd_path_commands() {
    let mut warnings = Vec::new();
    let cmds =
        parse_path_data("M 0 0 L 10 0 Q 12 0 12 2 B 12 4 10 6 8 6 C", &mut warnings).unwrap();
    assert_eq!(cmds.len(), 5);
    assert!(warnings.is_empty());
}

#[test]
fn layer_fill_colour_does_not_turn_on_path_filling() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("path-fill.ofd");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, content) in [
        (
            "OFD.xml",
            r#"<OFD><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
        ),
        (
            "Doc/Document.xml",
            r#"<Document><CommonData><PublicRes>Res.xml</PublicRes><PageArea><PhysicalBox>0 0 100 100</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/></Pages></Document>"#,
        ),
        (
            "Doc/Res.xml",
            r#"<Res><DrawParams><DrawParam ID="2"><FillColor Value="255 0 0"/></DrawParam></DrawParams></Res>"#,
        ),
        (
            "Doc/Page.xml",
            r#"<Page><Content><Layer DrawParam="2"><PathObject Boundary="0 0 20 20"><StrokeColor Value="255 0 0"/><AbbreviatedData>M 0 0 L 20 0 L 20 20 L 0 20 C</AbbreviatedData></PathObject><TextObject Boundary="30 0 20 5" Size="3"><TextCode X="0" Y="3">A</TextCode></TextObject></Layer></Content></Page>"#,
        ),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();

    let doc = OfdDocument::open(path).unwrap();
    let page = doc.display_list(0).unwrap();
    let Item::Path { style, .. } = &page.items[0] else {
        panic!("expected path")
    };
    // OFD 8.2 表 41: no Fill attribute means no fill, even when the layer
    // DrawParam supplies a fill colour.
    assert!(!style.fill);
    assert!(style.stroke);
    assert_eq!(style.stroke_color.unwrap().hex(), "#FF0000");

    // Text still inherits the layer colour (text is filled by default).
    let Item::Text { style, .. } = &page.items[1] else {
        panic!("expected text")
    };
    assert_eq!(style.color.hex(), "#FF0000");
}
