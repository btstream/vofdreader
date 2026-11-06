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
