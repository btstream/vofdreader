//! Pure render scheduling decisions.
//!
//! Keeping these out of the GPUI closures makes the "which background result
//! may touch the screen" rules testable without a window.

/// Identity of one whole-page raster request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PageRequest {
    pub(crate) seq: u64,
    pub(crate) page: usize,
    pub(crate) dpi: u32,
}

impl PageRequest {
    pub(crate) fn is_latest(self, latest: Option<Self>) -> bool {
        latest == Some(self)
    }

    /// A completed frame may only replace the displayed page when it belongs
    /// to the latest request, targets the visible page, and is not a
    /// downgrade. A missing bitmap accepts any density so the page is not
    /// left blank while a preview arrives.
    pub(crate) fn replaces_visible(
        self,
        current_page: usize,
        image_dpi: u32,
        image_present: bool,
    ) -> bool {
        frame_is_current(
            true,
            current_page == self.page,
            self.dpi,
            image_dpi,
            image_present,
        )
    }
}

/// A completed frame may only replace the displayed page when it belongs to
/// the latest request, targets the visible page, and is not a downgrade.
pub(crate) fn frame_is_current(
    latest: bool,
    page_matches: bool,
    dpi: u32,
    image_dpi: u32,
    image_present: bool,
) -> bool {
    latest && page_matches && (!image_present || dpi >= image_dpi)
}

/// Whether a cache holding `len` entries and `bytes` pixels must evict one
/// more entry. The visible page and the frame that was just produced are
/// protected by the caller.
pub(crate) fn cache_over_budget(
    len: usize,
    count_limit: usize,
    bytes: usize,
    byte_limit: usize,
) -> bool {
    len > count_limit || bytes > byte_limit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(seq: u64, page: usize, dpi: u32) -> PageRequest {
        PageRequest { seq, page, dpi }
    }

    #[test]
    fn only_the_latest_request_is_current() {
        let latest = request(7, 3, 216);
        assert!(request(7, 3, 216).is_latest(Some(latest)));
        assert!(!request(6, 3, 216).is_latest(Some(latest)));
        assert!(!request(8, 3, 216).is_latest(Some(latest)));
        assert!(!request(7, 4, 216).is_latest(Some(latest)));
        assert!(!request(7, 3, 144).is_latest(Some(latest)));
    }

    #[test]
    fn results_for_another_page_or_a_lower_density_do_not_replace_the_screen() {
        let image = request(7, 3, 168);
        assert!(image.replaces_visible(3, 168, true));
        assert!(image.replaces_visible(3, 168, false));
        assert!(!image.replaces_visible(4, 168, true));
        assert!(!request(7, 3, 144).replaces_visible(3, 168, true));
    }

    #[test]
    fn a_blank_page_accepts_the_first_frame_at_any_density() {
        assert!(request(9, 1, 96).replaces_visible(1, 384, false));
    }

    #[test]
    fn cache_evicts_on_either_the_count_or_the_byte_limit() {
        assert!(!cache_over_budget(8, 8, 100, 100));
        assert!(cache_over_budget(9, 8, 100, 100));
        assert!(cache_over_budget(8, 8, 101, 100));
    }
}
