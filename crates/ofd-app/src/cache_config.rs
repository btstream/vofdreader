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
    pub fn tile_working_bytes(self) -> usize {
        self.tile_bytes().min(32 * MIB)
    }
}

#[cfg(test)]
mod tests;
