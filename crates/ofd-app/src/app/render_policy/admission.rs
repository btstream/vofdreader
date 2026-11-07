//! Which background raster work may start right now.
//!
//! These functions only read numbers describing the reader; they never touch
//! a receiver, a timer or a window. Keeping the rules here means the
//! rendering modules can be reduced to "ask, then start".

/// What the single-page path should do for the visible page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SinglePagePlan {
    /// A cached bitmap at least as sharp as the target is already available.
    UseCached,
    /// Wait: a gesture, another job or a pending upgrade owns the slots.
    Wait,
    /// This exact request is already in flight.
    Coalesced,
    /// Start a rasterization at this density.
    Render { dpi: u32 },
}

/// Inputs for the single-page decision, in the order the checks apply.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SinglePageInput {
    pub(crate) target_dpi: u32,
    pub(crate) has_cached_at_least: bool,
    pub(crate) gesture_active: bool,
    pub(crate) other_jobs_active: bool,
    pub(crate) pending_request: Option<(usize, u32)>,
    pub(crate) page: usize,
}

pub(crate) fn plan_single_page(input: SinglePageInput) -> SinglePagePlan {
    if input.has_cached_at_least {
        return SinglePagePlan::UseCached;
    }
    if input.gesture_active || input.other_jobs_active {
        return SinglePagePlan::Wait;
    }
    if input
        .pending_request
        .is_some_and(|(page, dpi)| page == input.page && dpi == input.target_dpi)
    {
        return SinglePagePlan::Coalesced;
    }
    SinglePagePlan::Render {
        dpi: input.target_dpi,
    }
}

/// Whether a cached-page job may start. Every early return here is a rule the
/// caller used to encode inline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CachedPageInput {
    pub(crate) page: usize,
    pub(crate) page_count: usize,
    pub(crate) raster_jobs: usize,
    pub(crate) slots: usize,
    pub(crate) single_page_job_active: bool,
    /// A cached bitmap at least as sharp as the requested density exists.
    pub(crate) cached_sharp_enough: bool,
    pub(crate) already_failed: bool,
    pub(crate) already_in_flight: bool,
}

pub(crate) fn may_request_cached_page(input: CachedPageInput) -> bool {
    input.page < input.page_count
        && input.raster_jobs < input.slots.max(1)
        && !input.single_page_job_active
        && !input.cached_sharp_enough
        && !input.already_failed
        && !input.already_in_flight
}

/// Whether speculative neighbour or tile prefetch may run at all.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PrefetchGate {
    pub(crate) document_open: bool,
    pub(crate) renderer_ready: bool,
    pub(crate) grid_open: bool,
    pub(crate) single_page_scrolling: bool,
    pub(crate) dpi_pending: bool,
    pub(crate) single_page_job_active: bool,
    pub(crate) page_loading: bool,
}

pub(crate) fn may_schedule_prefetch(gate: PrefetchGate) -> bool {
    gate.document_open
        && gate.renderer_ready
        && !gate.grid_open
        && !gate.single_page_scrolling
        && !gate.dpi_pending
        && !gate.single_page_job_active
        && !gate.page_loading
}

/// Whether a tile rasterization may start.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TileGate {
    pub(crate) single_page_scrolling: bool,
    pub(crate) dpi_pending: bool,
    pub(crate) single_page_job_active: bool,
}

pub(crate) fn may_start_tile(gate: TileGate) -> bool {
    !(gate.single_page_scrolling || gate.dpi_pending || gate.single_page_job_active)
}

/// Whether a finished tile should be kept: still wanted and inside budget.
pub(crate) fn may_keep_tile(
    still_wanted: bool,
    protected_bytes: usize,
    in_flight_bytes: usize,
    new_bytes: usize,
    budget: usize,
) -> bool {
    still_wanted && protected_bytes + in_flight_bytes + new_bytes <= budget
}

/// Whether a thumbnail batch may render its next page: same batch, and no
/// foreground raster work is running.
pub(crate) fn may_render_thumbnail(
    batch_current: bool,
    scrolling: bool,
    page_jobs: bool,
    tile_jobs: bool,
    single_page_job_active: bool,
) -> bool {
    batch_current && !scrolling && !page_jobs && !tile_jobs && !single_page_job_active
}

/// Which cached density may stand in while the sharp frame is missing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PreviewSource {
    None,
    Cached(u32),
}

/// Sources of an on-screen preview while the sharp frame is missing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreviewInput {
    /// Highest cached density for the page, if any.
    pub(crate) best_cached_dpi: Option<u32>,
    /// No bitmap is displayed yet.
    pub(crate) image_missing: bool,
}

