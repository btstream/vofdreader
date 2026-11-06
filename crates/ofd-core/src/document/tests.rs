use super::{OfdDocument, decode_seal_appearance, der_field, extract_embedded_image, seal_picture};
use crate::{PageRenderer, RasterRegion};

#[test]
fn signed_value_ofd_seal_is_visible_and_transparent() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/pdf-match/1.ofd");
    let doc = OfdDocument::open(&path).unwrap();
    let stamps = doc.signature_seals();
    assert_eq!(stamps.len(), 1);
    assert_eq!(
        (stamps[0].x, stamps[0].y, stamps[0].width, stamps[0].height),
        (90.0, 8.0, 30.0, 20.0)
    );
    assert_eq!(stamps[0].mime, "image/svg+xml");
    let svg = std::str::from_utf8(&stamps[0].data).unwrap();
    assert!(svg.contains("viewBox=\"0 0 30 20\""));
    assert!(
        !svg.contains("fill=\"#FFFFFF\""),
        "no artificial paper background on a seal"
    );

    // Same document with signature appearances disabled: detect actual
    // added red ink, not merely the presence of an <image> in exported SVG.
    let bare = OfdDocument::from_seal_bytes(std::fs::read(path).unwrap()).unwrap();
    let renderer = PageRenderer::new(&doc);
    let baseline = PageRenderer::new(&bare);
    for dpi in [96.0, 384.0, 768.0] {
        let region = RasterRegion {
            x: (90.0 * dpi / 25.4) as u32,
            y: (8.0 * dpi / 25.4) as u32,
            width: (30.0 * dpi / 25.4_f32).ceil() as u32,
            height: (20.0 * dpi / 25.4_f32).ceil() as u32,
        };
        let actual = renderer.rasterize_region(&doc, 0, dpi, region).unwrap();
        let before = baseline.rasterize_region(&bare, 0, dpi, region).unwrap();
        let red_pixels = |data: &[u8]| {
            data.chunks_exact(4)
                .filter(|p| p[2] > 150 && p[1] < 100 && p[0] < 100 && p[3] > 150)
                .count()
        };
        assert!(
            red_pixels(&actual.bgra) > red_pixels(&before.bgra) + 100,
            "seal ink missing at {dpi} DPI"
        );
        // A transparent corner must leave the underlying page untouched.
        assert_eq!(&actual.bgra[..4], &before.bgra[..4]);
    }
}

#[test]
fn der_picture_fields_are_bounded_and_typed() {
    let picture = b"\x30\x10\x16\x03ofd\x04\x03zip\x02\x01\x1e\x02\x01\x14";
    let (kind, data) = seal_picture(picture, 0).unwrap();
    assert_eq!(kind, b"ofd");
    assert_eq!(data, b"zip");
    assert!(decode_seal_appearance(picture).is_none());
    for invalid in [
        b"\x30\x80".as_slice(),
        b"\x04\x88\xff\xff\xff\xff\xff\xff\xff\xff",
        b"\x04\x04abc",
        b"\x30",
    ] {
        assert!(der_field(invalid).is_none());
        assert!(decode_seal_appearance(invalid).is_none());
    }
    assert!(seal_picture(picture, 17).is_none());
    assert!(decode_seal_appearance(b"not a picture PK\x03\x04").is_none());
}

#[test]
fn extracts_png_from_an_asn1_envelope() {
    let mut data = vec![0x30, 0x82, 0x01, 0x00];
    data.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    // IHDR chunk with no payload, then IEND; extraction only scans chunks.
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(b"IEND");
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&[0, 1, 2]);
    let (image, mime) = extract_embedded_image(&data).unwrap();
    assert_eq!(mime, "image/png");
    assert!(image.starts_with(b"\x89PNG"));
    assert!(image.ends_with(&[0, 0, 0, 0]));
}

#[test]
fn returns_none_without_an_embedded_image() {
    assert!(extract_embedded_image(b"not an image").is_none());
}
