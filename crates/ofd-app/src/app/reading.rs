//! Reading position shared by the live reader and inactive tab snapshots.

use crate::*;

/// Page layout mode. `Continuous` is the default for new windows.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum ViewMode {
    Single,
    #[default]
    Continuous,
}

/// The part of a tab that must survive releasing its document. Keeping the
/// live reader and `DocumentTab` on one type means a new field cannot be
/// added to only one side of the save/restore path.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ReadingSnapshot {
    pub page: usize,
    pub zoom: f32,
    pub rotation: u8,
    pub view_mode: ViewMode,
    /// Vertical position inside `page`, as a 0..1 fraction.
    pub page_offset: f32,
}

impl Default for ReadingSnapshot {
    fn default() -> Self {
        Self {
            page: 0,
            zoom: 1.0,
            rotation: 0,
            view_mode: ViewMode::Continuous,
            page_offset: 0.0,
        }
    }
}

impl ReaderApp {
    /// Reading state to store in the active tab before its document is
    /// released. `page_offset` needs the continuous layout, so it is computed
    /// here rather than read from a field that does not exist in single-page
    /// mode.
    pub(crate) fn reading_snapshot(&self) -> ReadingSnapshot {
        ReadingSnapshot {
            page: self.page,
            zoom: self.zoom,
            rotation: self.rotation,
            view_mode: self.view_mode,
            page_offset: self.current_page_offset(),
        }
    }

    /// Restores the live reading fields from a snapshot. The page itself is
    /// applied by `load_document`, which also clamps it to the page count.
    pub(crate) fn apply_reading(&mut self, reading: &ReadingSnapshot) {
        self.zoom = reading.zoom.max(0.1);
        self.rotation = reading.rotation % 4;
        self.view_mode = reading.view_mode;
        self.pending_restore_scroll = (reading.page_offset > 0.0).then_some(reading.page_offset);
    }
}
