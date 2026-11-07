//! Building draw operations from display-list items.

use std::hash::Hasher;
use std::sync::Arc;

use resvg::tiny_skia as sk;
use resvg::usvg::{self, fontdb};

use super::super::PositionedChar;
use super::super::parse::blend_mode;
use super::super::svg::{glyph_orientation, text_glyph_items};
use super::super::{ClipShape, Item, Matrix, PageRenderer, PathCmd, PathStyle, TextStyle};
use super::FontCache;
use super::fonts::{select_face, system_outline};
use super::ops::{Bounds, ClipOutline, Op, Prepared};
use super::path::{
    build_path, color, hash_cmd, opacity_u8, push_path, push_stroke, stroke_style, to_ts,
};
use crate::model::SizeMm;

pub(crate) fn prepare(renderer: &PageRenderer, size: SizeMm, items: &[Item]) -> Prepared {
    let page = Bounds {
        x0: 0.0,
        y0: 0.0,
        x1: size.width as f32,
        y1: size.height as f32,
    };
    let mut builder = Builder {
        renderer,
        page,
        fonts: renderer
            .font_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    };
    let mut ops = Vec::with_capacity(items.len() + 1);
    // Paper, painted as a path like the former SVG `<rect>` so partially
    // covered edge pixels stay identical.
    if let Some(rect) = sk::Rect::from_xywh(0.0, 0.0, size.width as f32, size.height as f32) {
        let path = sk::PathBuilder::from_rect(rect);
        if let Some(bounds) = Bounds::of_rect(rect, sk::Transform::identity()) {
            ops.push(Op::Fill {
                path: Arc::new(path),
                ts: sk::Transform::identity(),
                shader: sk::Shader::SolidColor(sk::Color::WHITE),
                rule: sk::FillRule::Winding,
                bounds,
            });
        }
    }
    for item in items {
        builder.item(item, Matrix::IDENTITY, &mut ops);
    }
    Prepared { size, ops }
}

struct Builder<'a> {
    renderer: &'a PageRenderer,
    /// Page rectangle in millimetres; nothing outside it is ever visible.
    page: Bounds,
    fonts: std::sync::MutexGuard<'a, FontCache>,
}

