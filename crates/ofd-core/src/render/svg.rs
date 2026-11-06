use std::collections::HashMap;
use std::fmt::Write as _;

use crate::font::EmbeddedFace;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::fonts::GlyphFont;
use super::parse::blend_mode;
use super::types::*;

#[derive(Default)]
pub(crate) struct GlyphPathBuilder {
    cmds: Vec<PathCmd>,
}

impl ttf_parser::OutlineBuilder for GlyphPathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::Move(x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::Line(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.cmds
            .push(PathCmd::Quad(x1 as f64, y1 as f64, x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::Cubic(
            x1 as f64, y1 as f64, x2 as f64, y2 as f64, x as f64, y as f64,
        ));
    }
    fn close(&mut self) {
        self.cmds.push(PathCmd::Close);
    }
}

/// Draw embedded outlines using explicit GIDs first, otherwise Unicode cmap.
/// Missing outlines fall back per character, not for the entire text object.
pub(crate) fn text_glyph_items(
    item: &Item,
    fonts: &HashMap<String, GlyphFont>,
) -> Option<Vec<Item>> {
    let Item::Text {
        transform,
        chars,
        glyphs,
        style,
    } = item
    else {
        return None;
    };
    if chars.is_empty() {
        return None;
    }
    let font = fonts.get(&style.font_key)?;
    let face = EmbeddedFace::parse(&font.data, font.face_index)?;

    let mut items = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let cluster = style.clusters.iter().find(|cluster| cluster.start == index);
        let end = cluster.map_or(index + 1, |cluster| cluster.end);
        let ids: Vec<Option<u16>> = if let Some(cluster) = cluster {
            cluster
                .glyphs
                .iter()
                .map(|id| face.explicit_glyph(*id))
                .collect()
        } else if glyphs.len() == chars.len() {
            vec![face.explicit_glyph(glyphs[index])]
        } else {
            vec![face.glyph(chars[index].ch)]
        };
        let mut cluster_items = Vec::new();
        let mut advance = 0.0;
        let mut failed = false;
        for (glyph_index, glyph_id) in ids.iter().enumerate() {
            let positioned = if ids.len() == end - index {
                &chars[index + glyph_index]
            } else {
                &chars[index]
            };
            let mut builder = GlyphPathBuilder::default();
            let outlined = glyph_id
                .filter(|id| *id != 0)
                .is_some_and(|id| face.outline(id, &mut builder));
            if !outlined || builder.cmds.is_empty() {
                if end == index + 1 && positioned.ch.is_whitespace() {
                    continue;
                }
                failed = true;
                break;
            }
            let (dx, dy) = if ids.len() == end - index {
                (0.0, 0.0)
            } else {
                match style.read_direction {
                    90 => (0.0, advance),
                    180 => (-advance, 0.0),
                    270 => (0.0, -advance),
                    _ => (advance, 0.0),
                }
            };
            let position = cluster
                .and_then(|cluster| cluster.positions.get(glyph_index).copied())
                .unwrap_or((positioned.x + dx, positioned.y + dy));
            let scale = style.font_size * face.matrix().a.abs();
            let mut path_style = style.paint.clone();
            path_style.line_width /= scale.max(f64::EPSILON);
            if style.weight >= 600
                && style.weight > face.weight().saturating_add(100)
                && path_style.fill
                && !path_style.stroke
            {
                path_style.stroke = true;
                path_style.stroke_color = path_style.fill_color;
                path_style.join = "Round".into();
                path_style.line_width = (f64::from(style.weight - face.weight()) / 10000.0)
                    .min(0.06)
                    / face.matrix().a.abs().max(f64::EPSILON);
            }
            let italic = if style.italic && !face.italic() {
                12_f64.to_radians().tan()
            } else {
                0.0
            };
            let glyph_transform = transform
                .compose(Matrix::translate(position.0, position.1))
                .compose(glyph_orientation(style))
                .compose(Matrix {
                    a: style.font_size,
                    d: -style.font_size,
                    ..Matrix::IDENTITY
                })
                .compose(Matrix {
                    c: italic,
                    ..Matrix::IDENTITY
                })
                .compose(face.matrix());
            cluster_items.push(Item::Path {
                transform: glyph_transform,
                cmds: builder.cmds,
                style: path_style,
            });
            advance +=
                glyph_id.map_or(0.0, |id| face.advance(id)) * style.font_size * style.h_scale;
        }
        if failed {
            let mut fallback_style = style.clone();
            fallback_style.clusters.clear();
            items.push(Item::Text {
                transform: *transform,
                chars: chars[index..end].to_vec(),
                glyphs: Vec::new(),
                style: fallback_style,
            });
        } else {
            items.extend(cluster_items);
        }
        index = end;
    }
    if items.is_empty() { None } else { Some(items) }
}

