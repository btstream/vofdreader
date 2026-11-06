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
mod tests;
