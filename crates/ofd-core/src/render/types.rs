use super::svg::matrix_num;
use crate::model::{Color, DrawParam, SizeMm};

/// 3×2 affine matrix in SVG order: `a b c d e f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f64, y: f64) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut values = value.split_whitespace();
        let a = values.next()?.parse().ok()?;
        let b = values.next()?.parse().ok()?;
        let c = values.next()?.parse().ok()?;
        let d = values.next()?.parse().ok()?;
        let e = values.next()?.parse().ok()?;
        let f = values.next()?.parse().ok()?;
        Some(Self { a, b, c, d, e, f })
    }

    /// Compose `self * other` (apply `other` first, then `self`).
    pub fn compose(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    pub fn invert(&self) -> Option<Self> {
        let determinant = self.a * self.d - self.b * self.c;
        if determinant.abs() < 1e-12 {
            return None;
        }
        Some(Self {
            a: self.d / determinant,
            b: -self.b / determinant,
            c: -self.c / determinant,
            d: self.a / determinant,
            e: (self.c * self.f - self.d * self.e) / determinant,
            f: (self.b * self.e - self.a * self.f) / determinant,
        })
    }

    /// Applies this affine transform to a point.
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub(crate) fn svg(&self) -> String {
        format!(
            "matrix({} {} {} {} {} {})",
            matrix_num(self.a),
            matrix_num(self.b),
            matrix_num(self.c),
            matrix_num(self.d),
            matrix_num(self.e),
            matrix_num(self.f)
        )
    }
}

#[derive(Debug, Clone)]
pub struct DisplayList {
    pub size: SizeMm,
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Path {
        transform: Matrix,
        cmds: Vec<PathCmd>,
        style: PathStyle,
    },
    Text {
        transform: Matrix,
        chars: Vec<PositionedChar>,
        glyphs: Vec<u16>,
        style: TextStyle,
    },
    Image {
        transform: Matrix,
        width: f64,
        height: f64,
        mime: &'static str,
        data: Vec<u8>,
        blend: Option<String>,
        /// Optional clip rectangle in image-local millimetres.
        clip: Option<(f64, f64, f64, f64)>,
    },
    Group {
        transform: Matrix,
        items: Vec<Item>,
        opacity: f64,
    },
    /// Object-level `Clips`: `items` are visible only where every region
    /// overlaps (regions intersect; the shapes inside one region unite).
    Clip {
        regions: Vec<Vec<ClipShape>>,
        items: Vec<Item>,
    },
}

/// One clip outline, in the same coordinate space as the clipped items'
/// parent (the object's Boundary origin and the Area CTM are folded in).
#[derive(Debug, Clone)]
pub struct ClipShape {
    pub transform: Matrix,
    pub cmds: Vec<PathCmd>,
    pub even_odd: bool,
}

