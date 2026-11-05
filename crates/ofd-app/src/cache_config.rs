//! Cache policy boundary for a future settings/config-file adapter.
//! Limits cover retained raster results, not whole-process RAM or GPU memory.
const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct CacheConfig {
    total_bytes: usize,
    page_limit: usize,
    parsed_page_limit: usize,
    prefetch_pages: usize,
    prefetch_tile_ring: u32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self::new(256, 32, 12).with_prefetch(2, 1)
    }
}

impl CacheConfig {
    /// Central validation for future user settings. Apply when constructing a
    /// reader; changing a live reader's policy/evicting its cache is not exposed.
    pub fn new(total_mib: usize, page_limit: usize, parsed_page_limit: usize) -> Self {
        Self {
            total_bytes: total_mib.clamp(64, 2048) * MIB,
            page_limit: page_limit.clamp(4, 256),
            parsed_page_limit: parsed_page_limit.clamp(1, 64),
            prefetch_pages: 2,
            prefetch_tile_ring: 1,
        }
    }

    pub fn page_bytes(self) -> usize {
        self.total_bytes / 4 * 3
    }
    pub fn tile_bytes(self) -> usize {
        self.total_bytes / 4
    }
    pub fn page_limit(self) -> usize {
        self.page_limit
    }
    pub fn parsed_page_limit(self) -> usize {
        self.parsed_page_limit
    }

    /// Zero disables the respective prefetch path; larger ranges are bounded.
    pub fn with_prefetch(mut self, pages: usize, tile_ring: u32) -> Self {
        self.prefetch_pages = pages.min(4);
        self.prefetch_tile_ring = tile_ring.min(2);
        self
    }

    pub fn prefetch_pages(self) -> usize {
        self.prefetch_pages
    }
    pub fn prefetch_tile_ring(self) -> u32 {
        self.prefetch_tile_ring
    }

    // Separate active rendering quality from retention capacity: increasing
    // cache space should save history, not generate higher-density bitmaps.
    pub fn page_working_bytes(self) -> usize {
        self.page_bytes().min(96 * MIB)
    }
    pub fn tile_working_bytes(self) -> usize {
        self.tile_bytes().min(32 * MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefetch_defaults_can_be_disabled_or_bounded_by_future_settings() {
        let config = CacheConfig::default();
        assert_eq!(
            (config.prefetch_pages(), config.prefetch_tile_ring()),
            (2, 1)
        );
        let disabled = config.with_prefetch(0, 0);
        assert_eq!(
            (disabled.prefetch_pages(), disabled.prefetch_tile_ring()),
            (0, 0)
        );
        let bounded = config.with_prefetch(usize::MAX, u32::MAX);
        assert_eq!(
            (bounded.prefetch_pages(), bounded.prefetch_tile_ring()),
            (4, 2)
        );
    }
    #[test]
    fn larger_cache_retains_history_without_increasing_render_density_budget() {
        let old = CacheConfig::new(128, 8, 4);
        let new = CacheConfig::default();
        assert_eq!(new.page_bytes(), 192 * MIB);
        assert_eq!(new.tile_bytes(), 64 * MIB);
        assert_eq!(new.page_working_bytes(), old.page_working_bytes());
        assert_eq!(new.tile_working_bytes(), old.tile_working_bytes());
        assert_eq!(new.page_limit(), 32);
        // A4 96-DPI previews (~3.4 MiB) used to hit the 8-page count limit
        // even with unused byte budget; a return trip over 24 pages now fits.
        assert!(24 <= new.page_limit());
        assert!(24 * 4 * MIB < new.page_bytes());
    }
    #[test]
    fn user_policy_boundary_clamps_extreme_values() {
        for input in [0, 1, 128, 256, usize::MAX] {
            let config = CacheConfig::new(input, input, input);
            assert!((64 * MIB..=2048 * MIB).contains(&config.total_bytes));
            assert_eq!(
                config.page_bytes() + config.tile_bytes(),
                config.total_bytes
            );
            assert!((4..=256).contains(&config.page_limit()));
            assert!((1..=64).contains(&config.parsed_page_limit()));
            assert!(config.page_working_bytes() <= config.page_bytes());
            assert!(config.tile_working_bytes() <= config.tile_bytes());
        }
    }
}
