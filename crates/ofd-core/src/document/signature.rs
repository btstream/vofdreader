//! Signature material: seal appearances and the ASN.1 envelope around them.
//!
//! This module only extracts what must be drawn. Cryptographic verification
//! (SM2/SM3) is intentionally not implemented here; the reader reports
//! "signed, unverified" and never claims a valid signature.

use std::sync::{Arc, OnceLock};

use crate::loc::resolve_loc;
use crate::xml::{child_text, children, parse_xml};

use super::OfdDocument;

#[derive(Debug, Clone)]
pub(crate) enum SealAppearance {
    /// An embedded PNG/JPEG/SVG picture, ready to draw.
    Image {
        mime: &'static str,
        data: Arc<Vec<u8>>,
    },
    /// A nested OFD package extracted from the seal. Rendering it needs the
    /// renderer, so it is done lazily by the display builder instead of while
    /// parsing the document.
    Ofd(Arc<Vec<u8>>),
}

#[derive(Debug)]
pub(crate) struct RenderedSeal {
    pub mime: &'static str,
    pub data: Arc<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub(crate) struct SealStamp {
    pub page_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub clip: Option<(f64, f64, f64, f64)>,
    pub appearance: SealAppearance,
    /// Cached appearance converted to a drawable image, computed on first use
    /// by `render::display`.
    pub rendered: Arc<OnceLock<Option<RenderedSeal>>>,
}

pub(crate) fn load_signature_seals(doc: &OfdDocument) -> Vec<SealStamp> {
    let Some(signatures_loc) = &doc.ofd.signatures else {
        return Vec::new();
    };
    let Ok(signatures_file) = resolve_loc(&doc.entry, signatures_loc) else {
        return Vec::new();
    };
    let Ok(bytes) = doc.package.read_shared(&signatures_file) else {
        return Vec::new();
    };
    let Ok(document) = parse_xml(&bytes, &signatures_file) else {
        return Vec::new();
    };

    let mut stamps = Vec::new();
    for signature in children(document.root_element(), "Signature") {
        let Some(base_loc) = signature.attribute("BaseLoc") else {
            continue;
        };
        let Ok(signature_file) = resolve_loc(&signatures_file, base_loc) else {
            continue;
        };
        let Ok(signature_bytes) = doc.package.read_shared(&signature_file) else {
            continue;
        };
        let Ok(signature_doc) = parse_xml(&signature_bytes, &signature_file) else {
            continue;
        };
        let root = signature_doc.root_element();
        let seal_loc = root
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "Seal")
            .and_then(|seal| child_text(seal, "BaseLoc"));
        // Some producers only carry the SES seal inside SignedValue.
        let appearance = [seal_loc, child_text(root, "SignedValue")]
            .into_iter()
            .flatten()
            .find_map(|loc| {
                let file = resolve_loc(&signature_file, loc).ok()?;
                let bytes = doc.package.read(&file).ok()?;
                decode_seal_appearance(&bytes)
            });
        let Some(appearance) = appearance else {
            continue;
        };

        for annot in root
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "StampAnnot")
        {
            let Some(page_id) = annot.attribute("PageRef") else {
                continue;
            };
            let Some((x, y, width, height)) = annot
                .attribute("Boundary")
                .and_then(crate::model::parse_box)
            else {
                continue;
            };
            let clip = annot.attribute("Clip").and_then(crate::model::parse_box);
            stamps.push(SealStamp {
                page_id: page_id.to_string(),
                x,
                y,
                width,
                height,
                clip,
                appearance: appearance.clone(),
                rendered: Arc::new(OnceLock::new()),
            });
        }
    }
    stamps
}