/// `AxialShd` / `RadialShd` gradient in the path's own coordinates.
#[derive(Debug, Clone)]
pub struct Shading {
    pub kind: ShadingKind,
    /// Raw `Segment` stops, positions in 0..=1, ascending.
    pub stops: Vec<(f64, Color)>,
    pub extend_start: bool,
    pub extend_end: bool,
    pub spread: ShadingSpread,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadingKind {
    Axial {
        start: (f64, f64),
        end: (f64, f64),
    },
    /// Focal point `start` (radius `start_radius`) to circle `end`/`radius`.
    Radial {
        start: (f64, f64),
        start_radius: f64,
        end: (f64, f64),
        radius: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadingSpread {
    Pad,
    Repeat,
    Reflect,
}

impl Shading {
    /// Stops as both backends draw them. A non-zero start radius maps onto
    /// the start_radius/radius..1 part of the gradient (both backends only
    /// support a zero focal radius), and an end that must not extend gets a
    /// transparent guard stop so padding leaves it unpainted.
    pub fn resolved_stops(&self) -> Vec<(f64, Color)> {
        const GUARD: f64 = 1e-4;
        let offset = match self.kind {
            ShadingKind::Radial {
                start_radius,
                radius,
                ..
            } if radius > 0.0 && start_radius > 0.0 => (start_radius / radius).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let mut stops: Vec<(f64, Color)> = self
            .stops
            .iter()
            .map(|(position, color)| (offset + position.clamp(0.0, 1.0) * (1.0 - offset), *color))
            .collect();
        if self.spread != ShadingSpread::Pad || stops.is_empty() {
            return stops;
        }
        let clear = |color: Color| Color { a: 0, ..color };
        if !self.extend_start {
            let first = stops[0].1;
            for stop in &mut stops {
                stop.0 = stop.0.max(offset + GUARD);
            }
            stops.insert(0, (offset, clear(first)));
            if offset > 0.0 {
                stops.insert(0, (0.0, clear(first)));
            }
        } else if offset > 0.0 {
            stops.insert(0, (0.0, stops[0].1));
        }
        if !self.extend_end {
            let last = stops[stops.len() - 1].1;
            for stop in &mut stops {
                stop.0 = stop.0.min(1.0 - GUARD);
            }
            stops.push((1.0, clear(last)));
        }
        stops
    }
}

#[derive(Debug, Clone)]
pub enum PathCmd {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Arc {
        rx: f64,
        ry: f64,
        rotation: f64,
        large: bool,
        sweep: bool,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Debug, Clone)]
pub struct PathStyle {
    pub fill: bool,
    pub stroke: bool,
    pub fill_color: Option<Color>,
    pub stroke_color: Option<Color>,
    pub line_width: f64,
    pub cap: String,
    pub join: String,
    pub dash: Vec<f64>,
    pub opacity: f64,
    pub even_odd: bool,
    /// Gradient that replaces `fill_color` / `stroke_color` when present.
    pub fill_shading: Option<Shading>,
    pub stroke_shading: Option<Shading>,
}

impl Default for PathStyle {
    fn default() -> Self {
        Self {
            fill: true,
            stroke: true,
            fill_color: None,
            stroke_color: Some(Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            line_width: 0.25,
            cap: "Butt".to_string(),
            join: "Miter".to_string(),
            dash: Vec::new(),
            opacity: 1.0,
            even_odd: false,
            fill_shading: None,
            stroke_shading: None,
        }
    }
}

impl PathStyle {
    pub(crate) fn apply(&mut self, param: &DrawParam) {
        if let Some(width) = param.line_width {
            self.line_width = width;
        }
        if let Some(color) = param.stroke_color {
            self.stroke_color = Some(color);
        }
        if let Some(color) = param.fill_color {
            self.fill_color = Some(color);
        }
        if let Some(cap) = &param.cap {
            self.cap = cap.clone();
        }
        if let Some(join) = &param.join {
            self.join = join.clone();
        }
        if !param.dash_pattern.is_empty() {
            self.dash = param.dash_pattern.clone();
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextStyle {
    /// Resource key (`scope:id`) used to resolve embedded faces.
    pub font_key: String,
    pub font_family: String,
    pub font_size: f64,
    pub color: Color,
    pub weight: u16,
    pub italic: bool,
    pub opacity: f64,
    pub h_scale: f64,
    pub char_direction: u16,
    pub read_direction: u16,
    pub paint: PathStyle,
    pub clusters: Vec<GlyphCluster>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlyphCluster {
    pub start: usize,
    pub end: usize,
    pub glyphs: Vec<u16>,
    pub positions: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct PositionedChar {
    pub ch: char,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Rasterized page ready for a GPU texture. GPUI's `RenderImage` expects
/// straight-alpha BGRA, so no PNG encode/decode round trip is needed.
#[derive(Debug, Clone)]
pub struct RasterPage {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A crop in page-device pixels at the requested DPI. A caller can include a
/// small gutter and clip it when compositing adjacent tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RasterRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Bounding box of a text match in page millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMatchRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One selectable character with its page-space bounds.
#[derive(Debug, Clone)]
pub struct PageTextChar {
    pub ch: char,
    pub rect: TextMatchRect,
    /// Visual line index; selection inserts a newline between lines.
    pub line: u32,
    /// Source text object; same-line runs are joined with a space.
    pub run: u32,
}

/// Flattened selectable text of a page, in reading order of the display list.
/// Annotation groups are excluded so stamps and watermarks do not pollute a
/// selection.
#[derive(Debug, Clone, Default)]
pub struct PageTextLayout {
    pub chars: Vec<PageTextChar>,
}
