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
mod tests {
    use super::*;

    fn pixels(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| [i as u8, 0, 0, 255])
            .collect()
    }

    fn sample(data: &[u8], width: u32, x: u32, y: u32) -> u8 {
        data[(y * width + x) as usize * 4]
    }

    #[test]
    fn buffer_corners_rotate_clockwise_and_round_trip() {
        let data = pixels(3, 2);
        let (w, h, once) = buffer(3, 2, &data, 1).unwrap();
        assert_eq!((w, h), (2, 3));
        assert_eq!(sample(&once, w, 1, 0), 0);
        assert_eq!(sample(&once, w, 0, 0), 3);
        let (w, h, twice) = buffer(w, h, &once, 1).unwrap();
        let (w, h, thrice) = buffer(w, h, &twice, 1).unwrap();
        let (w, h, four) = buffer(w, h, &thrice, 1).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(four, data);
        assert_eq!(buffer(3, 2, &data, 4).unwrap().2, data);
        assert!(buffer(3, 2, &data[..4], 1).is_none());
        assert!(buffer(0, 2, &[], 1).is_none());
    }

    #[test]
    fn overlay_and_pointer_transforms_are_inverses() {
        let rect = Highlight {
            x: 0.1,
            y: 0.2,
            width: 0.3,
            height: 0.05,
        };
        let center = (0.25, 0.225);
        for turns in 0..4 {
            let rotated = highlights(vec![rect.clone()], turns);
            let (u, v) = match turns {
                1 => (1.0 - center.1, center.0),
                2 => (1.0 - center.0, 1.0 - center.1),
                3 => (center.1, 1.0 - center.0),
                _ => center,
            };
            let r = &rotated[0];
            assert!((r.x..=r.x + r.width).contains(&(u as f32)));
            assert!((r.y..=r.y + r.height).contains(&(v as f32)));
            let (x, y) = inverse_point(u, v, turns);
            assert!((x - center.0).abs() < 1e-6 && (y - center.1).abs() < 1e-6);
        }
        let (w, h) = size(200.0, 100.0, 1);
        assert_eq!((w, h), (100.0, 200.0));
    }

    #[test]
    fn tile_source_region_matches_pixel_rotation() {
        // Unrotated 3x2 device page. A rotated-space 1x2 strip at the top of
        // the 90-degree view corresponds to the right-hand column of the page.
        let r = RasterRegion {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };
        let source = source_region(2, 3, 1, r);
        assert_eq!(
            source,
            RasterRegion {
                x: 0,
                y: 0,
                width: 2,
                height: 1
            }
        );
        let full = pixels(3, 2);
        let (_, _, rotated) = buffer(3, 2, &full, 1).unwrap();
        let mut src = Vec::new();
        for y in source.y..source.y + source.height {
            for x in source.x..source.x + source.width {
                let i = (y * 3 + x) as usize * 4;
                src.extend_from_slice(&full[i..i + 4]);
            }
        }
        let (w, h, tile) = buffer(source.width, source.height, &src, 1).unwrap();
        assert_eq!((w, h), (r.width, r.height));
        for y in 0..h {
            for x in 0..w {
                assert_eq!(
                    sample(&tile, w, x, y),
                    sample(&rotated, 2, x + r.x, y + r.y)
                );
            }
        }
    }
}
