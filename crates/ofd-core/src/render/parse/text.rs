use crate::document::OfdDocument;
use crate::font::{EmbeddedFace, select_face};
use crate::model::{Color, FontResource};
use crate::xml::{child, child_text, children};

use super::super::text::default_advance;
use super::super::types::*;
use super::graphics::object_transform;

/// `CGTransform` maps text-code positions to glyph IDs. A `TextObject` may
/// contain several transforms, and positions count UTF-16 code units, so a
/// surrogate pair occupies two positions but produces one glyph. Build a list
/// aligned with our Rust `char` vector; an incomplete mapping falls back to the
/// Unicode cmap (or to the system font).
pub(crate) fn positioned_glyphs(
    node: roxmltree::Node<'_, '_>,
    chars: &[PositionedChar],
) -> Vec<u16> {
    let clusters = glyph_clusters(node, chars);
    let mut result = vec![None; chars.len()];
    for cluster in clusters {
        if cluster.end - cluster.start != cluster.glyphs.len() {
            return Vec::new();
        }
        for (slot, id) in result[cluster.start..cluster.end]
            .iter_mut()
            .zip(cluster.glyphs)
        {
            *slot = Some(id);
        }
    }
    result
        .into_iter()
        .zip(chars)
        .map(|(id, ch)| id.or_else(|| ch.ch.is_whitespace().then_some(0)))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

pub(crate) fn glyph_clusters(
    node: roxmltree::Node<'_, '_>,
    chars: &[PositionedChar],
) -> Vec<GlyphCluster> {
    let transforms: Vec<_> = children(node, "CGTransform").collect();
    if transforms.is_empty() {
        return Vec::new();
    }

    let mut unit_starts = Vec::with_capacity(chars.len());
    let mut unit_count = 0usize;
    for positioned in chars {
        unit_starts.push(unit_count);
        unit_count += positioned.ch.len_utf16();
    }
    let mut occupied = vec![false; chars.len()];
    let mut clusters = Vec::new();
    let mut cursor = 0usize;

    for transform in transforms {
        let glyphs: Vec<u16> = transform
            .attribute("Glyphs")
            .map(str::to_string)
            .or_else(|| child_text(transform, "Glyphs").map(str::to_string))
            .unwrap_or_default()
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<Vec<u16>, _>>()
            .unwrap_or_default();
        if glyphs.is_empty() {
            continue;
        }
        let code_position = transform
            .attribute("CodePosition")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(cursor);
        let code_count = transform
            .attribute("CodeCount")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let glyph_count = transform
            .attribute("GlyphCount")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let Some(code_end) = code_position.checked_add(code_count) else {
            return Vec::new();
        };
        if code_count == 0
            || glyph_count == 0
            || glyph_count != glyphs.len()
            || code_end > unit_count
        {
            continue;
        }
        let Ok(start_char) = unit_starts.binary_search(&code_position) else {
            return Vec::new();
        };
        let end_char = if code_end == unit_count {
            chars.len()
        } else if let Ok(index) = unit_starts.binary_search(&code_end) {
            index
        } else {
            return Vec::new();
        };
        if occupied[start_char..end_char].iter().any(|value| *value) {
            continue;
        }
        occupied[start_char..end_char].fill(true);
        if end_char - start_char == glyphs.len() {
            for (index, glyph) in (start_char..end_char).zip(glyphs) {
                clusters.push(GlyphCluster {
                    start: index,
                    end: index + 1,
                    glyphs: vec![glyph],
                    positions: Vec::new(),
                });
            }
        } else {
            clusters.push(GlyphCluster {
                start: start_char,
                end: end_char,
                glyphs,
                positions: Vec::new(),
            });
        }
        cursor = code_end;
    }

    clusters.sort_by_key(|cluster| cluster.start);
    clusters
}

pub(crate) fn text_direction(value: Option<&str>) -> u16 {
    match value.and_then(|v| v.parse().ok()) {
        Some(90) => 90,
        Some(180) => 180,
        Some(270) => 270,
        _ => 0,
    }
}

/// OFD permits UTF-16 code units escaped as a backslash plus four hex digits.
pub(crate) fn decode_text_code(text: &str) -> String {
    let mut units = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let mut lookahead = chars.clone();
            let digits: String = lookahead.by_ref().take(4).collect();
            if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                units.push(u16::from_str_radix(&digits, 16).expect("four hex digits"));
                chars = lookahead;
                continue;
            }
        }
        units.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
    }
    String::from_utf16_lossy(&units)
}