/// Which cached density to show as a stand-in, if any.
pub(crate) fn preview_density(input: PreviewInput) -> PreviewSource {
    match (input.image_missing, input.best_cached_dpi) {
        (true, Some(dpi)) => PreviewSource::Cached(dpi),
        _ => PreviewSource::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(dpi: u32) -> SinglePageInput {
        SinglePageInput {
            target_dpi: dpi,
            has_cached_at_least: false,
            gesture_active: false,
            other_jobs_active: false,
            pending_request: None,
            page: 2,
        }
    }

    #[test]
    fn cached_bitmap_wins_over_starting_a_render() {
        let plan = plan_single_page(SinglePageInput {
            has_cached_at_least: true,
            ..single(192)
        });
        assert_eq!(plan, SinglePagePlan::UseCached);
    }

    #[test]
    fn gestures_and_other_jobs_pause_single_page_renders() {
        assert_eq!(
            plan_single_page(SinglePageInput {
                gesture_active: true,
                ..single(192)
            }),
            SinglePagePlan::Wait
        );
        assert_eq!(
            plan_single_page(SinglePageInput {
                other_jobs_active: true,
                ..single(192)
            }),
            SinglePagePlan::Wait
        );
    }

    #[test]
    fn an_identical_request_is_coalesced_but_a_different_density_is_not() {
        assert_eq!(
            plan_single_page(SinglePageInput {
                pending_request: Some((2, 192)),
                ..single(192)
            }),
            SinglePagePlan::Coalesced
        );
        assert_eq!(
            plan_single_page(SinglePageInput {
                pending_request: Some((2, 144)),
                ..single(192)
            }),
            SinglePagePlan::Render { dpi: 192 }
        );
        assert_eq!(
            plan_single_page(SinglePageInput {
                pending_request: Some((3, 192)),
                ..single(192)
            }),
            SinglePagePlan::Render { dpi: 192 }
        );
    }

    fn cached() -> CachedPageInput {
        CachedPageInput {
            page: 1,
            page_count: 10,
            raster_jobs: 0,
            slots: 2,
            single_page_job_active: false,
            cached_sharp_enough: false,
            already_failed: false,
            already_in_flight: false,
        }
    }

    #[test]
    fn cached_page_admission_covers_every_early_return() {
        assert!(may_request_cached_page(cached()));

        assert!(!may_request_cached_page(CachedPageInput {
            page: 10,
            ..cached()
        }));
        assert!(!may_request_cached_page(CachedPageInput {
            raster_jobs: 2,
            ..cached()
        }));
        assert!(!may_request_cached_page(CachedPageInput {
            single_page_job_active: true,
            ..cached()
        }));
        assert!(!may_request_cached_page(CachedPageInput {
            cached_sharp_enough: true,
            ..cached()
        }));
        assert!(!may_request_cached_page(CachedPageInput {
            already_failed: true,
            ..cached()
        }));
        assert!(!may_request_cached_page(CachedPageInput {
            already_in_flight: true,
            ..cached()
        }));
    }

    #[test]
    fn prefetch_requires_an_idle_foreground() {
        let idle = PrefetchGate {
            document_open: true,
            renderer_ready: true,
            grid_open: false,
            single_page_scrolling: false,
            dpi_pending: false,
            single_page_job_active: false,
            page_loading: false,
        };
        assert!(may_schedule_prefetch(idle));
        for blocked in [
            PrefetchGate {
                document_open: false,
                ..idle
            },
            PrefetchGate {
                renderer_ready: false,
                ..idle
            },
            PrefetchGate {
                grid_open: true,
                ..idle
            },
            PrefetchGate {
                single_page_scrolling: true,
                ..idle
            },
            PrefetchGate {
                dpi_pending: true,
                ..idle
            },
            PrefetchGate {
                single_page_job_active: true,
                ..idle
            },
            PrefetchGate {
                page_loading: true,
                ..idle
            },
        ] {
            assert!(!may_schedule_prefetch(blocked));
        }
    }

    #[test]
    fn tiles_pause_during_single_page_gestures_and_upgrades() {
        let idle = TileGate {
            single_page_scrolling: false,
            dpi_pending: false,
            single_page_job_active: false,
        };
        assert!(may_start_tile(idle));
        assert!(!may_start_tile(TileGate {
            single_page_scrolling: true,
            ..idle
        }));
        assert!(!may_start_tile(TileGate {
            dpi_pending: true,
            ..idle
        }));
        assert!(!may_start_tile(TileGate {
            single_page_job_active: true,
            ..idle
        }));
    }

    #[test]
    fn tile_budget_rejects_an_oversized_result() {
        assert!(may_keep_tile(true, 10, 5, 5, 20));
        assert!(!may_keep_tile(true, 10, 5, 6, 20));
        assert!(!may_keep_tile(false, 0, 0, 1, 20));
    }

    #[test]
    fn thumbnails_wait_for_foreground_work() {
        assert!(may_render_thumbnail(true, false, false, false, false));
        assert!(!may_render_thumbnail(false, false, false, false, false));
        assert!(!may_render_thumbnail(true, true, false, false, false));
        assert!(!may_render_thumbnail(true, false, true, false, false));
        assert!(!may_render_thumbnail(true, false, false, true, false));
        assert!(!may_render_thumbnail(true, false, false, false, true));
    }

    #[test]
    fn preview_only_fills_a_missing_bitmap() {
        assert_eq!(
            preview_density(PreviewInput {
                best_cached_dpi: Some(96),
                image_missing: true,
            }),
            PreviewSource::Cached(96)
        );
        assert_eq!(
            preview_density(PreviewInput {
                best_cached_dpi: Some(96),
                image_missing: false,
            }),
            PreviewSource::None
        );
        assert_eq!(
            preview_density(PreviewInput {
                best_cached_dpi: None,
                image_missing: true,
            }),
            PreviewSource::None
        );
    }
}
