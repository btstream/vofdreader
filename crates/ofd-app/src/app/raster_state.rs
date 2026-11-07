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
use crate::app::render_policy::{self, PageRequest};
use crate::app::task_state::{InFlightTasks, RenderEpoch};
use crate::tiles;
use gpui_kit::RenderImage;

pub(crate) struct RasterState {
    pub(crate) pending_page: Option<PendingPage>,
    /// Bitmap currently on screen. Kept across an image-filter change until
    /// its replacement arrives; `image_dpi = 0` forces a re-render.
    pub(crate) page_image: Option<Arc<RenderImage>>,
    pub(crate) image_dpi: u32,
    pub(crate) page_cache: HashMap<(usize, u32), Arc<RenderImage>>,
    pub(crate) page_cache_order: VecDeque<(usize, u32)>,
    pub(crate) page_cache_bytes: usize,
    /// The entire affordable neighbour plan, not just the next insertion.
    /// Protect it from unrelated LRU touches by buffered continuous rows.
    pub(crate) page_prefetch: HashSet<usize>,
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
    // Registry ownership outlives cache eviction until the next window frame.
    registered: HashSet<BitmapId>,
    /// Raster jobs currently occupying a worker slot.
    pub(crate) tasks: InFlightTasks,
    /// Bitmap accounting shared by the caches, the on-screen slot and the
    /// jobs that are still rendering.
    pub(crate) budget: BudgetLedger,
}

/// A completed foreground output remains reserved until a window frame can
/// retire registry ownership and atomically replace the displayed bitmap.
pub(crate) struct PendingPage {
    pub(crate) request: PageRequest,
    pub(crate) epoch: RenderEpoch,
    pub(crate) image: Arc<RenderImage>,
    pub(crate) bytes: usize,
    pub(crate) reservation: Reservation,
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
            pending_page: None,
            page_image: None,
            image_dpi: 0,
            page_cache: HashMap::new(),
            page_cache_order: VecDeque::new(),
            page_cache_bytes: 0,
            page_prefetch: HashSet::new(),
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
            registered: HashSet::new(),
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
    /// must release the reservation when the closure returns, then separately
    /// admit the result only if it is still useful.
    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<Reservation, BudgetRefusal> {
        self.budget.reserve(bytes)
    }

    /// Whether `bytes` fits the configured capacity, ignoring temporary jobs.
    pub(crate) fn can_ever_reserve(&self, bytes: usize) -> Result<(), BudgetRefusal> {
        self.budget.can_ever_reserve(bytes)
    }

    pub(crate) fn affordable_page_dpi(
        &self,
        size: ofd_core::SizeMm,
        wanted: u32,
        floor: u32,
        rotation: u8,
    ) -> Option<u32> {
        render_policy::affordable_dpi(wanted, floor.min(wanted), |dpi| {
            self.can_ever_reserve(render_policy::page_raster_estimate(size, dpi, rotation))
                .is_ok()
        })
    }

    /// Avoid rasterizing a background frame that cannot coexist with the
    /// protected working set even after off-screen/retired owners are gone.
    pub(crate) fn page_working_set_fits(
        &self,
        page: usize,
        bytes: usize,
        on_screen: Range<usize>,
    ) -> bool {
        let mut seen = HashSet::new();
        let mut retained = bytes;
        for ((cached_page, _), image) in &self.page_cache {
            if *cached_page != page && on_screen.contains(cached_page) && seen.insert(image.id) {
                retained = retained.saturating_add(image_bytes(image));
            }
        }
        if let Some(image) = &self.page_image
            && seen.insert(image.id)
        {
            retained = retained.saturating_add(image_bytes(image));
        }
        for image in self
            .tile_wanted
            .values()
            .flatten()
            .filter_map(|key| self.tile_cache.get(key))
        {
            if seen.insert(image.id) {
                retained = retained.saturating_add(image_bytes(image));
            }
        }
        self.budget.fits_resident_capacity(retained)
    }

    pub(crate) fn cancel_reservation(&mut self, reservation: Reservation) {
        self.budget.cancel(reservation);
    }

    pub(crate) fn mark_registered(&mut self, image: &RenderImage) {
        self.registered.insert(Self::bitmap_id(image));
    }

