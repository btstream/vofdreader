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
mod tests {
    use super::*;
    #[test]
    fn nearby_pages_respect_document_edges_and_disabled_prefetch() {
        assert_eq!(prefetch_range(5..6, 20, 2), 3..8);
        assert_eq!(prefetch_range(0..2, 20, 2), 0..4);
        assert_eq!(prefetch_range(18..20, 20, 2), 16..20);
        assert_eq!(prefetch_range(5..8, 20, 0), 5..8);
        assert_eq!(prefetch_range(0..0, 0, 2), 0..0);
        assert_eq!(prefetch_range(0..1, 1, usize::MAX), 0..1);
    }

    #[test]
    fn scroll_lookup_and_spacers_preserve_mixed_page_geometry() {
        let mut cache = LayoutCache::default();
        let layout = cache.get(
            LayoutKey {
                width: 600.0,
                zoom: 1.0,
                revision: 0,
                rotation: 0,
                gap: 20.0,
            },
            &[
                SizeMm::new(200.0, 300.0),
                SizeMm::new(300.0, 200.0),
                SizeMm::new(200.0, 300.0),
            ],
        );
        assert_eq!(
            layout.sizes,
            vec![(600.0, 900.0), (600.0, 400.0), (600.0, 900.0)]
        );
        assert_eq!(layout.top(1), PAD + 900.0 + 20.0);
        assert_eq!(layout.page_at(layout.top(2)), 2);
        assert_eq!(layout.page_at(-500.0), 0);
        assert_eq!(layout.page_at(f32::MAX), 2);
        assert_eq!(layout.height, PAD * 2.0 + 2200.0 + 20.0 * 2.0);
        assert_eq!(layout.height - layout.bottom(2), PAD);
        for range in [0..1, 1..2, 1..3, 0..3] {
            let (top, bottom) = layout.spacers(range.clone());
            let rows: f32 = range
                .map(|index| {
                    layout.sizes[index].1
                        + if index + 1 < layout.sizes.len() {
                            20.0
                        } else {
                            0.0
                        }
                })
                .sum();
            assert_eq!(
                top + rows + bottom,
                layout.height,
                "virtualization must not change scroll extent"
            );
        }
    }

    #[test]
    fn ten_thousand_pages_only_need_a_small_visible_window() {
        let mut cache = LayoutCache::default();
        let pages = vec![SizeMm::new(210.0, 297.0); 10_000];
        let key = LayoutKey {
            width: 800.0,
            zoom: 1.0,
            revision: 0,
            rotation: 0,
            gap: 20.0,
        };
        let address = cache.get(key, &pages).sizes.as_ptr();
        for page in [0, 500, 9000, 9999] {
            let layout = cache.get(key, &pages);
            assert_eq!(
                address,
                layout.sizes.as_ptr(),
                "scrolling must not rebuild layout"
            );
            let range = layout.visible(layout.top(page), 860.0);
            assert!(range.contains(&page));
            assert!(range.len() <= 4);
        }
        let layout = cache.get(LayoutKey { zoom: 2.0, ..key }, &pages);
        assert_eq!(layout.sizes[0].0, 1600.0);
    }

    #[test]
    fn configurable_gap_changes_scroll_extent_without_touching_page_sizes() {
        let mut cache = LayoutCache::default();
        let pages = [SizeMm::new(200.0, 300.0); 2];
        let compact = LayoutKey {
            width: 600.0,
            zoom: 1.0,
            revision: 0,
            rotation: 0,
            gap: 10.0,
        };
        let wide = LayoutKey {
            gap: 32.0,
            ..compact
        };
        let a = cache.get(compact, &pages);
        let compact_height = a.height;
        let compact_top = a.top(1);
        let sizes = a.sizes.clone();
        assert_eq!(sizes, vec![(600.0, 900.0); 2]);
        let b = cache.get(wide, &pages);
        assert_eq!(b.sizes, sizes);
        assert_eq!(b.top(1) - compact_top, 22.0);
        assert_eq!(b.height - compact_height, 22.0);
    }

    #[test]
    fn rotation_swaps_page_footprint_and_rebuilds_layout() {
        let mut cache = LayoutCache::default();
        let pages = [SizeMm::new(200.0, 300.0)];
        let key = LayoutKey {
            width: 600.0,
            zoom: 1.0,
            revision: 0,
            rotation: 0,
            gap: 20.0,
        };
        assert_eq!(cache.get(key, &pages).sizes[0], (600.0, 900.0));
        let rotated = cache.get(LayoutKey { rotation: 1, ..key }, &pages);
        assert_eq!(rotated.sizes[0], (600.0, 400.0));
        assert_eq!(
            cache.get(LayoutKey { rotation: 2, ..key }, &pages).sizes[0],
            (600.0, 900.0)
        );
    }

