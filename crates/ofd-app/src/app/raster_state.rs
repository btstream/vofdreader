//! Per-document raster products: displayed page bitmap, page/tile caches,
//! in-flight job bookkeeping and the generations that reject stale results.
//!
//! Cache insertion and eviction live here so callers only provide limits and
//! which entries must survive. Texture ownership stays in `gpu::Resources`:
//! this type tracks pixels, not the window atlas they may be uploaded into.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use crate::app::memory_budget::{BitmapId, BudgetLedger, BudgetLimits, BudgetRefusal, Reservation};
use crate::app::render_policy::PageRequest;
use crate::app::task_state::InFlightTasks;
use crate::tiles;
use gpui_kit::RenderImage;

pub(crate) struct RasterState {
    /// Bitmap currently on screen. Kept across an image-filter change until
    /// its replacement arrives; `image_dpi = 0` forces a re-render.
    pub(crate) page_image: Option<Arc<RenderImage>>,
    pub(crate) image_dpi: u32,
    pub(crate) page_cache: HashMap<(usize, u32), Arc<RenderImage>>,
    pub(crate) page_cache_order: VecDeque<(usize, u32)>,
    pub(crate) page_cache_bytes: usize,
    pub(crate) tile_cache: HashMap<tiles::Key, Arc<RenderImage>>,
    pub(crate) tile_order: VecDeque<tiles::Key>,
    pub(crate) tile_bytes: usize,
    pub(crate) tile_wanted: HashMap<usize, Vec<tiles::Key>>,
    pub(crate) tile_prefetch: HashMap<usize, Vec<tiles::Key>>,
    pub(crate) tile_failed: HashSet<tiles::Key>,
    pub(crate) latest_render: Option<PageRequest>,
    pub(crate) render_seq: u64,
    /// Invalidates in-flight raster jobs when view orientation changes.
    pub(crate) view_generation: u64,
    /// Debounce state for single-page density upgrades.
    pub(crate) dpi_pending: bool,
    pub(crate) dpi_debounce_gen: u64,
    /// Page jobs that failed at this density and must not be retried.
    pub(crate) continuous_failed: HashSet<(usize, u32)>,
    /// Raster jobs currently occupying a worker slot.
    pub(crate) tasks: InFlightTasks,
    /// Bitmap accounting shared by the caches, the on-screen slot and the
    /// jobs that are still rendering.
    pub(crate) budget: BudgetLedger,
}

/// Limits and protected entries for one page-cache insertion.
pub(crate) struct PageEviction {
    pub(crate) count_limit: usize,
    pub(crate) byte_limit: usize,
    /// Key of the bitmap that produced this insertion; never evicted by it.
    pub(crate) protected: (usize, u32),
    /// Pages currently built in the continuous view; never evicted.
    pub(crate) on_screen: Range<usize>,
}

impl RasterState {
    /// Builds a state whose ledger starts with the user's cache setting.
    pub(crate) fn with_budget(limits: BudgetLimits) -> Self {
        Self {
            page_image: None,
            image_dpi: 0,
            page_cache: HashMap::new(),
            page_cache_order: VecDeque::new(),
            page_cache_bytes: 0,
            tile_cache: HashMap::new(),
            tile_order: VecDeque::new(),
            tile_bytes: 0,
            tile_wanted: HashMap::new(),
            tile_prefetch: HashMap::new(),
            tile_failed: HashSet::new(),
            latest_render: None,
            render_seq: 0,
            view_generation: 0,
            dpi_pending: false,
            dpi_debounce_gen: 0,
            continuous_failed: HashSet::new(),
            tasks: InFlightTasks::new(),
            budget: BudgetLedger::new(limits),
        }
    }

    /// Applies new bitmap limits. Existing entries are kept, so the caller can
    /// observe and then release whatever is now over budget.
    pub(crate) fn set_budget_limits(&mut self, limits: BudgetLimits) {
        self.budget.set_limits(limits);
    }

    /// Bitmap identity used by the ledger. GPUI keeps one atlas entry per
    /// `ImageId`, so the same allocation is charged once even when the cache,
    /// the on-screen slot and the image registry all reference it.
    fn bitmap_id(image: &RenderImage) -> BitmapId {
        BitmapId(image.id.0 as u64)
    }