    /// Called after atlas reconciliation (or explicit registry teardown).
    pub(crate) fn reconcile_registered(&mut self, retained: impl Fn(BitmapId) -> bool) {
        let retired: Vec<_> = self
            .registered
            .iter()
            .copied()
            .filter(|id| !retained(*id))
            .collect();
        for id in retired {
            self.registered.remove(&id);
            self.release_if_unreferenced(id);
        }
    }

    /// Refused/never-admitted pixels must never become the displayed frame.
    pub(crate) fn set_page_image(&mut self, image: Option<Arc<RenderImage>>) -> bool {
        if image
            .as_ref()
            .is_some_and(|image| !self.budget.contains(Self::bitmap_id(image)))
        {
            return false;
        }
        let old = std::mem::replace(&mut self.page_image, image);
        if let Some(old) = old {
            self.release_if_unreferenced(Self::bitmap_id(&old));
        }
        true
    }

    pub(crate) fn prepare_page_replacement(&mut self, page: usize) {
        self.set_page_image(None);
        let keys: Vec<_> = self
            .page_cache
            .keys()
            .copied()
            .filter(|key| key.0 == page)
            .collect();
        for key in keys {
            self.remove_page(key);
        }
    }

    /// Drops caches and marks in-flight results stale, while keeping the
    /// displayed bitmap.
    ///
    /// Running jobs stay in [`Self::tasks`]: their worker slots and pixel
    /// buffers are still in use until the closure returns. Removing them here
    /// would make the budget believe memory had been freed and allow work that
    /// cannot actually run in parallel.
    pub(crate) fn invalidate(&mut self) {
        if let Some(pending) = self.pending_page.take() {
            self.cancel_reservation(pending.reservation);
        }
        self.view_generation = self.view_generation.wrapping_add(1);
        self.render_seq = self.render_seq.wrapping_add(1);
        self.latest_render = None;
        self.image_dpi = 0;
        let released: Vec<_> = self
            .page_cache
            .values()
            .map(|image| Self::bitmap_id(image))
            .collect();
        self.page_cache.clear();
        self.page_cache_order.clear();
        self.page_prefetch.clear();
        self.page_cache_bytes = 0;
        self.clear_tiles();
        self.continuous_failed.clear();
        for id in released {
            self.release_if_unreferenced(id);
        }
    }

    /// Returns to the empty state when a document is closed or replaced.
    pub(crate) fn reset(&mut self) {
        self.invalidate();
        self.set_page_image(None);
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
        // Nothing is charged until admission: make room first, then
        // admit, so a rejected frame leaves no residue.
        self.evict_until_admissible(bitmap, bytes, &eviction, key);
        if !eviction.on_screen.contains(&page)
            && (self.page_cache.len().saturating_add(1) > eviction.count_limit
                || bytes > eviction.byte_limit.saturating_sub(self.page_cache_bytes))
        {
            // A late speculative result cannot exceed the plan's cache slice
            // just because all useful entries are now protected.
            return false;
        }
        if self.budget.can_admit(bitmap, bytes).is_err() {
            let visible: HashSet<_> = self.tile_wanted.values().flatten().copied().collect();
            let expendable: Vec<_> = self
                .tile_order
                .iter()
                .copied()
                .filter(|key| !visible.contains(key))
                .collect();
            for key in expendable {
                if self.budget.can_admit(bitmap, bytes).is_ok() {
                    break;
                }
                self.remove_tile(key);
            }
        }
        if !self.budget.admit(bitmap, bytes) {
            return false;
        }

        if let Some(old) = self.page_cache.insert(key, image) {
            let old_id = Self::bitmap_id(&old);
            self.release_if_unreferenced(old_id);
        }
        self.page_cache_bytes = self
            .page_cache
            .values()
            .map(|image| image_bytes(image))
            .sum();
        self.touch_page(page, dpi);
        true
    }

