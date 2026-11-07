//! What a finished raster job is allowed to do.
//!
//! Completion used to mix three questions in a single `bool`: is this result
//! still current, is it worth caching, and may it replace the screen. They are
//! separate: a stale frame can still release its slot, a valid frame for an
//! off-screen page belongs in the cache, and only the latest request for the
//! visible page may change what the user sees.

use crate::app::task_state::RenderEpoch;

/// The job's place in the document and view timeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Currency {
    /// Same document and view generation.
    Current,
    /// The document, rotation or sampling changed while it rendered.
    Stale,
}

/// Whether a result may enter the cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Retention {
    Keep,
    Drop,
}

/// Whether a result may replace the bitmap on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Display {
    Replace,
    KeepExisting,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Acceptance {
    pub(crate) currency: Currency,
    pub(crate) retention: Retention,
    pub(crate) display: Display,
}

impl Acceptance {
    pub(crate) fn is_current(self) -> bool {
        self.currency == Currency::Current
    }

    /// Only a current, retained frame may take over the screen.
    pub(crate) fn replaces_screen(self) -> bool {
        self.is_current() && self.retention == Retention::Keep && self.display == Display::Replace
    }
}

/// Inputs for a finished whole-page frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageAcceptanceInput {
    pub(crate) job: RenderEpoch,
    pub(crate) current: RenderEpoch,
    /// Whether this request is still the newest one for the visible page.
    pub(crate) latest_request: bool,
    /// The page the user is looking at now.
    pub(crate) visible_page: usize,
    pub(crate) result_page: usize,
    /// No bitmap is displayed yet, so any density is better than nothing.
    pub(crate) image_missing: bool,
    pub(crate) result_dpi: u32,
    pub(crate) displayed_dpi: u32,
    /// The frame is inside the visible or prefetch horizon.
    pub(crate) in_horizon: bool,
}

pub(crate) fn accept_page_frame(input: PageAcceptanceInput) -> Acceptance {
    let currency = if input.job.matches(input.current) {
        Currency::Current
    } else {
        Currency::Stale
    };
    // A stale frame is never useful: it was rendered for the previous
    // document, rotation or sampling. Frames outside the horizon are correct
    // but not worth keeping.
    let retention = if currency == Currency::Current && input.in_horizon {
        Retention::Keep
    } else {
        Retention::Drop
    };
    let fresh_enough = input.latest_request
        && input.result_page == input.visible_page
        && (input.image_missing || input.result_dpi >= input.displayed_dpi);
    let display = if retention == Retention::Keep && fresh_enough {
        Display::Replace
    } else {
        Display::KeepExisting
    };
    Acceptance {
        currency,
        retention,
        display,
    }
}

/// Inputs for a finished tile.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TileAcceptanceInput {
    pub(crate) job: RenderEpoch,
    pub(crate) current: RenderEpoch,
    /// The tile is still part of the current request plan.
    pub(crate) still_wanted: bool,
    /// The cache accepted the pixels within its byte budget.
    pub(crate) within_budget: bool,
}

pub(crate) fn accept_tile(input: TileAcceptanceInput) -> Acceptance {
    let currency = if input.job.matches(input.current) {
        Currency::Current
    } else {
        Currency::Stale
    };
    let retention = if currency == Currency::Current && input.still_wanted && input.within_budget {
        Retention::Keep
    } else {
        Retention::Drop
    };
    Acceptance {
        currency,
        retention,
        // Tiles never replace the whole-page bitmap directly.
        display: Display::KeepExisting,
    }
}

/// Inputs for a finished thumbnail.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThumbnailAcceptanceInput {
    /// The batch that produced it is still the newest one.
    pub(crate) batch_current: bool,
    pub(crate) page_in_range: bool,
}

