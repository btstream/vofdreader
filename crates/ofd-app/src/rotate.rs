//! Clockwise quarter-turn view rotation. Rendering, overlays, selection and
//! high-resolution tiles all use the same four transforms.
use ofd_core::RasterRegion;

use crate::search_highlight::Highlight;

/// Buffer rotation for cached bitmaps. Input/output are BGRA in display order.
pub fn buffer(width: u32, height: u32, data: &[u8], turns: u8) -> Option<(u32, u32, Vec<u8>)> {
    let turns = turns % 4;
    if width == 0 || height == 0 || data.len() != width as usize * height as usize * 4 {
        return None;
    }
    if turns == 0 {
        return Some((width, height, data.to_vec()));
    }
    let (out_width, out_height) = if turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let stride = width as usize * 4;
    let out_stride = out_width as usize * 4;
    let mut out = vec![0; out_stride * out_height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let (dx, dy) = match turns {
                1 => (height as usize - 1 - y, x),
                2 => (width as usize - 1 - x, height as usize - 1 - y),
                _ => (y, width as usize - 1 - x),
            };
            let src = y * stride + x * 4;
            let dst = dy * out_stride + dx * 4;
            out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
        }
    }
    Some((out_width, out_height, out))
}

/// Same as [`buffer`] but avoids a copy when the view is not rotated.
pub fn owned(width: u32, height: u32, data: Vec<u8>, turns: u8) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() != width as usize * height as usize * 4 {
        return None;
    }
    if turns.is_multiple_of(4) {
        Some((width, height, data))
    } else {
        buffer(width, height, &data, turns)
    }
}

/// Page dimensions after rotation.
pub fn size(width: f32, height: f32, turns: u8) -> (f32, f32) {
    if turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    }
}

/// Convert a page-relative overlay rectangle into rotated view space.
pub fn highlights(rects: Vec<Highlight>, turns: u8) -> Vec<Highlight> {
    match turns % 4 {
        1 => rects
            .into_iter()
            .map(|r| Highlight {
                x: 1.0 - (r.y + r.height),
                y: r.x,
                width: r.height,
                height: r.width,
            })
            .collect(),
        2 => rects
            .into_iter()
            .map(|r| Highlight {
                x: 1.0 - (r.x + r.width),
                y: 1.0 - (r.y + r.height),
                width: r.width,
                height: r.height,
            })
            .collect(),
        3 => rects
            .into_iter()
            .map(|r| Highlight {
                x: r.y,
                y: 1.0 - (r.x + r.width),
                width: r.height,
                height: r.width,
            })
            .collect(),
        _ => rects,
    }
}

/// Convert a normalized point in the rotated view back to unrotated page
/// fractions (`u` right, `v` down).
pub fn inverse_point(u: f64, v: f64, turns: u8) -> (f64, f64) {
    match turns % 4 {
        1 => (v, 1.0 - u),
        2 => (1.0 - u, 1.0 - v),
        3 => (1.0 - v, u),
        _ => (u, v),
    }
}

/// Map a region of the rotated device-pixel page back to the unrotated raster
/// region. Callers rotate the returned pixels into view orientation.
pub fn source_region(
    rotated_width: u32,
    rotated_height: u32,
    turns: u8,
    r: RasterRegion,
) -> RasterRegion {
    match turns % 4 {
        1 => RasterRegion {
            x: r.y,
            y: rotated_width.saturating_sub(r.x + r.width),
            width: r.height,
            height: r.width,
        },
        2 => RasterRegion {
            x: rotated_width.saturating_sub(r.x + r.width),
            y: rotated_height.saturating_sub(r.y + r.height),
            width: r.width,
            height: r.height,
        },
        3 => RasterRegion {
            x: rotated_height.saturating_sub(r.y + r.height),
            y: r.x,
            width: r.height,
            height: r.width,
        },
        _ => r,
    }
}

#[cfg(test)]
mod tests;
