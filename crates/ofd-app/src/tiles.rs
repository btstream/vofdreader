//! Device-pixel tiles. Gutters are sampled but clipped out when compositing.
use ofd_core::RasterRegion;

pub const EDGE: u32 = 512;
pub const MAX_TILE_BYTES: usize = (EDGE as usize + 4) * (EDGE as usize + 4) * 4;

/// Limit speculative tiles to one quarter of cache, leaving room for visible
/// content. Use padded size as a conservative upper bound per tile.
pub fn prefetch_limit(budget: usize, visible_count: usize) -> usize {
    (budget / 4).min(budget.saturating_sub(visible_count.saturating_mul(MAX_TILE_BYTES)))
        / MAX_TILE_BYTES
}

/// Global priority across page plans: a visible tile on a later page always
/// precedes speculative tiles on the current page.
pub fn requests(
    mut visible: Vec<Key>,
    mut nearby: Vec<(usize, Key)>,
    current_page: usize,
    budget: usize,
) -> Vec<Key> {
    use std::collections::HashSet;
    visible.sort_by_key(|key| (key.page.abs_diff(current_page), key.page, key.y, key.x));
    visible.dedup();
    let limit = prefetch_limit(budget, visible.len());
    let mut seen: HashSet<_> = visible.iter().copied().collect();
    nearby.sort_by_key(|(rank, key)| {
        (
            key.page.abs_diff(current_page),
            *rank,
            key.page,
            key.y,
            key.x,
        )
    });
    visible.extend(
        nearby
            .into_iter()
            .map(|(_, key)| key)
            .filter(|key| seen.insert(*key))
            .take(limit),
    );
    visible
}

pub fn neighbors(visible: &[Key], size: (u32, u32), ring: u32) -> Vec<Key> {
    use std::collections::HashSet;
    let Some(first) = visible.first() else {
        return vec![];
    };
    let columns = size.0.div_ceil(EDGE);
    let rows = size.1.div_ceil(EDGE);
    let seen: HashSet<_> = visible.iter().copied().collect();
    let mut candidates = HashSet::new();
    for key in visible {
        for y in key.y.saturating_sub(ring)..key.y.saturating_add(ring).saturating_add(1).min(rows)
        {
            for x in key.x.saturating_sub(ring)
                ..key.x.saturating_add(ring).saturating_add(1).min(columns)
            {
                let candidate = Key { x, y, ..*first };
                if !seen.contains(&candidate) {
                    candidates.insert(candidate);
                }
            }
        }
    }
    let mut result: Vec<_> = candidates.into_iter().collect();
    result.sort_by_key(|key| {
        (
            visible
                .iter()
                .map(|v| v.x.abs_diff(key.x).max(v.y.abs_diff(key.y)))
                .min()
                .unwrap_or(0),
            key.y,
            key.x,
        )
    });
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub page: usize,
    pub dpi: u32,
    pub x: u32,
    pub y: u32,
}

impl Key {
    pub fn regions(self, width: u32, height: u32) -> (RasterRegion, RasterRegion) {
        let content = RasterRegion {
            x: self.x * EDGE,
            y: self.y * EDGE,
            width: EDGE.min(width - self.x * EDGE),
            height: EDGE.min(height - self.y * EDGE),
        };
        let x = content.x.saturating_sub(2);
        let y = content.y.saturating_sub(2);
        let sample = RasterRegion {
            x,
            y,
            width: (content.x + content.width + 2).min(width) - x,
            height: (content.y + content.height + 2).min(height) - y,
        };
        (content, sample)
    }
}

/// Upper bound for a manual thread count: the machine's cores, at most 16.
pub const MAX_THREADS: usize = 16;

pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}

pub fn max_threads() -> usize {
    cores().clamp(1, MAX_THREADS)
}

/// Parallel tile jobs. `setting == 0` is automatic: half the cores (the UI
/// thread, GPU submission and other background work keep the rest), at least
/// 1, at most 4. A manual value is bounded by this machine's cores, so a
/// settings file copied from a larger machine cannot oversubscribe.
pub fn concurrency(setting: usize) -> usize {
    resolve(setting, cores())
}

fn resolve(setting: usize, cores: usize) -> usize {
    if setting == 0 {
        (cores / 2).clamp(1, 4)
    } else {
        setting.clamp(1, cores.clamp(1, MAX_THREADS))
    }
}

/// Safety ceiling for any raster density (zoomed tiles included).
pub const MAX_DPI: u32 = 1536;

/// Hard density: the quality preset's page DPI scaled by the reader zoom,
/// in 24 DPI buckets. Screen resolution and fit-to-width are deliberately
/// not involved, so what is drawn matches the chosen quality everywhere.
pub fn hard_dpi(preset: u32, zoom: f32) -> u32 {
    let dpi = preset.max(96) as f32 * zoom.max(0.01);
    ((dpi / 24.0).ceil() * 24.0).clamp(96.0, MAX_DPI as f32) as u32
}

