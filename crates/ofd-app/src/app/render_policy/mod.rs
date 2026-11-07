//! Pure render scheduling decisions.
//!
//! Keeping these out of the GPUI closures makes the "which background result
//! may touch the screen" rules testable without a window.

mod acceptance;
mod admission;

pub(crate) use acceptance::*;
pub(crate) use admission::*;

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

/// One neighbour prefetch candidate: the page, its density and the resident
/// pixel cost of showing it at that density.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrefetchCandidate {
    pub(crate) page: usize,
    pub(crate) dpi: u32,
    pub(crate) bytes: usize,
}

/// Order neighbour pages by scroll direction, then distance.
///
/// Pages ahead of the scroll direction come first; a page behind counts as
/// twice as far because it was usually just on screen and is still cached.
pub(crate) fn order_prefetch_pages(
    pages: impl IntoIterator<Item = usize>,
    current: usize,
    scroll_down: bool,
) -> Vec<usize> {
    let mut pages: Vec<_> = pages.into_iter().collect();
    pages.sort_by_key(|page| {
        let ahead = (*page > current) == scroll_down;
        (page.abs_diff(current) * if ahead { 1 } else { 2 }, !ahead)
    });
    pages
}

/// Visible pages are always requested; neighbours only while `room` bytes and
/// `slots` entries remain after the visible working set is reserved.
pub(crate) fn prefetch_plan(
    visible_bytes: usize,
    slot_limit: usize,
    visible_count: usize,
    page_bytes: usize,
    candidates: &[PrefetchCandidate],
) -> Vec<PrefetchCandidate> {
    let room = page_bytes.saturating_sub(visible_bytes);
    let slots = slot_limit.saturating_sub(visible_count);
    let fit = crate::continuous::prefetch_fit(
        candidates.iter().map(|candidate| candidate.bytes),
        room,
        slots,
    );
    candidates[..fit].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_decisions_are_pure() {
        let request = PageRequest {
            seq: 1,
            page: 2,
            dpi: 96,
        };
        assert!(request.is_latest(Some(request)));
    }

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
    fn prefetch_order_puts_pages_ahead_first() {
        let order = order_prefetch_pages([3, 1, 5, 2], 4, true);
        assert_eq!(order, vec![5, 3, 2, 1]);
        let order = order_prefetch_pages([3, 1, 5, 2], 4, false);
        assert_eq!(order, vec![3, 2, 5, 1]);
    }

    #[test]
    fn prefetch_plan_respects_room_and_slots() {
        let candidates = [
            PrefetchCandidate {
                page: 1,
                dpi: 96,
                bytes: 10,
            },
            PrefetchCandidate {
                page: 2,
                dpi: 96,
                bytes: 10,
            },
            PrefetchCandidate {
                page: 3,
                dpi: 96,
                bytes: 10,
            },
        ];
        // Only 15 bytes and one slot remain after the visible pages.
        let chosen = prefetch_plan(85, 8, 7, 100, &candidates);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].page, 1);
        // No room at all.
        assert!(prefetch_plan(100, 8, 7, 100, &candidates).is_empty());
    }

    #[test]
    fn cache_evicts_on_either_the_count_or_the_byte_limit() {
        assert!(!cache_over_budget(8, 8, 100, 100));
        assert!(cache_over_budget(9, 8, 100, 100));
        assert!(cache_over_budget(8, 8, 101, 100));
    }
}
