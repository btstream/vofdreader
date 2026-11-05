//! Direct DisplayList rasterizer: no SVG serialization and no usvg parse.
//!
//! A page is prepared once into flat draw operations (group transforms are
//! composed up front, only translucent groups stay nested as layers). Every
//! operation carries page-space bounds, so a tile only paints what it hits.
//! Paint semantics deliberately mirror the SVG/resvg path this replaces:
//! same tiny-skia rasterizer, opacity rounding, dash rules, image filtering
//! and usvg's system-font fallback order.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use resvg::tiny_skia as sk;
use resvg::usvg::{self, fontdb};

use super::{
    Item, Matrix, PageRenderer, PathCmd, PathStyle, TextStyle, blend_mode, cap, glyph_orientation,
    join, text_glyph_items,
};
use crate::model::SizeMm;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Bounds {
    fn of_rect(rect: sk::Rect, ts: sk::Transform) -> Option<Self> {
        let mut points = [
            sk::Point::from_xy(rect.left(), rect.top()),
            sk::Point::from_xy(rect.right(), rect.top()),
            sk::Point::from_xy(rect.right(), rect.bottom()),
            sk::Point::from_xy(rect.left(), rect.bottom()),
        ];
        ts.map_points(&mut points);
        let mut bounds = Self {
            x0: f32::INFINITY,
            y0: f32::INFINITY,
            x1: f32::NEG_INFINITY,
            y1: f32::NEG_INFINITY,
        };
        for point in points {
            bounds.x0 = bounds.x0.min(point.x);
            bounds.y0 = bounds.y0.min(point.y);
            bounds.x1 = bounds.x1.max(point.x);
            bounds.y1 = bounds.y1.max(point.y);
        }
        [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
            .iter()
            .all(|v| v.is_finite())
            .then_some(bounds)
    }

    fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    fn intersects(&self, other: &Self) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1 && self.y0 <= other.y1 && other.y0 <= self.y1
    }

    fn rect(self) -> Option<sk::Rect> {
        sk::Rect::from_ltrb(
            self.x0,
            self.y0,
            self.x1.max(self.x0 + f32::EPSILON),
            self.y1.max(self.y0 + f32::EPSILON),
        )
    }
}

enum Op {
    Fill {
        path: Arc<sk::Path>,
        ts: sk::Transform,
        color: sk::Color,
        rule: sk::FillRule,
        bounds: Bounds,
    },
    Stroke {
        path: Arc<sk::Path>,
        ts: sk::Transform,
        color: sk::Color,
        stroke: sk::Stroke,
        bounds: Bounds,
    },
    Image {
        image: Arc<sk::Pixmap>,
        /// Image pixel space to page millimetres.
        ts: sk::Transform,
        /// Visible part in image pixels (whole image or seal clip).
        rect: sk::Rect,
        blend: sk::BlendMode,
        bounds: Bounds,
    },
    Vector {
        tree: Arc<usvg::Tree>,
        /// SVG pixel space to page millimetres.
        ts: sk::Transform,
        clip: Option<sk::Rect>,
        blend: sk::BlendMode,
        bounds: Bounds,
    },
    Layer {
        ops: Vec<Op>,
        opacity: f32,
        bounds: Bounds,
    },
}

impl Op {
    fn bounds(&self) -> Bounds {
        match self {
            Op::Fill { bounds, .. }
            | Op::Stroke { bounds, .. }
            | Op::Image { bounds, .. }
            | Op::Vector { bounds, .. }
            | Op::Layer { bounds, .. } => *bounds,
        }
    }
}

/// A page ready for repeated rasterization at any density or region.
pub(crate) struct Prepared {
    pub size: SizeMm,
    ops: Vec<Op>,
}