pub(crate) fn glyph_orientation(style: &TextStyle) -> Matrix {
    let angle = f64::from(style.char_direction).to_radians();
    let (sin, cos) = angle.sin_cos();
    Matrix {
        a: cos * style.h_scale,
        b: sin * style.h_scale,
        c: -sin,
        d: cos,
        e: 0.0,
        f: 0.0,
    }
}

pub(crate) fn write_item(
    out: &mut String,
    item: &Item,
    families: Option<&HashMap<String, String>>,
) {
    write_item_with_fonts(out, item, families, None);
}

pub(crate) fn write_item_with_fonts(
    out: &mut String,
    item: &Item,
    families: Option<&HashMap<String, String>>,
    glyph_fonts: Option<&HashMap<String, GlyphFont>>,
) {
    if let Some(fonts) = glyph_fonts
        && let Some(paths) = text_glyph_items(item, fonts)
    {
        for path in &paths {
            write_item(out, path, families);
        }
        return;
    }
    match item {
        Item::Path {
            transform,
            cmds,
            style,
        } => {
            let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            let fill_gradient = style
                .fill
                .then_some(style.fill_shading.as_ref())
                .flatten()
                .map(|shading| write_gradient(out, shading));
            let stroke_gradient = style
                .stroke
                .then_some(style.stroke_shading.as_ref())
                .flatten()
                .map(|shading| write_gradient(out, shading));
            let _ = write!(out, r#"<path d="{}""#, path_data(cmds));
            if let Some(id) = &fill_gradient {
                let _ = write!(
                    out,
                    r#" fill="url(#{id})" fill-opacity="{}""#,
                    num(style.opacity)
                );
            } else if style.fill {
                if let Some(color) = style.fill_color {
                    let _ = write!(
                        out,
                        r#" fill="{}" fill-opacity="{}""#,
                        color.hex(),
                        num(color.opacity() * style.opacity)
                    );
                } else {
                    let _ = write!(out, r#" fill="none""#);
                }
            } else {
                let _ = write!(out, r#" fill="none""#);
            }
            if style.stroke {
                let paint = match (&stroke_gradient, style.stroke_color) {
                    (Some(id), _) => Some((format!("url(#{id})"), style.opacity)),
                    (None, Some(color)) => Some((color.hex(), color.opacity() * style.opacity)),
                    (None, None) => None,
                };
                if let Some((paint, opacity)) = paint {
                    let _ = write!(
                        out,
                        r#" stroke="{}" stroke-opacity="{}" stroke-width="{}""#,
                        paint,
                        num(opacity),
                        num(style.line_width)
                    );
                    if !style.dash.is_empty() {
                        let dash = style
                            .dash
                            .iter()
                            .map(|value| num(*value))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let _ = write!(out, r#" stroke-dasharray="{dash}""#);
                    }
                    let _ = write!(
                        out,
                        r#" stroke-linecap="{}" stroke-linejoin="{}""#,
                        cap(&style.cap),
                        join(&style.join)
                    );
                }
            } else {
                let _ = write!(out, r#" stroke="none""#);
            }
            let _ = writeln!(
                out,
                r#" fill-rule="{}"/></g>"#,
                if style.even_odd { "evenodd" } else { "nonzero" }
            );
        }
        Item::Text {
            transform,
            chars,
            style,
            ..
        } => {
            if style.char_direction != 0 || (style.h_scale - 1.0).abs() > 1e-9 {
                let mut plain_style = style.clone();
                plain_style.char_direction = 0;
                plain_style.h_scale = 1.0;
                for positioned in chars {
                    let plain = Item::Text {
                        transform: transform
                            .compose(Matrix::translate(positioned.x, positioned.y))
                            .compose(glyph_orientation(style)),
                        chars: vec![PositionedChar {
                            ch: positioned.ch,
                            x: 0.0,
                            y: 0.0,
                        }],
                        glyphs: Vec::new(),
                        style: plain_style.clone(),
                    };
                    write_item(out, &plain, families);
                }
                return;
            }
            let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            let color = style.color;
            let family_name = families
                .and_then(|map| map.get(&style.font_key))
                .map(String::as_str)
                .unwrap_or(&style.font_family);
            let family = css_escape(family_name);
            let generic = if style.font_family.contains("Courier") {
                "monospace"
            } else {
                "serif"
            };
            let mut start = 0;
            while start < chars.len() {
                let baseline = chars[start].y;
                let mut end = start + 1;
                while end < chars.len() && (chars[end].y - baseline).abs() < 1e-9 {
                    end += 1;
                }
                let run = &chars[start..end];
                let x_list = run
                    .iter()
                    .map(|positioned| num(positioned.x))
                    .collect::<Vec<_>>()
                    .join(" ");
                let text: String = run.iter().map(|positioned| positioned.ch).collect();
                let _ = write!(
                    out,
                    r#"<text x="{}" y="{}" font-family="'{}', 'Source Han Serif CN', 'Noto Serif CJK SC', {}" font-size="{}" fill="{}" fill-opacity="{}""#,
                    x_list,
                    num(baseline),
                    family,
                    generic,
                    num(style.font_size),
                    if style.paint.fill {
                        color.hex()
                    } else {
                        "none".into()
                    },
                    num(color.opacity() * style.opacity),
                );
                if style.paint.stroke
                    && let Some(stroke) = style.paint.stroke_color
                {
                    let _ = write!(
                        out,
                        r#" stroke="{}" stroke-opacity="{}" stroke-width="{}""#,
                        stroke.hex(),
                        num(stroke.opacity() * style.opacity),
                        num(style.paint.line_width)
                    );
                }
                if style.weight != 400 {
                    let _ = write!(out, r#" font-weight="{}""#, style.weight.clamp(100, 900));
                }
                if style.italic {
                    let _ = write!(out, r#" font-style="italic""#);
                }
                let _ = writeln!(out, r#" xml:space="preserve">{}</text>"#, escape_xml(&text));
                start = end;
            }
            out.push_str("</g>");
        }
        Item::Image {
            transform,
            width,
            height,
            mime,
            data,
            blend,
            clip,
        } => {
            let encoded = STANDARD.encode(data);
            let blend_style = blend
                .as_deref()
                .and_then(blend_mode)
                .map(|mode| format!(r#" style="mix-blend-mode:{mode}""#))
                .unwrap_or_default();
            let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            let clip_attr = if let Some((clip_x, clip_y, clip_width, clip_height)) = clip {
                static SEAL_CLIP_ID: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let id = SEAL_CLIP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = write!(
                    out,
                    r#"<defs><clipPath id="seal-clip-{id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath></defs>"#,
                    num(*clip_x),
                    num(*clip_y),
                    num(*clip_width),
                    num(*clip_height),
                );
                format!(r#" clip-path="url(#seal-clip-{id})""#)
            } else {
                String::new()
            };
            let _ = write!(
                out,
                r#"<image x="0" y="0" width="{}" height="{}" preserveAspectRatio="none"{}{} href="data:{};base64,{}"/></g>"#,
                num(*width),
                num(*height),
                blend_style,
                clip_attr,
                mime,
                encoded
            );
        }
        Item::Group {
            transform,
            items,
            opacity,
        } => {
            if *opacity < 1.0 {
                let _ = write!(
                    out,
                    r#"<g transform="{}" opacity="{}">"#,
                    transform.svg(),
                    num(*opacity)
                );
            } else {
                let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            }
            for item in items {
                write_item_with_fonts(out, item, families, glyph_fonts);
            }
            out.push_str("</g>");
        }
        Item::Clip { regions, items } => {
            // One nested group per region: nested clip-paths intersect,
            // children of one clipPath unite.
            for region in regions {
                let id = next_svg_id("clip");
                let _ = write!(
                    out,
                    r#"<defs><clipPath id="{id}" clipPathUnits="userSpaceOnUse">"#
                );
                for shape in region {
                    let _ = write!(
                        out,
                        r#"<path transform="{}" d="{}" clip-rule="{}"/>"#,
                        shape.transform.svg(),
                        path_data(&shape.cmds),
                        if shape.even_odd { "evenodd" } else { "nonzero" }
                    );
                }
                let _ = write!(out, r#"</clipPath></defs><g clip-path="url(#{id})">"#);
            }
            for item in items {
                write_item_with_fonts(out, item, families, glyph_fonts);
            }
            for _ in regions {
                out.push_str("</g>");
            }
        }
    }
}

/// Document-unique ids for inline `<defs>`.
pub(crate) fn next_svg_id(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{id}")
}

/// Writes a `userSpaceOnUse` gradient (path coordinates) and returns its id.
pub(crate) fn write_gradient(out: &mut String, shading: &Shading) -> String {
    let id = next_svg_id("shd");
    let spread = match shading.spread {
        ShadingSpread::Pad => "pad",
        ShadingSpread::Repeat => "repeat",
        ShadingSpread::Reflect => "reflect",
    };
    out.push_str("<defs>");
    match shading.kind {
        ShadingKind::Axial { start, end } => {
            let _ = write!(
                out,
                r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" spreadMethod="{spread}" x1="{}" y1="{}" x2="{}" y2="{}">"#,
                num(start.0),
                num(start.1),
                num(end.0),
                num(end.1)
            );
        }
        ShadingKind::Radial {
            start, end, radius, ..
        } => {
            let _ = write!(
                out,
                r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" spreadMethod="{spread}" cx="{}" cy="{}" r="{}" fx="{}" fy="{}">"#,
                num(end.0),
                num(end.1),
                num(radius),
                num(start.0),
                num(start.1)
            );
        }
    }
    for (offset, color) in shading.resolved_stops() {
        let _ = write!(
            out,
            r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
            num(offset),
            color.hex(),
            num(color.opacity())
        );
    }
    out.push_str(match shading.kind {
        ShadingKind::Axial { .. } => "</linearGradient></defs>",
        ShadingKind::Radial { .. } => "</radialGradient></defs>",
    });
    id
}

pub(crate) fn path_data(cmds: &[PathCmd]) -> String {
    let mut out = String::new();
    for cmd in cmds {
        let _ = match cmd {
            PathCmd::Move(x, y) => write!(out, "M {} {} ", num(*x), num(*y)),
            PathCmd::Line(x, y) => write!(out, "L {} {} ", num(*x), num(*y)),
            PathCmd::Quad(x1, y1, x2, y2) => write!(
                out,
                "Q {} {} {} {} ",
                num(*x1),
                num(*y1),
                num(*x2),
                num(*y2)
            ),
            PathCmd::Cubic(x1, y1, x2, y2, x3, y3) => write!(
                out,
                "C {} {} {} {} {} {} ",
                num(*x1),
                num(*y1),
                num(*x2),
                num(*y2),
                num(*x3),
                num(*y3)
            ),
            PathCmd::Arc {
                rx,
                ry,
                rotation,
                large,
                sweep,
                x,
                y,
            } => write!(
                out,
                "A {} {} {} {} {} {} {} ",
                num(*rx),
                num(*ry),
                num(*rotation),
                if *large { 1 } else { 0 },
                if *sweep { 1 } else { 0 },
                num(*x),
                num(*y)
            ),
            PathCmd::Close => write!(out, "Z "),
        };
    }
    out.trim_end().to_string()
}

pub(crate) fn num(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let formatted = format!("{value:.4}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

pub(crate) fn matrix_num(value: f64) -> String {
    if !value.is_finite() || value.abs() < 0.000_000_000_05 {
        return "0".into();
    }
    format!("{value:.10}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

pub(crate) fn cap(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "round" => "round",
        "square" => "square",
        _ => "butt",
    }
}

pub(crate) fn join(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "round" => "round",
        "bevel" => "bevel",
        _ => "miter",
    }
}

pub(crate) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) fn css_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}