    /// Trims caches until the ledger is back within its resident limit, in a
    /// deliberate order: stale tiles first, then off-screen pages, never the
    /// pages currently built in the continuous view.
    ///
    /// Called when the user lowers the cache setting. Returns the number of
    /// entries dropped so callers can decide whether to re-render.
    pub(crate) fn shrink_to_budget(&mut self, on_screen: Range<usize>, current: usize) -> usize {
        let mut dropped = 0;
        let over = |state: &Self| state.budget.over_resident_limit();
        if !over(self) {
            return 0;
        }
        // Prefetched tiles are the least valuable resident pixels.
        let visible_tiles: HashSet<_> = self.tile_wanted.values().flatten().copied().collect();
        let mut tile_candidates: Vec<_> = self.tile_order.iter().copied().collect();
        tile_candidates.sort_by_key(|key| visible_tiles.contains(key));
        for key in tile_candidates {
            if !over(self) {
                break;
            }
            // Visible tiles stay until the view's new density plan replaces them.
            if visible_tiles.contains(&key) {
                continue;
            }
            self.tile_order.retain(|entry| *entry != key);
            if let Some(image) = self.tile_cache.remove(&key) {
                self.tile_bytes = self
                    .tile_cache
                    .values()
                    .map(|image| image_bytes(image))
                    .sum();
                self.release_if_unreferenced(Self::bitmap_id(&image));
                dropped += 1;
            }
        }
        // Then off-screen page bitmaps, oldest first.
        let candidates = self.page_cache_order.len();
        for _ in 0..candidates {
            if !over(self) {
                break;
            }
            let Some(key) = self.page_cache_order.pop_front() else {
                break;
            };
            let visible = key.0 == current || on_screen.contains(&key.0);
            if visible {
                self.page_cache_order.push_back(key);
                continue;
            }
            self.remove_page(key);
            dropped += 1;
            if self.page_cache_order.is_empty() {
                break;
            }
        }
        dropped
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
                *entry == key
                    || *entry == protected
                    || on_screen.contains(&entry.0)
                    || self.page_prefetch.contains(&entry.0)
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

    pub(crate) fn remove_page(&mut self, key: (usize, u32)) {
        if let Some(old) = self.page_cache.remove(&key) {
            self.page_cache_bytes = self
                .page_cache
                .values()
                .map(|image| image_bytes(image))
                .sum();
            self.release_if_unreferenced(Self::bitmap_id(&old));
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

    pub(crate) fn remove_tile(&mut self, key: tiles::Key) {
        if let Some(image) = self.tile_cache.remove(&key) {
            self.tile_order.retain(|entry| *entry != key);
            self.tile_bytes = self
                .tile_cache
                .values()
                .map(|image| image_bytes(image))
                .sum();
            self.release_if_unreferenced(Self::bitmap_id(&image));
        }
    }

    /// Releases a ledger entry only when no cache or on-screen slot still
    /// holds the same allocation.
    fn release_if_unreferenced(&mut self, id: BitmapId) {
        let referenced = self.registered.contains(&id)
            || self
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
        if let Some(old) = self.tile_cache.insert(key, image) {
            self.release_if_unreferenced(Self::bitmap_id(&old));
        }
        self.tile_order.retain(|entry| *entry != key);
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
        state.cache_page(0, 96, render_image(1, 1), 4, eviction((0, 96)));
        // A second insertion is over both limits, but the new entry is
        // protected; the older one is evicted instead.
        state.cache_page(1, 96, render_image(1, 1), 4, eviction((1, 96)));
        assert!(state.page_cache.contains_key(&(1, 96)));
        assert_eq!(state.page_cache.len(), 1);
    }

    #[test]
    fn on_screen_pages_survive_a_tight_budget() {
        let mut state = RasterState::with_budget(BudgetLimits::split(8));
        let eviction = || PageEviction {
            count_limit: 1,
            byte_limit: 4,
            protected: (0, 96),
            on_screen: 0..2,
        };
        assert!(state.cache_page(0, 96, render_image(1, 1), 4, eviction()));
        assert!(state.cache_page(1, 96, render_image(1, 1), 4, eviction()));
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
        state.cache_page(0, 96, render_image(1, 1), 4, eviction(0));
        state.cache_page(0, 192, render_image(1, 1), 4, eviction(0));
        assert!(!state.page_cache.contains_key(&(0, 96)));
        assert!(state.page_cache.contains_key(&(0, 192)));
    }

    #[test]
    fn invalidate_keeps_the_visible_bitmap_until_its_replacement() {
        let mut state = RasterState::with_budget(BudgetLimits::split(4));
        let image = render_image(1, 1);
        state.image_dpi = 192;
        state.cache_page(
            0,
            192,
            image,
            4,
            PageEviction {
                count_limit: 8,
                byte_limit: 1 << 20,
                protected: (0, 192),
                on_screen: 0..0,
            },
        );
        state.set_page_image(state.page_cache.get(&(0, 192)).cloned());
        state.invalidate();
        assert!(state.page_image.is_some());
        assert_eq!(state.image_dpi, 0);
        assert!(state.page_cache.is_empty());
        assert_eq!(state.budget.usage().resident_bytes, 4);
        state.set_page_image(None);
        assert_eq!(state.budget.usage().resident_bytes, 0);
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

    fn roomy(protected: (usize, u32)) -> PageEviction {
        PageEviction {
            count_limit: 8,
            byte_limit: 4096,
            protected,
            on_screen: 0..0,
        }
    }

    #[test]
    fn reset_releases_pixels_but_preserves_physical_jobs() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        let image = render_image(2, 2);
        assert!(state.cache_page(0, 96, image.clone(), 16, roomy((0, 96))));
        state.set_page_image(Some(image));
        let reservation = state.reserve(16).unwrap();
        let ticket = state.tasks.start(
            crate::app::task_state::TaskTarget::page(1, 96),
            16,
            RenderEpoch::default(),
        );
        state.reset();
        assert_eq!(state.budget.usage().resident_bytes, 0);
        assert_eq!(state.budget.usage().reserved_bytes, 16);
        assert_eq!(state.tasks.len(), 1);
        state.tasks.finish(ticket);
        state.cancel_reservation(reservation);
        assert_eq!(state.budget.usage().reserved_bytes, 0);
        assert!(state.tasks.is_empty());
    }

    #[test]
    fn discarded_output_never_becomes_resident() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        let reservation = state.reserve(16).unwrap();
        state.invalidate();
        let result = render_image(2, 2);
        state.cancel_reservation(reservation);
        drop(result);
        assert_eq!(state.budget.usage().resident_bytes, 0);
        assert_eq!(state.budget.usage().reserved_bytes, 0);
    }

    #[test]
    fn registry_retirement_not_cache_eviction_releases_registered_pixels() {
        let mut state = RasterState::with_budget(BudgetLimits::split(16));
        let image = render_image(2, 2);
        assert!(state.cache_page(0, 96, image.clone(), 16, roomy((0, 96))));
        state.mark_registered(&image);
        state.set_page_image(Some(image));
        state.invalidate();
        state.set_page_image(None);
        assert_eq!(state.budget.usage().resident_bytes, 16);
        state.reconcile_registered(|_| true); // failed atlas drop: still owned
        assert_eq!(state.budget.usage().resident_bytes, 16);
        state.reconcile_registered(|_| false);
        assert_eq!(state.budget.usage().resident_bytes, 0);
    }

    #[test]
    fn replacing_screen_and_same_key_tile_does_not_leak_the_old_allocation() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        let old = render_image(1, 1);
        assert!(state.cache_page(0, 96, old.clone(), 4, roomy((0, 96))));
        state.set_page_image(Some(old));
        assert!(state.cache_page(0, 192, render_image(1, 1), 4, roomy((0, 192))));
        assert_eq!(state.budget.usage().resident_bytes, 8);
        state.set_page_image(state.page_cache.get(&(0, 192)).cloned());
        assert_eq!(state.budget.usage().resident_bytes, 4);
        let key = tiles::Key {
            page: 0,
            dpi: 192,
            x: 0,
            y: 0,
        };
        assert!(state.cache_tile(key, render_image(1, 1), 4, 64, &[]));
        assert!(state.cache_tile(key, render_image(1, 1), 4, 64, &[]));
        assert_eq!(state.budget.usage().resident_bytes, 8);
        assert_eq!(state.tile_order.len(), 1);
    }

    #[test]
    fn shrinking_multiple_protected_pages_terminates_even_when_over_budget() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        for page in 0..2 {
            assert!(state.cache_page(page, 96, render_image(2, 2), 16, roomy((page, 96))));
        }
        state.set_budget_limits(BudgetLimits::split(16));
        assert_eq!(state.shrink_to_budget(0..2, 0), 0);
        assert_eq!(state.page_cache.len(), 2);
        assert_eq!(state.budget.usage().resident_bytes, 32);
    }