impl Prepared {
    /// Device size, rounded exactly like the former `width="..mm"` SVG root.
    pub fn pixel_size(&self, dpi: f32) -> (u32, u32) {
        let px = |mm: f64| {
            let rounded = (mm * 10_000.0).round() / 10_000.0;
            // usvg: `length as f32 * dpi / 25.4`, all in f32.
            let svg_px = rounded as f32 * 96.0_f32 / 25.4_f32;
            (svg_px * (dpi / 96.0)).ceil().max(1.0) as u32
        };
        (px(self.size.width), px(self.size.height))
    }

    /// Paint into `pixmap`; `base` maps page millimetres to its pixels.
    pub fn render(&self, base: sk::Transform, pixmap: &mut sk::PixmapMut) {
        // Visible page rectangle plus an anti-aliasing margin of 2 pixels.
        let Some(inverse) = base.invert() else {
            return;
        };
        let device = sk::Rect::from_xywh(
            -2.0,
            -2.0,
            pixmap.width() as f32 + 4.0,
            pixmap.height() as f32 + 4.0,
        )
        .expect("pixmap size is positive");
        let Some(view) = Bounds::of_rect(device, inverse) else {
            return;
        };
        draw(&self.ops, base, &view, pixmap);
    }
}

fn draw(ops: &[Op], base: sk::Transform, view: &Bounds, pixmap: &mut sk::PixmapMut) {
    for op in ops {
        if !op.bounds().intersects(view) {
            continue;
        }
        match op {
            Op::Fill {
                path,
                ts,
                color,
                rule,
                ..
            } => {
                let paint = sk::Paint {
                    shader: sk::Shader::SolidColor(*color),
                    anti_alias: true,
                    ..sk::Paint::default()
                };
                pixmap.fill_path(path, &paint, *rule, base.pre_concat(*ts), None);
            }
            Op::Stroke {
                path,
                ts,
                color,
                stroke,
                ..
            } => {
                let paint = sk::Paint {
                    shader: sk::Shader::SolidColor(*color),
                    anti_alias: true,
                    ..sk::Paint::default()
                };
                pixmap.stroke_path(path, &paint, stroke, base.pre_concat(*ts), None);
            }
            Op::Image {
                image,
                ts,
                rect,
                blend,
                ..
            } => {
                let paint = sk::Paint {
                    shader: sk::Pattern::new(
                        image.as_ref().as_ref(),
                        sk::SpreadMode::Pad,
                        sk::FilterQuality::Bicubic,
                        1.0,
                        sk::Transform::identity(),
                    ),
                    blend_mode: *blend,
                    anti_alias: true,
                    ..sk::Paint::default()
                };
                pixmap.fill_rect(*rect, &paint, base.pre_concat(*ts), None);
            }
            Op::Vector {
                tree,
                ts,
                clip,
                blend,
                ..
            } => {
                let transform = base.pre_concat(*ts);
                if clip.is_none() && *blend == sk::BlendMode::SourceOver {
                    resvg::render(tree, transform, pixmap);
                    continue;
                }
                let Some(mut layer) = sk::Pixmap::new(pixmap.width(), pixmap.height()) else {
                    continue;
                };
                resvg::render(tree, transform, &mut layer.as_mut());
                let mask = clip.and_then(|clip| {
                    let mut mask = sk::Mask::new(pixmap.width(), pixmap.height())?;
                    mask.fill_path(
                        &sk::PathBuilder::from_rect(clip),
                        sk::FillRule::Winding,
                        true,
                        transform,
                    );
                    Some(mask)
                });
                pixmap.draw_pixmap(
                    0,
                    0,
                    layer.as_ref(),
                    &sk::PixmapPaint {
                        blend_mode: *blend,
                        ..sk::PixmapPaint::default()
                    },
                    sk::Transform::identity(),
                    mask.as_ref(),
                );
            }
            Op::Layer {
                ops,
                opacity,
                bounds,
            } => {
                let Some(rect) = bounds.rect() else { continue };
                let Some(device) = Bounds::of_rect(rect, base) else {
                    continue;
                };
                let x0 = (device.x0.floor() as i64 - 2).max(0);
                let y0 = (device.y0.floor() as i64 - 2).max(0);
                let x1 = (device.x1.ceil() as i64 + 2).min(i64::from(pixmap.width()));
                let y1 = (device.y1.ceil() as i64 + 2).min(i64::from(pixmap.height()));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let Some(mut layer) = sk::Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32) else {
                    continue;
                };
                let shifted = base.post_translate(-(x0 as f32), -(y0 as f32));
                let Some(inverse) = shifted.invert() else {
                    continue;
                };
                let local = sk::Rect::from_xywh(
                    -2.0,
                    -2.0,
                    layer.width() as f32 + 4.0,
                    layer.height() as f32 + 4.0,
                )
                .and_then(|r| Bounds::of_rect(r, inverse));
                let Some(local) = local else { continue };
                draw(ops, shifted, &local, &mut layer.as_mut());
                pixmap.draw_pixmap(
                    x0 as i32,
                    y0 as i32,
                    layer.as_ref(),
                    &sk::PixmapPaint {
                        opacity: *opacity,
                        ..sk::PixmapPaint::default()
                    },
                    sk::Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// A system-font outline in font units, with that face's units per em.
type SystemGlyph = Option<(Arc<sk::Path>, f32)>;

/// Glyph outlines and font choices shared by all pages of one renderer.
#[derive(Default)]
pub(crate) struct FontCache {
    /// (family, generic, weight, italic) → base face, as usvg's query.
    base: HashMap<(String, &'static str, u16, bool), Option<fontdb::ID>>,
    /// (base face, char) → face actually used, as usvg's fallback selector.
    fallback: HashMap<(fontdb::ID, char), Option<fontdb::ID>>,
    /// (face, char) → outline in font units plus units per em.
    system_glyphs: HashMap<(fontdb::ID, char), SystemGlyph>,
    /// Embedded glyph outlines by (font key, path command identity).
    embedded: HashMap<(usize, u64), Arc<sk::Path>>,
}

/// Decoded images shared across pages (templates and seals repeat).
#[derive(Default)]
pub(crate) struct ImageCache {
    entries: HashMap<(u64, usize), Arc<sk::Pixmap>>,
    bytes: usize,
}

const IMAGE_CACHE_BYTES: usize = 256 * 1024 * 1024;

impl ImageCache {
    fn get_or_decode(&mut self, data: &[u8]) -> Option<Arc<sk::Pixmap>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut hasher);
        let key = (hasher.finish(), data.len());
        if let Some(image) = self.entries.get(&key) {
            return Some(image.clone());
        }
        let image = Arc::new(decode_raster(data)?);
        let size = image.data().len();
        if self.bytes + size > IMAGE_CACHE_BYTES {
            // Pages that still use an image keep their own Arc.
            self.entries.clear();
            self.bytes = 0;
        }
        self.bytes += size;
        self.entries.insert(key, image.clone());
        Some(image)
    }
}

pub(crate) fn prepare(renderer: &PageRenderer, size: SizeMm, items: &[Item]) -> Prepared {
    let mut builder = Builder {
        renderer,
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
                color: sk::Color::WHITE,
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
        chars: &[super::PositionedChar],
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
                        color,
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
                    push_stroke(Arc::new(local), to_ts(space), stroke.clone(), *color, out);
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
            .get_or_decode(data)
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

fn push_path(path: Arc<sk::Path>, matrix: Matrix, style: &PathStyle, out: &mut Vec<Op>) {
    let ts = to_ts(matrix);
    let path_bounds = path.bounds();
    // usvg drops fills of zero-area paths (e.g. a filled straight line).
    if style.fill
        && let Some(fill) = style.fill_color
        && path_bounds.width() > 0.0
        && path_bounds.height() > 0.0
        && let Some(bounds) = Bounds::of_rect(path_bounds, ts)
    {
        out.push(Op::Fill {
            path: path.clone(),
            ts,
            color: color(fill.r, fill.g, fill.b, fill.opacity() * style.opacity),
            rule: if style.even_odd {
                sk::FillRule::EvenOdd
            } else {
                sk::FillRule::Winding
            },
            bounds,
        });
    }
    if style.stroke
        && let Some(stroke_color) = style.stroke_color
        && let Some(stroke) = stroke_style(style)
    {
        let color = color(
            stroke_color.r,
            stroke_color.g,
            stroke_color.b,
            stroke_color.opacity() * style.opacity,
        );
        push_stroke(path, ts, stroke, color, out);
    }
}

fn push_stroke(
    path: Arc<sk::Path>,
    ts: sk::Transform,
    stroke: sk::Stroke,
    color: sk::Color,
    out: &mut Vec<Op>,
) {
    // Miter joins extend at most limit/2 widths; square caps less.
    let reach = stroke.width * stroke.miter_limit.max(1.0) * 0.5 + stroke.width;
    let bounds = path.bounds();
    let Some(outset) = sk::Rect::from_ltrb(
        bounds.left() - reach,
        bounds.top() - reach,
        bounds.right() + reach,
        bounds.bottom() + reach,
    ) else {
        return;
    };
    let Some(bounds) = Bounds::of_rect(outset, ts) else {
        return;
    };
    out.push(Op::Stroke {
        path,
        ts,
        color,
        stroke,
        bounds,
    });
}

fn stroke_style(style: &PathStyle) -> Option<sk::Stroke> {
    // The SVG path wrote widths with four decimals; smaller ones became 0,
    // which SVG treats as no stroke (tiny-skia would draw a hairline).
    let width = round4(style.line_width);
    if !(width.is_finite() && width > 0.0) {
        return None;
    }
    let dash = if style.dash.is_empty() {
        None
    } else {
        let mut list: Vec<f32> = style.dash.iter().map(|v| round4(*v) as f32).collect();
        if list.iter().any(|v| v.is_sign_negative()) || list.iter().sum::<f32>() <= 0.0 {
            None
        } else {
            if list.len() % 2 == 1 {
                list.extend_from_within(..);
            }
            sk::StrokeDash::new(list, 0.0)
        }
    };
    Some(sk::Stroke {
        width: width as f32,
        miter_limit: 4.0,
        line_cap: match cap(&style.cap) {
            "round" => sk::LineCap::Round,
            "square" => sk::LineCap::Square,
            _ => sk::LineCap::Butt,
        },
        line_join: match join(&style.join) {
            "round" => sk::LineJoin::Round,
            "bevel" => sk::LineJoin::Bevel,
            _ => sk::LineJoin::Miter,
        },
        dash,
    })
}

fn round4(value: f64) -> f64 {
    if value.is_finite() {
        (value * 10_000.0).round() / 10_000.0
    } else {
        0.0
    }
}

fn opacity_u8(opacity: f64) -> u8 {
    // usvg rounds SVG opacity (written with four decimals) the same way.
    (round4(opacity).clamp(0.0, 1.0) as f32 * 255.0 + 0.5) as u8
}

fn color(r: u8, g: u8, b: u8, opacity: f64) -> sk::Color {
    sk::Color::from_rgba8(r, g, b, opacity_u8(opacity))
}

fn to_ts(m: Matrix) -> sk::Transform {
    sk::Transform::from_row(
        m.a as f32, m.b as f32, m.c as f32, m.d as f32, m.e as f32, m.f as f32,
    )
}

fn hash_cmd(cmd: &PathCmd, hasher: &mut impl Hasher) {
    let values: &[f64] = match cmd {
        PathCmd::Move(x, y) => {
            0u8.hash(hasher);
            &[*x, *y]
        }
        PathCmd::Line(x, y) => {
            1u8.hash(hasher);
            &[*x, *y]
        }
        PathCmd::Quad(a, b, c, d) => {
            2u8.hash(hasher);
            &[*a, *b, *c, *d]
        }
        PathCmd::Cubic(a, b, c, d, e, f) => {
            3u8.hash(hasher);
            &[*a, *b, *c, *d, *e, *f]
        }
        PathCmd::Arc {
            rx,
            ry,
            rotation,
            large,
            sweep,
            x,
            y,
        } => {
            (4u8, *large, *sweep).hash(hasher);
            &[*rx, *ry, *rotation, *x, *y]
        }
        PathCmd::Close => {
            5u8.hash(hasher);
            &[]
        }
    };
    for value in values {
        value.to_bits().hash(hasher);
    }
}

fn build_path(cmds: &[PathCmd]) -> Option<sk::Path> {
    let mut builder = sk::PathBuilder::new();
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for cmd in cmds {
        match *cmd {
            PathCmd::Move(x, y) => {
                builder.move_to(x as f32, y as f32);
                current = (x, y);
                start = current;
            }
            PathCmd::Line(x, y) => {
                builder.line_to(x as f32, y as f32);
                current = (x, y);
            }
            PathCmd::Quad(x1, y1, x, y) => {
                builder.quad_to(x1 as f32, y1 as f32, x as f32, y as f32);
                current = (x, y);
            }
            PathCmd::Cubic(x1, y1, x2, y2, x, y) => {
                builder.cubic_to(
                    x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32,
                );
                current = (x, y);
            }
            PathCmd::Arc {
                rx,
                ry,
                rotation,
                large,
                sweep,
                x,
                y,
            } => {
                arc_to(
                    &mut builder,
                    current,
                    rx,
                    ry,
                    rotation,
                    large,
                    sweep,
                    (x, y),
                );
                current = (x, y);
            }
            PathCmd::Close => {
                builder.close();
                current = start;
            }
        }
    }
    builder.finish()
}

/// SVG elliptical arc (endpoint form, SVG 1.1 F.6.5) as cubic Béziers.
#[allow(clippy::too_many_arguments)]
fn arc_to(
    builder: &mut sk::PathBuilder,
    (x1, y1): (f64, f64),
    rx: f64,
    ry: f64,
    rotation: f64,
    large: bool,
    sweep: bool,
    (x2, y2): (f64, f64),
) {
    if (x1 - x2).abs() < 1e-12 && (y1 - y2).abs() < 1e-12 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        builder.line_to(x2 as f32, y2 as f32);
        return;
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos * dx + sin * dy;
    let y1p = -sin * dx + cos * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }
    let numerator = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let denominator = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coefficient = if denominator > 0.0 {
        (numerator / denominator).max(0.0).sqrt()
    } else {
        0.0
    };
    if large == sweep {
        coefficient = -coefficient;
    }
    let cxp = coefficient * rx * y1p / ry;
    let cyp = -coefficient * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
    let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let start = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    let segments = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / segments as f64;
    let t = 4.0 / 3.0 * (step / 4.0).tan();
    let map = |ux: f64, uy: f64| {
        (
            (cx + cos * rx * ux - sin * ry * uy) as f32,
            (cy + sin * rx * ux + cos * ry * uy) as f32,
        )
    };
    for index in 0..segments {
        let a1 = start + step * index as f64;
        let a2 = a1 + step;
        let (s1, c1) = a1.sin_cos();
        let (s2, c2) = a2.sin_cos();
        let p1 = map(c1 - t * s1, s1 + t * c1);
        let p2 = map(c2 + t * s2, s2 - t * c2);
        let end = if index + 1 == segments {
            (x2 as f32, y2 as f32)
        } else {
            map(c2, s2)
        };
        builder.cubic_to(p1.0, p1.1, p2.0, p2.1, end.0, end.1);
    }
}

fn face_has_char(db: &fontdb::Database, id: fontdb::ID, ch: char) -> bool {
    db.with_face_data(id, |data, index| {
        ttf_parser::Face::parse(data, index)
            .ok()
            .and_then(|face| face.glyph_index(ch))
            .is_some_and(|glyph| glyph.0 != 0)
    })
    .unwrap_or(false)
}

/// usvg's default fallback: base face if it maps the char, otherwise the
/// first face in database order with a compatible style that does.
fn select_face(db: &fontdb::Database, base: fontdb::ID, ch: char) -> Option<fontdb::ID> {
    if face_has_char(db, base, ch) {
        return Some(base);
    }
    let base_face = db.face(base)?;
    for face in db.faces() {
        if face.id == base {
            continue;
        }
        if base_face.style != face.style
            && base_face.weight != face.weight
            && base_face.stretch != face.stretch
        {
            continue;
        }
        if face_has_char(db, face.id, ch) {
            return Some(face.id);
        }
    }
    // usvg keeps the base face and draws its .notdef box; nothing visible
    // is lost by skipping an outline-less missing glyph.
    Some(base)
}

fn system_outline(db: &fontdb::Database, id: fontdb::ID, ch: char) -> SystemGlyph {
    db.with_face_data(id, |data, index| {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let glyph = face.glyph_index(ch).unwrap_or(ttf_parser::GlyphId(0));
        let mut builder = SkOutline(sk::PathBuilder::new());
        face.outline_glyph(glyph, &mut builder)?;
        Some((
            Arc::new(builder.0.finish()?),
            f32::from(face.units_per_em()),
        ))
    })
    .flatten()
}

struct SkOutline(sk::PathBuilder);

impl ttf_parser::OutlineBuilder for SkOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// Decode by content, as resvg would for a correctly labelled data URL.
fn decode_raster(data: &[u8]) -> Option<sk::Pixmap> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        sk::Pixmap::decode_png(data).ok()
    } else if data.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(data)
    } else if data.starts_with(b"GIF8") {
        decode_gif(data)
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        decode_webp(data)
    } else {
        None
    }
}

fn decode_jpeg(data: &[u8]) -> Option<sk::Pixmap> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data), options);
    decoder.decode_headers().ok()?;
    if decoder.output_colorspace()? != ColorSpace::RGBA {
        return None;
    }
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let size = sk::IntSize::from_wh(u32::from(info.width), u32::from(info.height))?;
    sk::Pixmap::from_vec(pixels, size)
}

