//! Cached document geometry: scrolling changes the visible range, not layout.
use std::ops::Range;

use ofd_core::SizeMm;

pub const PAD: f32 = 24.0;

#[derive(Clone, Copy, PartialEq)]
pub struct LayoutKey {
    pub width: f32,
    pub zoom: f32,
    pub revision: u64,
    /// Quarter turns clockwise; odd values swap each page's footprint.
    pub rotation: u8,
    /// Vertical space between consecutive page cards.
    pub gap: f32,
}

#[derive(Default)]
pub struct LayoutCache {
    key: Option<LayoutKey>,
    layout: Layout,
}

#[derive(Default)]
pub struct Layout {
    pub sizes: Vec<(f32, f32)>,
    tops: Vec<f32>,
    boundaries: Vec<f32>,
    pub height: f32,
    pub gap: f32,
}

impl Layout {
    pub fn gap(&self) -> f32 {
        self.gap
    }
}

impl LayoutCache {
    pub fn get(&mut self, key: LayoutKey, sizes: &[SizeMm]) -> &Layout {
        if self.key != Some(key) || self.layout.sizes.len() != sizes.len() {
            let mut layout = Layout {
                gap: key.gap,
                ..Layout::default()
            };
            let mut top = PAD;
            for size in sizes {
                let width = key.width * key.zoom;
                let (source_w, source_h) = if key.rotation % 2 == 1 {
                    (size.height, size.width)
                } else {
                    (size.width, size.height)
                };
                let height = width * (source_h / source_w.max(0.01)) as f32;
                layout.tops.push(top);
                layout.sizes.push((width, height));
                layout.boundaries.push(top + height + key.gap * 0.5);
                top += height + key.gap;
            }
            layout.height = if sizes.is_empty() {
                0.0
            } else {
                top - key.gap + PAD
            };
            self.layout = layout;
            self.key = Some(key);
        }
        &self.layout
    }
}

impl Layout {
    pub fn top(&self, page: usize) -> f32 {
        self.tops.get(page).copied().unwrap_or(PAD)
    }

    pub fn page_at(&self, offset: f32) -> usize {
        self.boundaries
            .partition_point(|boundary| *boundary <= offset.max(0.0))
            .min(self.sizes.len().saturating_sub(1))
    }

    pub fn visible(&self, offset: f32, height: f32) -> Range<usize> {
        if self.sizes.is_empty() {
            return 0..0;
        }
        self.page_at(offset).saturating_sub(1)
            ..(self.page_at(offset + height) + 2).min(self.sizes.len())
    }

    pub fn bottom(&self, page: usize) -> f32 {
        self.top(page) + self.sizes.get(page).map_or(0.0, |size| size.1)
    }

    pub fn spacers(&self, range: Range<usize>) -> (f32, f32) {
        if range.is_empty() {
            return (0.0, self.height);
        }
        let used = if range.end < self.sizes.len() {
            self.top(range.end)
        } else {
            self.bottom(range.end - 1)
        };
        (self.top(range.start), (self.height - used).max(0.0))
    }
}

/// Cap each resident bitmap so the *whole* visible/prefetch working set fits.
/// Otherwise 4 A4 images at 384 DPI exceed 128 MiB and endlessly evict/re-render.
pub fn budgeted_dpi(ideal: u32, page: SizeMm, count: usize, budget: usize, max_dpi: u32) -> u32 {
    let bytes_at_96 = (page.width.max(1.0) * 96.0 / 25.4 + 1.0)
        * (page.height.max(1.0) * 96.0 / 25.4 + 1.0)
        * 4.0;
    let per_page = budget as f64 * 0.75 / count.max(1) as f64;
    let cap = (96.0 * (per_page / bytes_at_96).sqrt() / 24.0).floor() * 24.0;
    ideal.min(cap.clamp(96.0, max_dpi.max(96) as f64) as u32)
}

/// Whole-page bitmap density: `wanted` (preset × zoom), never above the
/// preset (zoomed detail comes from tiles), within the 8192 px GPU texture
/// side and the page cache shared by `visible` pages.
pub fn page_dpi(wanted: u32, preset: u32, page: SizeMm, visible: usize, budget: usize) -> u32 {
    let longest = page.width.max(page.height).max(1.0);
    let texture = (((8192.0 * 25.4 / longest) / 24.0).floor() * 24.0).max(96.0) as u32;
    budgeted_dpi(
        wanted.min(preset).min(texture),
        page,
        visible,
        budget,
        crate::tiles::MAX_DPI,
    )
}

/// How many leading prefetch candidates (in priority order, by resident
/// bytes) fit into the cache room left after the on-screen pages. Stops at
/// the first one that does not fit so lower priority pages never displace
/// higher ones; together with on-screen pages this never exceeds the cache,
/// so finished prefetches cannot evict each other in a loop.
pub fn prefetch_fit(bytes: impl IntoIterator<Item = usize>, room: usize, slots: usize) -> usize {
    let mut left = room;
    bytes
        .into_iter()
        .take(slots)
        .take_while(|bytes| {
            let fits = *bytes <= left;
            left = left.saturating_sub(*bytes);
            fits
        })
        .count()
}

/// A raster job may start while scrolling as long as a worker slot is free;
/// pages entering the view must already be at the chosen quality.
pub fn render_ready(in_flight: usize, slots: usize) -> bool {
    in_flight < slots.max(1)
}

pub fn prefetch_range(visible: Range<usize>, total: usize, radius: usize) -> Range<usize> {
    if visible.is_empty() || total == 0 {
        return 0..0;
    }
    visible.start.saturating_sub(radius).min(total)..visible.end.saturating_add(radius).min(total)
}

#[cfg(test)]
mod tests;