pub(crate) fn parse_text_object(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
) -> Option<Item> {
    // Text inherits FillColor/Alpha from its layer or PageBlock DrawParam, just
    // like paths. Object-level DrawParam still wins over the inherited style.
    let mut inherited_style = inherited.clone();
    if let Some(id) = node.attribute("DrawParam")
        && let Some(param) = doc.resources.draw_param(id)
    {
        inherited_style.apply(&param);
    }

    let size = node
        .attribute("Size")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(3.175);
    let font_id = node.attribute("Font").unwrap_or("");
    let font = doc.resources.font(font_id);
    let font_key = font.map(|font| font.key()).unwrap_or_default();
    let family = font
        .map(|font| font.css_family().to_string())
        .unwrap_or_else(|| "sans-serif".to_string());
    let color = child(node, "FillColor")
        .and_then(|color| {
            let value = color.attribute("Value")?;
            let alpha = color
                .attribute("Alpha")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(255);
            Color::parse(value, alpha)
        })
        .or(inherited_style.fill_color)
        .unwrap_or(Color {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });

    let object_opacity = node
        .attribute("Alpha")
        .and_then(|value| value.parse::<u16>().ok())
        .map(|alpha| alpha.min(255) as f64 / 255.0)
        .unwrap_or(1.0);

    let weight = node
        .attribute("Weight")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(if font.is_some_and(|font| font.bold) {
            700
        } else {
            400
        });
    let italic = node
        .attribute("Italic")
        .map(|value| value == "true" || value == "1")
        .unwrap_or(font.is_some_and(|font| font.italic));

    let mut paint = inherited_style.clone();
    paint.fill = node
        .attribute("Fill")
        .is_none_or(|v| v != "false" && v != "0");
    paint.stroke = node
        .attribute("Stroke")
        .is_some_and(|v| v == "true" || v == "1");
    paint.fill_color = Some(color);
    if let Some(stroke) = child(node, "StrokeColor") {
        paint.stroke_color = stroke.attribute("Value").and_then(|value| {
            Color::parse(
                value,
                stroke
                    .attribute("Alpha")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(255),
            )
        });
    }
    if let Some(width) = node
        .attribute("LineWidth")
        .and_then(|v| v.parse::<f64>().ok())
    {
        paint.line_width = width;
    }
    paint.opacity = object_opacity * inherited_style.opacity;
    let mut style = TextStyle {
        font_key,
        font_family: family,
        font_size: size,
        color,
        weight,
        italic,
        opacity: object_opacity * inherited_style.opacity,
        h_scale: node
            .attribute("HScale")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0),
        char_direction: text_direction(node.attribute("CharDirection")),
        read_direction: text_direction(node.attribute("ReadDirection")),
        paint,
        clusters: Vec::new(),
    };

    let mut chars = Vec::new();
    let mut previous_start = (0.0, 0.0);
    for code in children(node, "TextCode") {
        let text = decode_text_code(code.text().unwrap_or(""));
        let start_x = code
            .attribute("X")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(previous_start.0);
        let baseline = code
            .attribute("Y")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(previous_start.1);
        previous_start = (start_x, baseline);
        let deltas = code
            .attribute("DeltaX")
            .map(parse_delta_list)
            .unwrap_or_default();
        let delta_y = code
            .attribute("DeltaY")
            .map(parse_delta_list)
            .unwrap_or_default();
        let has_delta_x = code.attribute("DeltaX").is_some();
        let vertical = !delta_y.is_empty() && !has_delta_x;

        let mut x = start_x;
        let mut y = baseline;
        for (index, ch) in text.chars().enumerate() {
            if index > 0 {
                let advance = default_advance(ch, size) * style.h_scale;
                let (default_x, default_y) = match style.read_direction {
                    90 => (0.0, advance),
                    180 => (-advance, 0.0),
                    270 => (0.0, -advance),
                    _ if vertical => (0.0, 0.0),
                    _ => (advance, 0.0),
                };
                let advance_x = deltas.get(index - 1).copied().unwrap_or(default_x);
                x += advance_x;
                y += delta_y.get(index - 1).copied().unwrap_or(default_y);
            }
            chars.push(PositionedChar { ch, x, y });
        }
    }

    if chars.is_empty() {
        return None;
    }

    let glyphs = positioned_glyphs(node, &chars);
    style.clusters = glyph_clusters(node, &chars);
    position_glyph_clusters(doc, font, node, &mut chars, &mut style);

    Some(Item::Text {
        transform: object_transform(node),
        chars,
        glyphs,
        style,
    })
}