    #[test]
    fn high_dpi_working_set_fits_the_cache_budget() {
        let page = SizeMm::new(210.0, 297.0);
        let budget = 128 * 1024 * 1024;
        for count in [2, 4, 8] {
            let dpi = budgeted_dpi(384, page, count, budget, 384);
            let width = (page.width * f64::from(dpi) / 25.4).ceil();
            let height = (page.height * f64::from(dpi) / 25.4).ceil();
            assert!(width * height * 4.0 * count as f64 <= budget as f64);
            assert!(dpi < 384);
        }
    }

    #[test]
    fn page_density_follows_the_preset_not_the_screen() {
        let a4 = SizeMm::new(210.0, 297.0);
        let budget = 384 * 1024 * 1024;
        // 100%: exactly the preset, for visible pages and prefetch alike.
        assert_eq!(page_dpi(192, 192, a4, 3, budget), 192);
        assert_eq!(page_dpi(384, 384, a4, 3, budget), 384);
        // Zoomed in: the page bitmap stays at the preset, tiles add detail.
        assert_eq!(page_dpi(288, 192, a4, 3, budget), 192);
        // Zoomed out: lower density is enough.
        assert_eq!(page_dpi(96, 192, a4, 3, budget), 96);
        // A0 at 384 DPI would exceed the 8192 px texture side.
        let a0 = SizeMm::new(841.0, 1189.0);
        let dpi = page_dpi(384, 384, a0, 1, usize::MAX / 4);
        assert!(f64::from(dpi) * 1189.0 / 25.4 <= 8192.0, "{dpi}");
        // A small cache lowers density rather than overflowing.
        assert!(page_dpi(384, 384, a4, 3, 64 * 1024 * 1024) < 384);
    }

    #[test]
    fn prefetch_never_exceeds_the_room_left_by_visible_pages() {
        const MIB: usize = 1024 * 1024;
        let a4_192 = 15 * MIB;
        // Smallest cache: 48 MiB of pages, two A4 pages on screen.
        let room = (48 * MIB).saturating_sub(2 * a4_192);
        assert_eq!(prefetch_fit([a4_192; 8], room, 30), 1);
        // On-screen pages already fill the cache: nothing is prefetched.
        assert_eq!(prefetch_fit([a4_192; 8], 0, 30), 0);
        // The count limit applies as well.
        assert_eq!(prefetch_fit([MIB; 8], 512 * MIB, 3), 3);
        // A page that does not fit stops the list; a smaller, lower priority
        // page behind it is not taken in its place.
        assert_eq!(prefetch_fit([MIB, 40 * MIB, MIB], 10 * MIB, 30), 1);
    }

    #[test]
    fn wheel_burst_admits_at_most_one_job_per_slot() {
        let mut jobs = 0;
        for _ in 0..1000 {
            if render_ready(jobs, 4) {
                jobs += 1;
            }
        }
        assert_eq!(jobs, 4, "a scroll burst fills the slots, not a backlog");
        assert!(render_ready(0, 0), "a zero slot count still admits one job");
        assert!(!render_ready(1, 0));
    }

    #[test]
    #[ignore = "manual geometry benchmark; run with --release --ignored --nocapture"]
    fn scrolling_geometry_benchmark() {
        use std::{hint::black_box, time::Instant};
        let pages = vec![SizeMm::new(210.0, 297.0); 10_000];
        let key = LayoutKey {
            width: 800.0,
            zoom: 1.0,
            revision: 0,
            rotation: 0,
            gap: 20.0,
        };
        let mut cache = LayoutCache::default();
        let layout = cache.get(key, &pages);
        let offsets: Vec<_> = (0..2000)
            .map(|index| layout.height * index as f32 / 2000.0)
            .collect();
        let start = Instant::now();
        for offset in &offsets {
            // Former hot path: scan from document start for every wheel event.
            let mut top = PAD;
            let mut page = 0;
            for size in &pages {
                let height = key.width * (size.height / size.width) as f32;
                if *offset < top + height + 20.0 * 0.5 {
                    break;
                }
                top += height + 20.0;
                page += 1;
            }
            black_box(page);
        }
        let scan = start.elapsed();
        let start = Instant::now();
        let mut largest_window = 0;
        for offset in offsets {
            let layout = cache.get(key, &pages);
            let range = layout.visible(offset, 860.0);
            largest_window = largest_window.max(range.len());
            black_box(range);
        }
        println!(
            "10,000 pages / 2,000 scroll positions: linear scan {:?}, cached lookup {:?}, max live rows {}",
            scan,
            start.elapsed(),
            largest_window
        );
        assert!(largest_window <= 4);
    }
}