    #[test]
    fn shrinking_does_not_evict_visible_tiles_as_prefetch() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        let visible = tiles::Key {
            page: 0,
            dpi: 96,
            x: 0,
            y: 0,
        };
        let prefetch = tiles::Key { x: 1, ..visible };
        assert!(state.cache_tile(visible, render_image(2, 2), 16, 64, &[]));
        assert!(state.cache_tile(prefetch, render_image(2, 2), 16, 64, &[]));
        state.tile_wanted.insert(0, vec![visible]);
        state.set_budget_limits(BudgetLimits::split(16));
        assert_eq!(state.shrink_to_budget(0..1, 0), 1);
        assert!(state.tile_cache.contains_key(&visible));
        assert!(!state.tile_cache.contains_key(&prefetch));
    }

    #[test]
    fn density_keys_ignore_temporary_contention_and_recover_with_capacity() {
        let size = ofd_core::SizeMm::new(210.0, 297.0);
        let capacity = render_policy::page_raster_estimate(size, 144, 0);
        let mut state = RasterState::with_budget(BudgetLimits {
            resident_bytes: capacity * 4,
            reserved_bytes: capacity,
        });
        let resolved = state.affordable_page_dpi(size, 384, 96, 0).unwrap();
        assert_eq!(resolved, 144);
        let reservation = state.reserve(capacity).unwrap();
        assert_eq!(state.affordable_page_dpi(size, 384, 96, 0), Some(resolved));
        let target = crate::app::task_state::TaskTarget::page(0, resolved);
        let ticket = state.tasks.start(target, capacity, RenderEpoch::default());
        assert!(state.tasks.contains_current(target, RenderEpoch::default()));
        state.tasks.finish(ticket);
        state.cancel_reservation(reservation);
        assert!(state.cache_page(0, resolved, render_image(1, 1), 4, roomy((0, resolved))));
        assert!(
            state
                .cached_page(0, state.affordable_page_dpi(size, 384, 96, 0).unwrap())
                .is_some()
        );
        assert_eq!(state.affordable_page_dpi(size, 96, 96, 0), Some(96));
        assert_eq!(
            state.affordable_page_dpi(ofd_core::SizeMm::new(50.0, 50.0), 384, 96, 0),
            Some(384)
        );
        state.set_budget_limits(BudgetLimits::split(capacity * 16));
        assert!(state.affordable_page_dpi(size, 384, 96, 0).unwrap() > resolved);
    }

    #[test]
    fn pending_replacement_stays_reserved_and_invalidation_cancels_it() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        let reservation = state.reserve(16).unwrap();
        state.pending_page = Some(PendingPage {
            request: PageRequest {
                seq: 1,
                page: 0,
                dpi: 96,
            },
            epoch: RenderEpoch::default(),
            image: render_image(2, 2),
            bytes: 16,
            reservation,
        });
        assert_eq!(state.budget.usage().reserved_bytes, 16);
        assert_eq!(state.budget.usage().resident_bytes, 0);
        state.invalidate();
        assert!(state.pending_page.is_none());
        assert_eq!(state.budget.usage().reserved_bytes, 0);
    }

    #[test]
    fn replacement_after_lowering_budget_retires_old_registry_before_display() {
        let mut state = RasterState::with_budget(BudgetLimits::split(256));
        let old = render_image(4, 4);
        assert!(state.cache_page(0, 192, old.clone(), 64, roomy((0, 192))));
        state.mark_registered(&old);
        assert!(state.set_page_image(Some(old)));
        state.set_budget_limits(BudgetLimits::split(16));
        let reservation = state.reserve(4).unwrap();
        let next = render_image(1, 1);
        assert!(
            !state.set_page_image(Some(next.clone())),
            "not admitted yet"
        );
        state.prepare_page_replacement(0);
        assert!(!state.cache_page(0, 96, next.clone(), 4, roomy((0, 96))));
        assert!(
            !state.set_page_image(Some(next.clone())),
            "registry still owns old pixels"
        );
        assert_eq!(state.budget.usage().reserved_bytes, 4);
        state.reconcile_registered(|_| false);
        assert!(state.cache_page(0, 96, next.clone(), 4, roomy((0, 96))));
        assert!(state.set_page_image(Some(next)));
        state.cancel_reservation(reservation);
        assert_eq!(state.budget.usage().resident_bytes, 4);
        assert_eq!(state.budget.usage().reserved_bytes, 0);
    }

    #[test]
    fn impossible_background_working_set_is_rejected_before_rasterizing() {
        let mut state = RasterState::with_budget(BudgetLimits::split(32));
        let shown = render_image(2, 2);
        assert!(state.cache_page(0, 96, shown.clone(), 16, roomy((0, 96))));
        state.set_page_image(Some(shown));
        assert!(state.page_working_set_fits(1, 16, 0..1));
        assert!(!state.page_working_set_fits(1, 20, 0..1));
        assert_eq!(state.budget.usage().reserved_bytes, 0);
    }

    #[test]
    fn foreground_page_can_reclaim_speculative_tile_capacity() {
        let mut state = RasterState::with_budget(BudgetLimits::split(16));
        let tile = tiles::Key {
            page: 1,
            dpi: 192,
            x: 0,
            y: 0,
        };
        assert!(state.cache_tile(tile, render_image(2, 2), 16, 16, &[]));
        assert!(state.cache_page(0, 96, render_image(1, 1), 4, roomy((0, 96))));
        assert!(state.tile_cache.is_empty());
        assert_eq!(state.budget.usage().resident_bytes, 4);
    }

    #[test]
    fn affordable_prefetch_plan_survives_buffered_row_lru_touches() {
        let mut state = RasterState::with_budget(BudgetLimits::split(64));
        let eviction = || PageEviction {
            count_limit: 12,
            byte_limit: 48,
            protected: (2, 0),
            on_screen: 2..3,
        };
        for page in [2, 3, 0] {
            assert!(state.cache_page(page, 168, render_image(2, 2), 16, eviction()));
        }
        // The viewport needs page 2 and can afford neighbours 0 and 1. A
        // buffered row for page 3 used to displace these neighbours forever.
        state.page_prefetch.extend([0, 1]);
        state.touch_page(3, 168);
        assert!(state.cache_page(1, 168, render_image(2, 2), 16, eviction()));
        assert!(!state.page_cache.contains_key(&(3, 168)));
        assert!(
            !state.cache_page(3, 168, render_image(2, 2), 16, eviction()),
            "late off-plan output cannot evict the affordable working set"
        );
        for _ in 0..100 {
            for page in [0, 1, 2] {
                assert!(state.cached_page(page, 168).is_some());
            }
        }
        assert_eq!(state.page_cache.len(), 3);
        assert_eq!(state.budget.usage().resident_bytes, 48);
    }

    #[test]
    fn shrinking_keeps_on_screen_pages_and_drops_prefetched_tiles_first() {
        let mut state = RasterState::with_budget(BudgetLimits {
            resident_bytes: 4096,
            reserved_bytes: 1024,
        });
        let roomy = |protected| PageEviction {
            count_limit: 64,
            byte_limit: 1 << 20,
            protected,
            on_screen: 0..0,
        };
        // Two pages of 4x4 pixels (64 bytes each) plus a tile.
        assert!(state.cache_page(0, 96, render_image(4, 4), 64, roomy((0, 96))));
        assert!(state.cache_page(5, 96, render_image(4, 4), 64, roomy((5, 96))));
        assert!(state.cache_tile(
            tiles::Key {
                page: 0,
                dpi: 96,
                x: 0,
                y: 0
            },
            render_image(2, 2),
            16,
            1 << 20,
            &[],
        ));

        // The user lowers the setting below what is currently retained.
        state.set_budget_limits(BudgetLimits {
            resident_bytes: 64,
            reserved_bytes: 16,
        });
        assert!(state.budget.over_resident_limit());

        // Page 0 is on screen; page 5 is not.
        let dropped = state.shrink_to_budget(0..1, 0);

        assert!(dropped > 0);
        assert!(
            state.page_cache.contains_key(&(0, 96)),
            "visible page stays"
        );
        assert!(!state.page_cache.contains_key(&(5, 96)));
        assert!(state.tile_cache.is_empty(), "prefetch tiles go first");
        assert!(!state.budget.over_resident_limit());
    }

    #[test]
    fn shrinking_does_nothing_when_already_within_budget() {
        let mut state = RasterState::with_budget(BudgetLimits {
            resident_bytes: 1024,
            reserved_bytes: 256,
        });
        assert!(state.cache_page(
            0,
            96,
            render_image(2, 2),
            16,
            PageEviction {
                count_limit: 8,
                byte_limit: 1 << 20,
                protected: (0, 96),
                on_screen: 0..0,
            }
        ));
        assert_eq!(state.shrink_to_budget(0..1, 0), 0);
        assert!(state.page_cache.contains_key(&(0, 96)));
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
