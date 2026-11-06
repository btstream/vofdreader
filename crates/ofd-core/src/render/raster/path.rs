use super::*;

pub(crate) fn hash_cmd(cmd: &PathCmd, hasher: &mut impl Hasher) {
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

pub(crate) fn build_path(cmds: &[PathCmd]) -> Option<sk::Path> {
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
pub(crate) fn arc_to(
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

pub(crate) fn push_path(path: Arc<sk::Path>, matrix: Matrix, style: &PathStyle, out: &mut Vec<Op>) {
    let ts = to_ts(matrix);
    let path_bounds = path.bounds();
    // usvg drops fills of zero-area paths (e.g. a filled straight line).
    if style.fill
        && path_bounds.width() > 0.0
        && path_bounds.height() > 0.0
        && let Some(shader) =
            paint_shader(style.fill_shading.as_ref(), style.fill_color, style.opacity)
        && let Some(bounds) = Bounds::of_rect(path_bounds, ts)
    {
        out.push(Op::Fill {
            path: path.clone(),
            ts,
            shader,
            rule: if style.even_odd {
                sk::FillRule::EvenOdd
            } else {
                sk::FillRule::Winding
            },
            bounds,
        });
    }
    if style.stroke
        && let Some(shader) = paint_shader(
            style.stroke_shading.as_ref(),
            style.stroke_color,
            style.opacity,
        )
        && let Some(stroke) = stroke_style(style)
    {
        push_stroke(path, ts, stroke, shader, out);
    }
}

pub(crate) fn push_stroke(
    path: Arc<sk::Path>,
    ts: sk::Transform,
    stroke: sk::Stroke,
    shader: sk::Shader<'static>,
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
        shader,
        stroke,
        bounds,
    });
}

pub(crate) fn stroke_style(style: &PathStyle) -> Option<sk::Stroke> {
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

/// A gradient when the shading is drawable, else the plain colour.
pub(crate) fn paint_shader(
    shading: Option<&Shading>,
    fallback: Option<Color>,
    opacity: f64,
) -> Option<sk::Shader<'static>> {
    if let Some(shader) = shading.and_then(|shading| gradient_shader(shading, opacity)) {
        return Some(shader);
    }
    let fill = fallback?;
    Some(sk::Shader::SolidColor(color(
        fill.r,
        fill.g,
        fill.b,
        fill.opacity() * opacity,
    )))
}

pub(crate) fn gradient_shader(shading: &Shading, opacity: f64) -> Option<sk::Shader<'static>> {
    let stops: Vec<sk::GradientStop> = shading
        .resolved_stops()
        .into_iter()
        .map(|(position, c)| {
            sk::GradientStop::new(position as f32, color(c.r, c.g, c.b, c.opacity() * opacity))
        })
        .collect();
    let mode = match shading.spread {
        ShadingSpread::Pad => sk::SpreadMode::Pad,
        ShadingSpread::Repeat => sk::SpreadMode::Repeat,
        ShadingSpread::Reflect => sk::SpreadMode::Reflect,
    };
    let point = |(x, y): (f64, f64)| sk::Point::from_xy(x as f32, y as f32);
    match shading.kind {
        ShadingKind::Axial { start, end } => sk::LinearGradient::new(
            point(start),
            point(end),
            stops,
            mode,
            sk::Transform::identity(),
        ),
        ShadingKind::Radial {
            start, end, radius, ..
        } => sk::RadialGradient::new(
            point(start),
            point(end),
            radius as f32,
            stops,
            mode,
            sk::Transform::identity(),
        ),
    }
}

pub(crate) fn to_ts(m: Matrix) -> sk::Transform {
    sk::Transform::from_row(
        m.a as f32, m.b as f32, m.c as f32, m.d as f32, m.e as f32, m.f as f32,
    )
}

pub(crate) fn round4(value: f64) -> f64 {
    if value.is_finite() {
        (value * 10_000.0).round() / 10_000.0
    } else {
        0.0
    }
}

pub(crate) fn opacity_u8(opacity: f64) -> u8 {
    // usvg rounds SVG opacity (written with four decimals) the same way.
    (round4(opacity).clamp(0.0, 1.0) as f32 * 255.0 + 0.5) as u8
}

pub(crate) fn color(r: u8, g: u8, b: u8, opacity: f64) -> sk::Color {
    sk::Color::from_rgba8(r, g, b, opacity_u8(opacity))
}