    /// Reserves the pixels a job will produce before it starts. The caller
    /// must hand the reservation back with [`Self::finish_reservation`] or
    /// [`Self::cancel_reservation`] when the closure returns.
    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<Reservation, BudgetRefusal> {
        self.budget.reserve(bytes)
    }

    pub(crate) fn cancel_reservation(&mut self, reservation: Reservation) {
        self.budget.cancel(reservation);
    }

    /// Settles a finished job. Returns whether the pixels were kept.
    pub(crate) fn finish_reservation(
        &mut self,
        reservation: Reservation,
        image: &RenderImage,
        actual_bytes: usize,
    ) -> bool {
        self.budget
            .finish(reservation, Self::bitmap_id(image), actual_bytes)
    }

    /// Drops caches and marks in-flight results stale, while keeping the
    /// displayed bitmap.
    ///
    /// Running jobs stay in [`Self::tasks`]: their worker slots and pixel
    /// buffers are still in use until the closure returns. Removing them here
    /// would make the budget believe memory had been freed and allow work that
    /// cannot actually run in parallel.
    pub(crate) fn invalidate(&mut self) {
        self.view_generation = self.view_generation.wrapping_add(1);
        self.render_seq = self.render_seq.wrapping_add(1);
        self.latest_render = None;
        self.image_dpi = 0;
        self.page_cache.clear();
        self.page_cache_order.clear();
        self.page_cache_bytes = 0;
        self.clear_tiles();
        self.continuous_failed.clear();
    }

    /// Returns to the empty state when a document is closed or replaced.
    pub(crate) fn reset(&mut self) {
        self.invalidate();
        self.page_image = None;
        self.dpi_pending = false;
        self.dpi_debounce_gen = self.dpi_debounce_gen.wrapping_add(1);
    }

    /// Exact bitmap if present, otherwise the smallest cached bitmap that is
    /// still at least as sharp as `target_dpi`.
    pub(crate) fn cached_page(
        &mut self,
        page: usize,
        target_dpi: u32,
    ) -> Option<(u32, Arc<RenderImage>)> {
        if let Some(image) = self.page_cache.get(&(page, target_dpi)).cloned() {
            self.touch_page(page, target_dpi);
            return Some((target_dpi, image));
        }
        let sharper = self
            .page_cache
            .iter()
            .filter(|((cached_page, dpi), _)| *cached_page == page && *dpi > target_dpi)
            .map(|((_, dpi), _)| *dpi)
            .min()?;
        let image = self.page_cache.get(&(page, sharper)).cloned()?;
        self.touch_page(page, sharper);
        Some((sharper, image))
    }

    /// Bytes of cached page bitmaps whose key matches.
    pub(crate) fn page_bytes_matching(&self, matches: impl Fn((usize, u32)) -> bool) -> usize {
        self.page_cache
            .iter()
            .filter(|(key, _)| matches(**key))
            .map(|(_, image)| image_bytes(image))
            .sum()
    }

    pub(crate) fn touch_page(&mut self, page: usize, dpi: u32) {
        let key = (page, dpi);
        self.page_cache_order.retain(|existing| *existing != key);
        self.page_cache_order.push_back(key);
    }

    /// Keeps decoded pages in memory within a byte budget. The displayed page
    /// is never evicted by the request that produced it.
    ///
    /// Returns whether the pixels were admitted by the ledger. A refused frame
    /// must not be kept: retaining it would break the budget the caller was
    /// told about.
    pub(crate) fn cache_page(
        &mut self,
        page: usize,
        dpi: u32,
        image: Arc<RenderImage>,
        bytes: usize,
        eviction: PageEviction,
    ) -> bool {
        let key = (page, dpi);
        // A sharper bitmap subsumes older densities; retaining both can make
        // a visible working set exceed its budget during zoom refinement.
        let obsolete: Vec<_> = self
            .page_cache
            .keys()
            .filter(|(cached_page, cached_dpi)| *cached_page == page && *cached_dpi < dpi)
            .copied()
            .collect();
        for old_key in obsolete {
            self.remove_page(old_key);
        }

        let bitmap = Self::bitmap_id(&image);
        // Nothing is charged until the languages accept: make room first, then
        // admit, so a rejected frame leaves no residue.
        self.evict_until_admissible(bitmap, bytes, &eviction, key);
        if !self.budget.admit(bitmap, bytes) {
            return false;
        }

        if let Some(old) = self.page_cache.insert(key, image) {
            let old_id = Self::bitmap_id(&old);
            if !self.page_cache.values().any(|kept| kept.id == old.id)
                && self
                    .page_image
                    .as_ref()
                    .is_none_or(|shown| shown.id != old.id)
            {
                self.budget.release(old_id);
            }
        }
        self.page_cache_bytes = self
            .page_cache
            .values()
            .map(|image| image_bytes(image))
            .sum();
        self.touch_page(page, dpi);
        true
    }