pub(crate) fn accept_thumbnail(input: ThumbnailAcceptanceInput) -> Acceptance {
    let retention = if input.batch_current && input.page_in_range {
        Retention::Keep
    } else {
        Retention::Drop
    };
    Acceptance {
        currency: if input.batch_current {
            Currency::Current
        } else {
            Currency::Stale
        },
        retention,
        display: Display::KeepExisting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(document: u64, view: u64) -> RenderEpoch {
        RenderEpoch { document, view }
    }

    fn page() -> PageAcceptanceInput {
        PageAcceptanceInput {
            job: epoch(1, 1),
            current: epoch(1, 1),
            latest_request: true,
            visible_page: 2,
            result_page: 2,
            image_missing: false,
            result_dpi: 192,
            displayed_dpi: 144,
            in_horizon: true,
        }
    }

    #[test]
    fn a_current_latest_frame_replaces_the_screen() {
        let acceptance = accept_page_frame(page());
        assert_eq!(acceptance.currency, Currency::Current);
        assert_eq!(acceptance.retention, Retention::Keep);
        assert!(acceptance.replaces_screen());
    }

    #[test]
    fn a_stale_frame_is_neither_cached_nor_displayed() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            job: epoch(1, 1),
            current: epoch(2, 1),
            ..page()
        });
        assert_eq!(acceptance.currency, Currency::Stale);
        assert_eq!(acceptance.retention, Retention::Drop);
        assert!(!acceptance.replaces_screen());
    }

    #[test]
    fn a_rotation_change_invalidates_the_frame() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            job: epoch(1, 1),
            current: epoch(1, 2),
            ..page()
        });
        assert_eq!(acceptance.currency, Currency::Stale);
        assert!(!acceptance.replaces_screen());
    }

    #[test]
    fn a_superseded_frame_is_cached_but_does_not_replace_the_screen() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            latest_request: false,
            ..page()
        });
        assert_eq!(acceptance.currency, Currency::Current);
        assert_eq!(acceptance.retention, Retention::Keep);
        assert!(!acceptance.replaces_screen());
    }

    #[test]
    fn a_frame_for_an_offscreen_page_is_cached_only() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            visible_page: 5,
            ..page()
        });
        assert_eq!(acceptance.retention, Retention::Keep);
        assert!(!acceptance.replaces_screen());
    }

    #[test]
    fn a_frame_outside_the_horizon_is_dropped_entirely() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            in_horizon: false,
            ..page()
        });
        assert_eq!(acceptance.retention, Retention::Drop);
        assert!(!acceptance.replaces_screen());
    }

    #[test]
    fn a_blank_page_accepts_a_lower_density_than_the_target() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            image_missing: true,
            result_dpi: 96,
            displayed_dpi: 384,
            ..page()
        });
        assert!(acceptance.replaces_screen());
    }

    #[test]
    fn a_lower_density_never_replaces_a_sharper_image() {
        let acceptance = accept_page_frame(PageAcceptanceInput {
            result_dpi: 144,
            displayed_dpi: 192,
            ..page()
        });
        assert!(!acceptance.replaces_screen());
    }

    #[test]
    fn stale_errors_must_not_overwrite_current_state() {
        // The error path asks the same currency question; a stale failure has
        // no retention and no display effect.
        let acceptance = accept_page_frame(PageAcceptanceInput {
            job: epoch(3, 1),
            current: epoch(4, 1),
            ..page()
        });
        assert!(!acceptance.is_current());
        assert_eq!(acceptance.retention, Retention::Drop);
    }

    fn tile() -> TileAcceptanceInput {
        TileAcceptanceInput {
            job: epoch(1, 1),
            current: epoch(1, 1),
            still_wanted: true,
            within_budget: true,
        }
    }

    #[test]
    fn a_tile_is_kept_only_when_current_wanted_and_affordable() {
        let acceptance = accept_tile(tile());
        assert_eq!(acceptance.retention, Retention::Keep);
        assert_eq!(acceptance.display, Display::KeepExisting);

        for rejected in [
            TileAcceptanceInput {
                current: epoch(2, 1),
                ..tile()
            },
            TileAcceptanceInput {
                still_wanted: false,
                ..tile()
            },
            TileAcceptanceInput {
                within_budget: false,
                ..tile()
            },
        ] {
            assert_eq!(accept_tile(rejected).retention, Retention::Drop);
        }
    }

    #[test]
    fn an_older_thumbnail_batch_is_dropped() {
        assert_eq!(
            accept_thumbnail(ThumbnailAcceptanceInput {
                batch_current: false,
                page_in_range: true,
            })
            .retention,
            Retention::Drop
        );
        assert_eq!(
            accept_thumbnail(ThumbnailAcceptanceInput {
                batch_current: true,
                page_in_range: true,
            })
            .retention,
            Retention::Keep
        );
    }
}
