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

use super::parse::blend_mode;
use super::svg::{cap, glyph_orientation, join, text_glyph_items};
use super::{
    ClipShape, ImageFilter, Item, Matrix, PageRenderer, PathCmd, PathStyle, Shading, ShadingKind,
    ShadingSpread, TextStyle,
};
use crate::model::{Color, SizeMm};

mod fonts;
mod images;
mod path;

pub(crate) use fonts::*;
pub(crate) use images::*;
pub(crate) use path::*;

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

    fn intersection(&self, other: &Self) -> Option<Self> {
        let bounds = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (bounds.x0 <= bounds.x1 && bounds.y0 <= bounds.y1).then_some(bounds)
    }

    /// `other` lies inside `self`, allowing `slack` on every side.
    fn contains(&self, other: &Self, slack: f32) -> bool {
        self.x0 - slack <= other.x0
            && self.y0 - slack <= other.y0
            && self.x1 + slack >= other.x1
            && self.y1 + slack >= other.y1
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

pub(crate) enum Op {
    Fill {
        path: Arc<sk::Path>,
        ts: sk::Transform,
        /// Solid colour or gradient in path coordinates.
        shader: sk::Shader<'static>,
        rule: sk::FillRule,
        bounds: Bounds,
    },
    Stroke {
        path: Arc<sk::Path>,
        ts: sk::Transform,
        shader: sk::Shader<'static>,
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
    /// `ops` masked by the intersection of `regions` (each a union of
    /// page-space outlines).
    Clip {
        ops: Vec<Op>,
        regions: Vec<Vec<ClipOutline>>,
        bounds: Bounds,
    },
}

pub(crate) struct ClipOutline {
    path: sk::Path,
    ts: sk::Transform,
    rule: sk::FillRule,
}

impl Op {
    fn bounds(&self) -> Bounds {
        match self {
            Op::Fill { bounds, .. }
            | Op::Stroke { bounds, .. }
            | Op::Image { bounds, .. }
            | Op::Vector { bounds, .. }
            | Op::Layer { bounds, .. }
            | Op::Clip { bounds, .. } => *bounds,
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
    pub fn render(&self, base: sk::Transform, pixmap: &mut sk::PixmapMut, filter: ImageFilter) {
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
        let filter = match filter {
            ImageFilter::Nearest => sk::FilterQuality::Nearest,
            ImageFilter::Bilinear => sk::FilterQuality::Bilinear,
            ImageFilter::Bicubic => sk::FilterQuality::Bicubic,
        };
        draw(&self.ops, base, &view, pixmap, filter);
    }
}

fn draw(
    ops: &[Op],
    base: sk::Transform,
    view: &Bounds,
    pixmap: &mut sk::PixmapMut,
    filter: sk::FilterQuality,
) {
    for op in ops {
        if !op.bounds().intersects(view) {
            continue;
        }
        match op {
            Op::Fill {
                path,
                ts,
                shader,
                rule,
                ..
            } => {
                let paint = sk::Paint {
                    shader: shader.clone(),
                    anti_alias: true,
                    ..sk::Paint::default()
                };
                pixmap.fill_path(path, &paint, *rule, base.pre_concat(*ts), None);
            }
            Op::Stroke {
                path,
                ts,
                shader,
                stroke,
                ..
            } => {
                let paint = sk::Paint {
                    shader: shader.clone(),
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
                        filter,
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
                bounds,
            } => {
                let transform = base.pre_concat(*ts);
                if clip.is_none() && *blend == sk::BlendMode::SourceOver {
                    resvg::render(tree, transform, pixmap);
                    continue;
                }
                // Layer only as large as the seal, not the whole pixmap.
                let Some((x0, y0, width, height)) = device_rect(bounds, base, pixmap) else {
                    continue;
                };
                let Some(mut layer) = sk::Pixmap::new(width, height) else {
                    continue;
                };
                let shifted = transform.post_translate(-(x0 as f32), -(y0 as f32));
                resvg::render(tree, shifted, &mut layer.as_mut());
                if let Some(clip) = clip
                    && let Some(mut mask) = sk::Mask::new(width, height)
                {
                    mask.fill_path(
                        &sk::PathBuilder::from_rect(*clip),
                        sk::FillRule::Winding,
                        true,
                        shifted,
                    );
                    layer.apply_mask(&mask);
                }
                pixmap.draw_pixmap(
                    x0,
                    y0,
                    layer.as_ref(),
                    &sk::PixmapPaint {
                        blend_mode: *blend,
                        ..sk::PixmapPaint::default()
                    },
                    sk::Transform::identity(),
                    None,
                );
            }
            Op::Layer {
                ops,
                opacity,
                bounds,
            } => draw_layer(ops, bounds, base, pixmap, *opacity, &[], filter),
            Op::Clip {
                ops,
                regions,
                bounds,
            } => draw_layer(ops, bounds, base, pixmap, 1.0, regions, filter),
        }
    }
}

/// Pixel rectangle `(x, y, width, height)` covering page-space `bounds`
/// plus a 2 px anti-aliasing margin, clamped to the pixmap.
fn device_rect(
    bounds: &Bounds,
    base: sk::Transform,
    pixmap: &sk::PixmapMut,
) -> Option<(i32, i32, u32, u32)> {
    let device = Bounds::of_rect(bounds.rect()?, base)?;
    let x0 = (device.x0.floor() as i64 - 2).max(0);
    let y0 = (device.y0.floor() as i64 - 2).max(0);
    let x1 = (device.x1.ceil() as i64 + 2).min(i64::from(pixmap.width()));
    let y1 = (device.y1.ceil() as i64 + 2).min(i64::from(pixmap.height()));
    (x1 > x0 && y1 > y0).then(|| (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Paints `ops` into an offscreen layer covering `bounds`, masks it by the
/// clip regions and composites it with `opacity`.
fn draw_layer(
    ops: &[Op],
    bounds: &Bounds,
    base: sk::Transform,
    pixmap: &mut sk::PixmapMut,
    opacity: f32,
    regions: &[Vec<ClipOutline>],
    filter: sk::FilterQuality,
) {
    let Some((x0, y0, width, height)) = device_rect(bounds, base, pixmap) else {
        return;
    };
    let Some(mut layer) = sk::Pixmap::new(width, height) else {
        return;
    };
    let shifted = base.post_translate(-(x0 as f32), -(y0 as f32));
    let Some(inverse) = shifted.invert() else {
        return;
    };
    let local = sk::Rect::from_xywh(-2.0, -2.0, width as f32 + 4.0, height as f32 + 4.0)
        .and_then(|r| Bounds::of_rect(r, inverse));
    let Some(local) = local else { return };
    draw(ops, shifted, &local, &mut layer.as_mut(), filter);
    if let Some(mask) = clip_mask(regions, shifted, width, height) {
        layer.apply_mask(&mask);
    }
    pixmap.draw_pixmap(
        x0,
        y0,
        layer.as_ref(),
        &sk::PixmapPaint {
            opacity,
            ..sk::PixmapPaint::default()
        },
        sk::Transform::identity(),
        None,
    );
}

/// Coverage of the intersection of all regions; `None` when unclipped.
fn clip_mask(
    regions: &[Vec<ClipOutline>],
    ts: sk::Transform,
    width: u32,
    height: u32,
) -> Option<sk::Mask> {
    let mut combined: Option<sk::Mask> = None;
    for region in regions {
        let mut mask = sk::Mask::new(width, height)?;
        for outline in region {
            mask.fill_path(&outline.path, outline.rule, true, ts.pre_concat(outline.ts));
        }
        combined = Some(match combined {
            None => mask,
            Some(mut acc) => {
                for (a, b) in acc.data_mut().iter_mut().zip(mask.data()) {
                    *a = ((u16::from(*a) * u16::from(*b) + 127) / 255) as u8;
                }
                acc
            }
        });
    }
    combined
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

const IMAGE_CACHE_BYTES: usize = 256 * 1024 * 1024;

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
