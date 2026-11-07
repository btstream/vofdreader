//! Flat draw operations produced from a `DisplayList`.

use std::sync::Arc;

use resvg::tiny_skia as sk;
use resvg::usvg;

use super::super::ImageFilter;
use crate::model::SizeMm;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub(crate) x0: f32,
    pub(crate) y0: f32,
    pub(crate) x1: f32,
    pub(crate) y1: f32,
}

impl Bounds {
    pub(crate) fn of_rect(rect: sk::Rect, ts: sk::Transform) -> Option<Self> {
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

    pub(crate) fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub(crate) fn intersection(&self, other: &Self) -> Option<Self> {
        let bounds = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (bounds.x0 <= bounds.x1 && bounds.y0 <= bounds.y1).then_some(bounds)
    }

    /// `other` lies inside `self`, allowing `slack` on every side.
    pub(crate) fn contains(&self, other: &Self, slack: f32) -> bool {
        self.x0 - slack <= other.x0
            && self.y0 - slack <= other.y0
            && self.x1 + slack >= other.x1
            && self.y1 + slack >= other.y1
    }

    pub(crate) fn intersects(&self, other: &Self) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1 && self.y0 <= other.y1 && other.y0 <= self.y1
    }

    pub(crate) fn rect(self) -> Option<sk::Rect> {
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
    pub(crate) path: sk::Path,
    pub(crate) ts: sk::Transform,
    pub(crate) rule: sk::FillRule,
}

impl Op {
    pub(crate) fn bounds(&self) -> Bounds {
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
    pub(crate) ops: Vec<Op>,
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
        super::paint::draw(&self.ops, base, &view, pixmap, filter);
    }
}
