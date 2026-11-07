//! Painting prepared operations into a tiny-skia pixmap.

use resvg::tiny_skia as sk;

use super::ops::{Bounds, ClipOutline, Op};

pub(crate) fn draw(
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
pub(crate) fn draw_layer(
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