/// Extracts the seal's drawable material without rendering it. Nested OFD
/// appearances are converted by the display builder, which has the renderer.
pub(crate) fn decode_seal_appearance(data: &[u8]) -> Option<SealAppearance> {
    if let Some((kind, picture)) = seal_picture(data, 0) {
        if kind.eq_ignore_ascii_case(b"ofd") {
            // No temporary extraction, external paths, or nested signature
            // recursion. Package applies the usual ZIP entry/size limits.
            if picture.len() > 16 * 1024 * 1024 {
                return None;
            }
            return Some(SealAppearance::Ofd(Arc::new(picture.to_vec())));
        }
        if let Some((data, mime)) = extract_embedded_image(picture) {
            return Some(SealAppearance::Image {
                mime,
                data: Arc::new(data),
            });
        }
    }
    extract_embedded_image(data).map(|(data, mime)| SealAppearance::Image {
        mime,
        data: Arc::new(data),
    })
}

/// Read one definite-length DER field, rejecting truncation and arithmetic
/// overflow. This only decodes appearance metadata; it does not verify a seal.
pub(crate) fn der_field(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&length, rest) = rest.split_first()?;
    let (length, body) = if length & 0x80 == 0 {
        (length as usize, rest)
    } else {
        let count = (length & 0x7f) as usize;
        if count == 0 || count > std::mem::size_of::<usize>() || count > rest.len() {
            return None;
        }
        let mut length = 0usize;
        for byte in &rest[..count] {
            length = length.checked_mul(256)?.checked_add(*byte as usize)?;
        }
        (length, &rest[count..])
    };
    if length > body.len() {
        return None;
    }
    Some((tag, &body[..length], &body[length..]))
}

/// SES picture info is a SEQUENCE { type IA5String, data OCTET STRING,
/// width INTEGER, height INTEGER }. Locate the typed field, not a coincidental
/// ZIP magic inside a certificate or some unrelated signed payload.
pub(crate) fn seal_picture(mut data: &[u8], depth: usize) -> Option<(&[u8], &[u8])> {
    if depth > 16 {
        return None;
    }
    while !data.is_empty() {
        let (tag, value, rest) = der_field(data)?;
        if tag == 0x30 {
            if let Some((0x16, kind, fields)) = der_field(value)
                && [
                    b"ofd".as_slice(),
                    b"png".as_slice(),
                    b"jpg".as_slice(),
                    b"jpeg".as_slice(),
                ]
                .iter()
                .any(|supported| kind.eq_ignore_ascii_case(supported))
                && let Some((0x04, picture, _)) = der_field(fields)
            {
                return Some((kind, picture));
            }
            if let Some(picture) = seal_picture(value, depth + 1) {
                return Some(picture);
            }
        }
        data = rest;
    }
    None
}

/// Seal containers are ASN.1 envelopes with a complete PNG or JPEG embedded.
/// Extract the first complete image without depending on a specific vendor's
/// ASN.1 layout.
pub(crate) fn extract_embedded_image(data: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    if let Some(start) = find_bytes(data, b"\x89PNG\r\n\x1a\n") {
        let mut position = start + 8;
        while position + 12 <= data.len() {
            let length = u32::from_be_bytes([
                data[position],
                data[position + 1],
                data[position + 2],
                data[position + 3],
            ]) as usize;
            let kind = &data[position + 4..position + 8];
            position += 12 + length;
            if position > data.len() {
                break;
            }
            if kind == b"IEND" {
                return Some((data[start..position].to_vec(), "image/png"));
            }
        }
    }
    if let Some(start) = find_bytes(data, b"\xff\xd8\xff") {
        let mut position = start + 2;
        while position + 1 < data.len() {
            if data[position] == 0xff {
                let marker = data[position + 1];
                if marker == 0xd9 {
                    return Some((data[start..position + 2].to_vec(), "image/jpeg"));
                }
                if marker == 0xda {
                    // Skip entropy-coded data until the next marker.
                    position += 2;
                    while position + 1 < data.len() {
                        if data[position] == 0xff && data[position + 1] != 0x00 {
                            break;
                        }
                        position += 1;
                    }
                    continue;
                }
                if position + 4 <= data.len() {
                    let length =
                        u16::from_be_bytes([data[position + 2], data[position + 3]]) as usize;
                    position += 2 + length;
                    continue;
                }
            }
            position += 1;
        }
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