    /// Evicts cache entries that are not protected until the new bitmap fits
    /// both the count/byte limits and the ledger. The new frame is not in the
    /// cache yet, so its own size is added to the comparison.
    fn evict_until_admissible(
        &mut self,
        bitmap: BitmapId,
        bytes: usize,
        eviction: &PageEviction,
        key: (usize, u32),
    ) {
        let protected = eviction.protected;
        let on_screen = eviction.on_screen.clone();
        let over_limits = |state: &Self| {
            state.page_cache.len() + 1 > eviction.count_limit
                || state.page_cache_bytes + bytes > eviction.byte_limit
        };
        while self.budget.can_admit(bitmap, bytes).is_err() || over_limits(self) {
            if self.page_cache.is_empty() {
                break;
            }
            let Some(oldest) = self.page_cache_order.pop_front() else {
                break;
            };
            let keep = |entry: &(usize, u32)| {
                *entry == key || *entry == protected || on_screen.contains(&entry.0)
            };
            if keep(&oldest) {
                self.page_cache_order.push_back(oldest);
                if self.page_cache_order.iter().all(keep) {
                    break;
                }
                continue;
            }
            self.remove_page(oldest);
        }
    }

    fn remove_page(&mut self, key: (usize, u32)) {
        if let Some(old) = self.page_cache.remove(&key) {
            self.page_cache_bytes = self
                .page_cache
                .values()
                .map(|image| image_bytes(image))
                .sum();
            let still_owned = self
                .page_image
                .as_ref()
                .is_some_and(|shown| shown.id == old.id)
                || self.tile_cache.values().any(|tile| tile.id == old.id)
                || self.page_cache.values().any(|kept| kept.id == old.id);
            if !still_owned {
                self.budget.release(Self::bitmap_id(&old));
            }
        }
        self.page_cache_order.retain(|existing| *existing != key);
    }

    pub(crate) fn clear_tiles(&mut self) {
        let released: Vec<_> = self
            .tile_cache
            .values()
            .map(|image| Self::bitmap_id(image))
            .collect();
        self.tile_cache.clear();
        self.tile_order.clear();
        self.tile_wanted.clear();
        self.tile_prefetch.clear();
        self.tile_failed.clear();
        self.tile_bytes = 0;
        // An old document's tasks keep their slots until they return.
        for id in released {
            self.release_if_unreferenced(id);
        }
    }

    /// Releases a ledger entry only when no cache or on-screen slot still
    /// holds the same allocation.
    fn release_if_unreferenced(&mut self, id: BitmapId) {
        let referenced = self
            .page_cache
            .values()
            .any(|image| Self::bitmap_id(image) == id)
            || self
                .page_image
                .as_ref()
                .is_some_and(|image| Self::bitmap_id(image) == id)
            || self
                .tile_cache
                .values()
                .any(|image| Self::bitmap_id(image) == id);
        if !referenced {
            self.budget.release(id);
        }
    }

    /// Inserts a finished tile, evicting least-recently used entries that are
    /// not protected by the current request plan. Returns whether it fit both
    /// the caller's byte limit and the shared ledger.
    pub(crate) fn cache_tile(
        &mut self,
        key: tiles::Key,
        image: Arc<RenderImage>,
        bytes: usize,
        limit: usize,
        protected: &[tiles::Key],
    ) -> bool {
        let bitmap = Self::bitmap_id(&image);
        while self.tile_bytes + bytes > limit || self.budget.can_admit(bitmap, bytes).is_err() {
            let Some(position) = self
                .tile_order
                .iter()
                .position(|old| !protected.contains(old))
            else {
                break;
            };
            let old = self.tile_order.remove(position).expect("index is valid");
            if let Some(image) = self.tile_cache.remove(&old) {
                self.tile_bytes = self
                    .tile_cache
                    .values()
                    .map(|image| image_bytes(image))
                    .sum();
                self.release_if_unreferenced(Self::bitmap_id(&image));
            }
        }
        if self.tile_bytes + bytes > limit || !self.budget.admit(bitmap, bytes) {
            return false;
        }
        self.tile_cache.insert(key, image);
        self.tile_order.push_back(key);
        self.tile_bytes = self
            .tile_cache
            .values()
            .map(|image| image_bytes(image))
            .sum();
        true
    }
}