impl Builder<'_> {
    fn item(&mut self, item: &Item, parent: Matrix, out: &mut Vec<Op>) {
        match item {
            Item::Path {
                transform,
                cmds,
                style,
            } => {
                if let Some(path) = build_path(cmds) {
                    push_path(Arc::new(path), parent.compose(*transform), style, out);
                }
            }
            Item::Text { .. } => {
                if let Some(converted) = text_glyph_items(item, &self.renderer.glyph_fonts) {
                    for converted in &converted {
                        match converted {
                            Item::Path {
                                transform,
                                cmds,
                                style,
                            } => {
                                if let Some(path) = self.embedded_glyph(cmds) {
                                    push_path(path, parent.compose(*transform), style, out);
                                }
                            }
                            Item::Text {
                                transform,
                                chars,
                                style,
                                ..
                            } => self.system_text(parent.compose(*transform), chars, style, out),
                            _ => {}
                        }
                    }
                } else if let Item::Text {
                    transform,
                    chars,
                    style,
                    ..
                } = item
                {
                    self.system_text(parent.compose(*transform), chars, style, out);
                }
            }
            Item::Image {
                transform,
                width,
                height,
                mime,
                data,
                blend,
                clip,
            } => self.image(
                parent.compose(*transform),
                (*width, *height),
                mime,
                data,
                blend.as_deref(),
                *clip,
                out,
            ),
            Item::Group {
                transform,
                items,
                opacity,
            } => {
                if opacity_u8(*opacity) == 0 {
                    return;
                }
                let matrix = parent.compose(*transform);
                if *opacity < 1.0 {
                    let mut ops = Vec::new();
                    for item in items {
                        self.item(item, matrix, &mut ops);
                    }
                    let Some(bounds) = ops.iter().map(Op::bounds).reduce(Bounds::union) else {
                        return;
                    };
                    out.push(Op::Layer {
                        ops,
                        opacity: opacity_u8(*opacity) as f32 / 255.0,
                        bounds,
                    });
                } else {
                    for item in items {
                        self.item(item, matrix, out);
                    }
                }
            }
            Item::Clip { regions, items } => {
                let mut ops = Vec::new();
                for item in items {
                    self.item(item, parent, &mut ops);
                }
                let Some(content) = ops.iter().map(Op::bounds).reduce(Bounds::union) else {
                    return;
                };
                // Only the on-page part of the content can ever be seen.
                let Some(visible_content) = content.intersection(&self.page) else {
                    return;
                };
                let mut bounds = visible_content;
                let mut outlines = Vec::with_capacity(regions.len());
                for region in regions {
                    let region = clip_region(region, parent);
                    let Some(area) = region
                        .iter()
                        .filter_map(|outline| Bounds::of_rect(outline.path.bounds(), outline.ts))
                        .reduce(Bounds::union)
                    else {
                        // Nothing in this region is visible.
                        return;
                    };
                    let Some(visible) = bounds.intersection(&area) else {
                        return;
                    };
                    bounds = visible;
                    // A rectangle around everything visible is a no-op;
                    // producers emit page-sized ones on nearly every object.
                    // 0.05 mm (0.2 px at 96 DPI) absorbs pt→mm rounding:
                    // page clips often start ~0.02 mm inside the page.
                    if !(region.len() == 1
                        && is_axis_rect(&region[0])
                        && area.contains(&visible_content, 0.05))
                    {
                        outlines.push(region);
                    }
                }
                if outlines.is_empty() {
                    out.extend(ops);
                } else {
                    out.push(Op::Clip {
                        ops,
                        regions: outlines,
                        bounds,
                    });
                }
            }
        }
    }

    /// Identical glyph outlines (same commands) share one path.
    fn embedded_glyph(&mut self, cmds: &[PathCmd]) -> Option<Arc<sk::Path>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for cmd in cmds {
            hash_cmd(cmd, &mut hasher);
        }
        let key = (cmds.len(), hasher.finish());
        if let Some(path) = self.fonts.embedded.get(&key) {
            return Some(path.clone());
        }
        let path = Arc::new(build_path(cmds)?);
        self.fonts.embedded.insert(key, path.clone());
        Some(path)
    }

    fn system_text(
        &mut self,
        transform: Matrix,
        chars: &[PositionedChar],
        style: &TextStyle,
        out: &mut Vec<Op>,
    ) {
        let family = self
            .renderer
            .font_aliases
            .get(&style.font_key)
            .cloned()
            .unwrap_or_else(|| style.font_family.clone());
        let generic = if style.font_family.contains("Courier") {
            "monospace"
        } else {
            "serif"
        };
        let weight = if style.weight == 400 {
            400
        } else {
            style.weight.clamp(100, 900)
        };
        let db = &self.renderer.fontdb;
        let base = *self
            .fonts
            .base
            .entry((family.clone(), generic, weight, style.italic))
            .or_insert_with(|| {
                let generic = if generic == "monospace" {
                    fontdb::Family::Monospace
                } else {
                    fontdb::Family::Serif
                };
                db.query(&fontdb::Query {
                    families: &[
                        fontdb::Family::Name(&family),
                        fontdb::Family::Name("Source Han Serif CN"),
                        fontdb::Family::Name("Noto Serif CJK SC"),
                        generic,
                        fontdb::Family::Serif,
                    ],
                    weight: fontdb::Weight(weight),
                    stretch: fontdb::Stretch::Normal,
                    style: if style.italic {
                        fontdb::Style::Italic
                    } else {
                        fontdb::Style::Normal
                    },
                })
            });
        let Some(base) = base else { return };
        let orientation = glyph_orientation(style);
        let fill = style.paint.fill.then(|| {
            color(
                style.color.r,
                style.color.g,
                style.color.b,
                style.color.opacity() * style.opacity,
            )
        });
        let stroke = style
            .paint
            .stroke
            .then_some(style.paint.stroke_color)
            .flatten()
            .and_then(|stroke_color| {
                let stroke = stroke_style(&PathStyle {
                    cap: "Butt".into(),
                    join: "Miter".into(),
                    dash: Vec::new(),
                    ..style.paint.clone()
                })?;
                Some((
                    stroke,
                    color(
                        stroke_color.r,
                        stroke_color.g,
                        stroke_color.b,
                        stroke_color.opacity() * style.opacity,
                    ),
                ))
            });
        for positioned in chars {
            if positioned.ch.is_whitespace() || positioned.ch.is_control() {
                continue;
            }
            let face = *self
                .fonts
                .fallback
                .entry((base, positioned.ch))
                .or_insert_with(|| select_face(db, base, positioned.ch));
            let Some(face) = face else { continue };
            let glyph = self
                .fonts
                .system_glyphs
                .entry((face, positioned.ch))
                .or_insert_with(|| system_outline(db, face, positioned.ch))
                .clone();
            let Some((path, units_per_em)) = glyph else {
                continue;
            };
            let scale = style.font_size / f64::from(units_per_em);
            let space = transform
                .compose(Matrix::translate(positioned.x, positioned.y))
                .compose(orientation);
            let glyph_matrix = space.compose(Matrix {
                a: scale,
                d: -scale,
                ..Matrix::IDENTITY
            });
            if let Some(color) = fill {
                let ts = to_ts(glyph_matrix);
                if let Some(bounds) = Bounds::of_rect(path.bounds(), ts) {
                    out.push(Op::Fill {
                        path: path.clone(),
                        ts,
                        shader: sk::Shader::SolidColor(color),
                        rule: sk::FillRule::Winding,
                        bounds,
                    });
                }
            }
            if let Some((stroke, color)) = &stroke {
                // SVG text strokes are sized in the text's user space.
                let local = (*path)
                    .clone()
                    .transform(sk::Transform::from_scale(scale as f32, -scale as f32));
                if let Some(local) = local {
                    push_stroke(
                        Arc::new(local),
                        to_ts(space),
                        stroke.clone(),
                        sk::Shader::SolidColor(*color),
                        out,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn image(
        &mut self,
        matrix: Matrix,
        (width, height): (f64, f64),
        mime: &str,
        data: &[u8],
        blend: Option<&str>,
        clip: Option<(f64, f64, f64, f64)>,
        out: &mut Vec<Op>,
    ) {
        if !(width > 0.0 && height > 0.0) {
            return;
        }
        let blend =
            blend
                .and_then(blend_mode)
                .map_or(sk::BlendMode::SourceOver, |mode| match mode {
                    "darken" => sk::BlendMode::Darken,
                    "multiply" => sk::BlendMode::Multiply,
                    "screen" => sk::BlendMode::Screen,
                    "overlay" => sk::BlendMode::Overlay,
                    "lighten" => sk::BlendMode::Lighten,
                    _ => sk::BlendMode::SourceOver,
                });
        if mime == "image/svg+xml" {
            let options = usvg::Options {
                fontdb: self.renderer.fontdb.clone(),
                font_size: 12.0,
                ..Default::default()
            };
            let Ok(tree) = usvg::Tree::from_data(data, &options) else {
                return;
            };
            let (tw, th) = (
                f64::from(tree.size().width()),
                f64::from(tree.size().height()),
            );
            let ts = to_ts(matrix.compose(Matrix {
                a: width / tw,
                d: height / th,
                ..Matrix::IDENTITY
            }));
            let full = sk::Rect::from_xywh(0.0, 0.0, tw as f32, th as f32);
            let clip = clip.and_then(|(x, y, w, h)| {
                sk::Rect::from_xywh(
                    (x * tw / width) as f32,
                    (y * th / height) as f32,
                    (w * tw / width) as f32,
                    (h * th / height) as f32,
                )
            });
            let visible = clip.and_then(|clip| full?.intersect(&clip)).or(full);
            let Some(bounds) = visible.and_then(|rect| Bounds::of_rect(rect, ts)) else {
                return;
            };
            out.push(Op::Vector {
                tree: Arc::new(tree),
                ts,
                clip,
                blend,
                bounds,
            });
            return;
        }
        let Some(image) = self
            .renderer
            .image_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_decode(data, &self.renderer.image_cache_limit)
        else {
            return;
        };
        let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
        let ts = to_ts(matrix.compose(Matrix {
            a: width / iw,
            d: height / ih,
            ..Matrix::IDENTITY
        }));
        let Some(full) = sk::Rect::from_xywh(0.0, 0.0, iw as f32, ih as f32) else {
            return;
        };
        let rect = match clip {
            Some((x, y, w, h)) => {
                let clip = sk::Rect::from_xywh(
                    (x * iw / width) as f32,
                    (y * ih / height) as f32,
                    (w * iw / width) as f32,
                    (h * ih / height) as f32,
                );
                match clip.and_then(|clip| full.intersect(&clip)) {
                    Some(rect) => rect,
                    None => return,
                }
            }
            None => full,
        };
        let Some(bounds) = Bounds::of_rect(rect, ts) else {
            return;
        };
        out.push(Op::Image {
            image,
            ts,
            rect,
            blend,
            bounds,
        });
    }
}

fn clip_region(shapes: &[ClipShape], parent: Matrix) -> Vec<ClipOutline> {
    shapes
        .iter()
        .filter_map(|shape| {
            Some(ClipOutline {
                path: build_path(&shape.cmds)?,
                ts: to_ts(parent.compose(shape.transform)),
                rule: if shape.even_odd {
                    sk::FillRule::EvenOdd
                } else {
                    sk::FillRule::Winding
                },
            })
        })
        .collect()
}

/// An outline that is an axis-aligned rectangle in page space.
fn is_axis_rect(outline: &ClipOutline) -> bool {
    if outline.ts.kx != 0.0 || outline.ts.ky != 0.0 {
        return false;
    }
    let (mut moves, mut lines) = (0, 0);
    for segment in outline.path.segments() {
        match segment {
            sk::PathSegment::MoveTo(_) => moves += 1,
            sk::PathSegment::LineTo(_) => lines += 1,
            sk::PathSegment::Close => {}
            _ => return false,
        }
    }
    let bounds = outline.path.bounds();
    let on_edge = |value: f32, a: f32, b: f32| {
        let tolerance = 1e-4 * (1.0 + a.abs().max(b.abs()));
        (value - a).abs() <= tolerance || (value - b).abs() <= tolerance
    };
    let points = outline.path.points();
    let straight = |a: &sk::Point, b: &sk::Point| a.x == b.x || a.y == b.y;
    moves == 1
        && (3..=4).contains(&lines)
        && points.iter().all(|point| {
            on_edge(point.x, bounds.left(), bounds.right())
                && on_edge(point.y, bounds.top(), bounds.bottom())
        })
        && points.windows(2).all(|pair| straight(&pair[0], &pair[1]))
        && straight(&points[0], &points[points.len() - 1])
}