/// Visible rectangle in page device pixels, including horizontal clipping.
pub fn plan(page: usize, dpi: u32, size: (u32, u32), rect: [f32; 4]) -> Vec<Key> {
    let (width, height) = size;
    let [left, top, right, bottom] = rect;
    if !rect.iter().all(|v| v.is_finite()) || right <= left || bottom <= top {
        return vec![];
    }
    let x0 = left.max(0.0).floor() as u32;
    let y0 = top.max(0.0).floor() as u32;
    let x1 = (right.ceil().max(0.0) as u32).min(width);
    let y1 = (bottom.ceil().max(0.0) as u32).min(height);
    if x0 >= x1 || y0 >= y1 {
        return vec![];
    }
    let mut keys = Vec::new();
    for y in y0 / EDGE..=(y1 - 1) / EDGE {
        for x in x0 / EDGE..=(x1 - 1) / EDGE {
            keys.push(Key { page, dpi, x, y });
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_tiles_on_all_pages_outrank_nearby_tiles_and_cached_return_trip_reuses_keys() {
        let a = Key {
            page: 0,
            dpi: 384,
            x: 1,
            y: 1,
        };
        let b = Key { page: 1, ..a };
        let neighbor = Key { y: 2, ..a };
        let planned = requests(
            vec![b, a],
            vec![(0, neighbor), (1, a), (2, neighbor)],
            0,
            64 * 1024 * 1024,
        );
        assert_eq!(planned, vec![a, b, neighbor]);
        let cached = std::collections::HashSet::from([a, b, neighbor]);
        // A neighbouring block that becomes visible keeps its exact cache key.
        let returned = requests(vec![neighbor], vec![(0, a)], 0, 64 * 1024 * 1024);
        assert!(returned.iter().all(|key| cached.contains(key)));
    }
    #[test]
    fn nearby_ring_is_bounded_deduplicated_and_excludes_visible_tiles() {
        let center = Key {
            page: 2,
            dpi: 384,
            x: 1,
            y: 1,
        };
        let nearby = neighbors(&[center], (2048, 2048), 1);
        assert_eq!(nearby.len(), 8);
        assert!(!nearby.contains(&center));
        assert!(nearby.iter().all(|key| key.page == 2 && key.dpi == 384));
        let edge = Key {
            x: 0,
            y: 0,
            ..center
        };
        assert_eq!(neighbors(&[edge], (2048, 2048), 1).len(), 3);
        let visible = [center, Key { x: 2, ..center }];
        let nearby = neighbors(&visible, (2048, 2048), 1);
        assert_eq!(nearby.len(), 10);
        assert!(nearby.iter().all(|key| !visible.contains(key)));
        assert_eq!(
            nearby
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            nearby.len()
        );
        assert!(neighbors(&visible, (2048, 2048), 0).is_empty());
        assert!(neighbors(&[], (2048, 2048), 1).is_empty());
    }

    #[test]
    fn speculative_tiles_leave_room_for_visible_working_set() {
        let budget = 64 * 1024 * 1024;
        let limit = prefetch_limit(budget, 32);
        assert!(limit * MAX_TILE_BYTES <= budget / 4);
        assert!((limit + 32) * MAX_TILE_BYTES <= budget);
        assert_eq!(prefetch_limit(budget, usize::MAX), 0);
        assert_eq!(prefetch_limit(0, 0), 0);
    }
    #[test]
    fn boundaries_gutters_and_horizontal_clip() {
        let keys = plan(3, 384, (1100, 800), [512.0, 0.0, 1100.0, 512.0]);
        assert_eq!(keys.len(), 2);
        assert_eq!((keys[0].x, keys[1].x), (1, 2));
        let (a, sample) = keys[0].regions(1100, 800);
        let (b, _) = keys[1].regions(1100, 800);
        assert_eq!(a.x + a.width, b.x);
        assert_eq!(b.width, 76);
        assert_eq!((sample.x, sample.width), (510, 516));
        assert!(plan(0, 96, (100, 100), [101.0, 0.0, 200.0, 100.0]).is_empty());
        assert!(plan(0, 96, (100, 100), [0.0, f32::NAN, 2.0, 2.0]).is_empty());
    }
    #[test]
    fn tile_concurrency_auto_uses_half_the_cores_within_bounds() {
        assert_eq!(resolve(0, 1), 1);
        assert_eq!(resolve(0, 2), 1);
        assert_eq!(resolve(0, 4), 2);
        assert_eq!(resolve(0, 8), 4);
        assert_eq!(resolve(0, 64), 4);
        assert!((1..=4).contains(&concurrency(0)));
    }
    #[test]
    fn manual_thread_count_is_bounded_by_cores() {
        assert_eq!(resolve(1, 8), 1);
        assert_eq!(resolve(6, 8), 6);
        assert_eq!(resolve(12, 8), 8);
        assert_eq!(resolve(3, 1), 1);
        assert_eq!(resolve(32, 64), MAX_THREADS);
        assert!(concurrency(usize::MAX) <= max_threads());
    }
    #[test]
    fn complete_grid_covers_each_pixel_once() {
        let keys = plan(0, 192, (1100, 800), [0.0, 0.0, 1100.0, 800.0]);
        let area: u32 = keys
            .iter()
            .map(|key| {
                let (r, _) = key.regions(1100, 800);
                r.width * r.height
            })
            .sum();
        assert_eq!(area, 1100 * 800);
        assert_eq!(hard_dpi(192, 1.0), 192);
        assert_eq!(hard_dpi(192, 1.5), 288);
        assert_eq!(hard_dpi(192, 0.5), 96);
        assert_eq!(hard_dpi(384, 1.1), 432);
        assert_eq!(hard_dpi(384, 8.0), MAX_DPI);
    }
}