fn decode_gif(data: &[u8]) -> Option<sk::Pixmap> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(data).ok()?;
    let frame = decoder.read_next_frame().ok()??;
    let mut pixmap = sk::Pixmap::new(u32::from(frame.width), u32::from(frame.height))?;
    premultiply_rgba(&frame.buffer, pixmap.data_mut());
    Some(pixmap)
}

fn decode_webp(data: &[u8]) -> Option<sk::Pixmap> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).ok()?;
    let mut frame = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut frame).ok()?;
    let (width, height) = decoder.dimensions();
    let mut pixmap = sk::Pixmap::new(width, height)?;
    if decoder.has_alpha() {
        premultiply_rgba(&frame, pixmap.data_mut());
    } else {
        for (dst, src) in pixmap
            .data_mut()
            .chunks_exact_mut(4)
            .zip(frame.chunks_exact(3))
        {
            dst[..3].copy_from_slice(src);
            dst[3] = 255;
        }
    }
    Some(pixmap)
}

fn premultiply_rgba(src: &[u8], dst: &mut [u8]) {
    for (dst, src) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let alpha = f64::from(src[3]) / 255.0;
        dst[0] = (f64::from(src[0]) * alpha + 0.5) as u8;
        dst[1] = (f64::from(src[1]) * alpha + 0.5) as u8;
        dst[2] = (f64::from(src[2]) * alpha + 0.5) as u8;
        dst[3] = src[3];
    }
}