fn image_bytes(image: &RenderImage) -> usize {
    let size = image.size(0);
    size.width.0 as usize * size.height.0 as usize * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_image(width: u32, height: u32) -> Arc<RenderImage> {
        let buffer = image::RgbaImage::from_raw(
            width,
            height,
            vec![0u8; width as usize * height as usize * 4],
        )
        .expect("buffer size matches");
        Arc::new(RenderImage::new([image::Frame::new(buffer)]))
    }

    fn eviction(protected: (usize, u32)) -> PageEviction {
        PageEviction {
            count_limit: 1,
            byte_limit: 4,
            protected,
            on_screen: 0..0,
        }
    }

    #[test]
    fn page_cache_never_evicts_the_bitmap_that_produced_the_insert() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        state.cache_page(0, 96, render_image(1, 1), 0, eviction((0, 96)));
        // A second insertion is over both limits, but the new entry is
        // protected; the older one is evicted instead.
        state.cache_page(1, 96, render_image(1, 1), 0, eviction((1, 96)));
        assert!(state.page_cache.contains_key(&(1, 96)));
        assert_eq!(state.page_cache.len(), 1);
    }

    #[test]
    fn on_screen_pages_survive_a_tight_budget() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        let eviction = || PageEviction {
            count_limit: 1,
            byte_limit: 4,
            protected: (0, 96),
            on_screen: 0..2,
        };
        state.cache_page(0, 96, render_image(1, 1), 0, eviction());
        state.cache_page(1, 96, render_image(1, 1), 0, eviction());
        assert!(state.page_cache.contains_key(&(0, 96)));
        assert!(state.page_cache.contains_key(&(1, 96)));
    }

    #[test]
    fn sharper_pages_replace_lower_densities() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        let eviction = |page| PageEviction {
            count_limit: 8,
            byte_limit: 1 << 20,
            protected: (page, 0),
            on_screen: 0..0,
        };
        state.cache_page(0, 96, render_image(1, 1), 0, eviction(0));
        state.cache_page(0, 192, render_image(1, 1), 0, eviction(0));
        assert!(!state.page_cache.contains_key(&(0, 96)));
        assert!(state.page_cache.contains_key(&(0, 192)));
    }

    #[test]
    fn invalidate_keeps_the_visible_bitmap_until_its_replacement() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        state.page_image = Some(render_image(2, 2));
        state.image_dpi = 192;
        state.cache_page(
            0,
            192,
            render_image(1, 1),
            0,
            PageEviction {
                count_limit: 8,
                byte_limit: 1 << 20,
                protected: (0, 192),
                on_screen: 0..0,
            },
        );
        state.invalidate();
        assert!(state.page_image.is_some());
        assert_eq!(state.image_dpi, 0);
        assert!(state.page_cache.is_empty());
    }

    #[test]
    fn reset_drops_the_visible_bitmap_and_invalidates_pending_debounce() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        state.page_image = Some(render_image(1, 1));
        state.image_dpi = 96;
        let debounce = state.dpi_debounce_gen;
        state.reset();
        assert!(state.page_image.is_none());
        assert!(state.dpi_debounce_gen > debounce);
    }

    #[test]
    fn tiles_evict_unprotected_entries_but_reject_an_oversized_insert() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        let key = |x| tiles::Key {
            page: 0,
            dpi: 96,
            x,
            y: 0,
        };
        assert!(state.cache_tile(key(0), render_image(1, 1), 4, 8, &[]));
        assert!(state.cache_tile(key(1), render_image(1, 1), 4, 8, &[]));
        // Over the limit, the oldest unprotected entry is dropped.
        assert!(state.cache_tile(key(2), render_image(1, 1), 4, 8, &[key(2)]));
        assert!(!state.tile_cache.contains_key(&key(0)));
        // An entry larger than the whole budget is rejected.
        assert!(!state.cache_tile(key(3), render_image(1, 1), 16, 8, &[]));
    }
}