/// DeltaX/DeltaY address the rendered glyph stream, which may be longer or
/// shorter than the source character stream after CGTransform. Preserve the
/// source characters for copying while positioning the glyphs separately.
pub(crate) fn position_glyph_clusters(
    doc: &OfdDocument,
    font: Option<&FontResource>,
    node: roxmltree::Node<'_, '_>,
    chars: &mut [PositionedChar],
    style: &mut TextStyle,
) {
    let data = font
        .and_then(|font| font.file.as_ref())
        .and_then(|file| doc.package.read_shared(file).ok());
    let face = data
        .as_ref()
        .and_then(|data| EmbeddedFace::parse(data, font.map_or(0, |font| select_face(data, font))));
    let mut source_start = 0;
    let mut previous_start = (0.0, 0.0);
    for code in children(node, "TextCode") {
        let count = decode_text_code(code.text().unwrap_or("")).chars().count();
        let source_end = (source_start + count).min(chars.len());
        let start = (
            code.attribute("X")
                .and_then(|v| v.parse().ok())
                .unwrap_or(previous_start.0),
            code.attribute("Y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(previous_start.1),
        );
        previous_start = start;
        let dx = code
            .attribute("DeltaX")
            .map(parse_delta_list)
            .unwrap_or_default();
        let dy = code
            .attribute("DeltaY")
            .map(parse_delta_list)
            .unwrap_or_default();
        let vertical = !dy.is_empty() && code.attribute("DeltaX").is_none();
        let mut position = start;
        let mut glyph_index = 0usize;
        let mut previous_advance = 0.0;
        let mut index = source_start;
        while index < source_end {
            let cluster = style
                .clusters
                .iter_mut()
                .find(|cluster| cluster.start == index && cluster.end <= source_end);
            let end = cluster.as_ref().map_or(index + 1, |cluster| cluster.end);
            let ids = cluster.as_ref().map_or_else(
                || vec![face.as_ref().and_then(|face| face.glyph(chars[index].ch))],
                |cluster| {
                    cluster
                        .glyphs
                        .iter()
                        .map(|id| face.as_ref().and_then(|face| face.explicit_glyph(*id)))
                        .collect()
                },
            );
            let mut positions = Vec::with_capacity(ids.len());
            for id in ids {
                if glyph_index > 0 {
                    let default = match style.read_direction {
                        90 => (0.0, previous_advance),
                        180 => (-previous_advance, 0.0),
                        270 => (0.0, -previous_advance),
                        _ if vertical => (0.0, 0.0),
                        _ => (previous_advance, 0.0),
                    };
                    position.0 += dx.get(glyph_index - 1).copied().unwrap_or(default.0);
                    position.1 += dy.get(glyph_index - 1).copied().unwrap_or(default.1);
                }
                positions.push(position);
                previous_advance = if vertical || matches!(style.read_direction, 90 | 270) {
                    style.font_size
                } else {
                    id.and_then(|id| {
                        face.as_ref()
                            .map(|face| face.advance(id) * style.font_size * style.h_scale)
                    })
                    .filter(|advance| advance.is_finite() && *advance > 0.0)
                    .unwrap_or_else(|| {
                        default_advance(chars[index].ch, style.font_size) * style.h_scale
                    })
                };
                glyph_index += 1;
            }
            for (offset, ch) in chars[index..end].iter_mut().enumerate() {
                let pos = positions.get(offset).copied().unwrap_or(positions[0]);
                ch.x = pos.0;
                ch.y = pos.1;
            }
            if let Some(cluster) = cluster {
                cluster.positions = positions;
            }
            index = end;
        }
        source_start = source_end;
    }
}

/// OFD allows `g N X` to repeat the advance `X` for `N` following glyphs.
/// Producers use it heavily for long runs of fixed-pitch text.
pub(crate) fn parse_delta_list(value: &str) -> Vec<f64> {
    let mut tokens = value.split_whitespace().peekable();
    let mut deltas = Vec::new();
    while let Some(token) = tokens.next() {
        if token == "g" {
            let Some(count) = tokens.next().and_then(|value| value.parse::<usize>().ok()) else {
                continue;
            };
            let Some(advance) = tokens.next().and_then(|value| value.parse::<f64>().ok()) else {
                continue;
            };
            deltas.extend(std::iter::repeat_n(advance, count.min(10_000)));
            continue;
        }
        if let Ok(advance) = token.trim_start_matches('g').parse::<f64>() {
            deltas.push(advance);
        }
    }
    deltas
}
