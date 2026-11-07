//! Identity and bookkeeping for background raster work.
//!
//! Every job gets a fresh [`TaskId`]; completion settles the job by id, never
//! by its target. A stale result can therefore release its own slot without
//! deleting a newer job that happens to want the same page, tile or
//! thumbnail. Deduplication is a separate question and asks whether the
//! target is already in flight.

use std::collections::HashMap;

use crate::tiles;

/// Generations that must still match for a result to be usable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct RenderEpoch {
    /// Bumped when the document is closed or replaced.
    pub(crate) document: u64,
    /// Bumped when caches and in-flight jobs are invalidated (rotation,
    /// image sampling, explicit invalidation).
    pub(crate) view: u64,
}

impl RenderEpoch {
    pub(crate) fn matches(&self, other: Self) -> bool {
        *self == other
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct TaskId(u64);

/// What a job produces. Cheap to copy and comparable, so it can serve both as
/// the deduplication key and as the identity carried into the cache.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TaskTarget {
    /// Whole-page bitmap at a density.
    Page { page: usize, dpi: u32 },
    /// One device-pixel tile.
    Tile { key: tiles::Key },
}

impl TaskTarget {
    pub(crate) fn page(page: usize, dpi: u32) -> Self {
        Self::Page { page, dpi }
    }

    pub(crate) fn tile(key: tiles::Key) -> Self {
        Self::Tile { key }
    }

    pub(crate) fn is_page(&self) -> bool {
        matches!(self, Self::Page { .. })
    }

    pub(crate) fn page_index(&self) -> Option<usize> {
        match self {
            Self::Page { page, .. } => Some(*page),
            Self::Tile { key } => Some(key.page),
        }
    }
}

/// Proof that this caller owns one in-flight slot. Finishing with a ticket
/// whose job was already forgotten (document switch, invalidation) is a
/// no-op rather than a removal of somebody else's work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct TaskTicket {
    id: TaskId,
    pub(crate) target: TaskTarget,
    pub(crate) epoch: RenderEpoch,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TaskInfo {
    pub(crate) target: TaskTarget,
    /// Resident bytes this job will produce; used for tile budgeting.
    pub(crate) bytes: usize,
    /// The view the job was started for. A job with an older epoch still
    /// occupies a worker slot and its reserved bytes, but its result is no
    /// longer usable and it must not block deduplication for the new view.
    pub(crate) epoch: RenderEpoch,
}

/// Jobs currently occupying a worker slot.
///
/// Entries are physical: they stay until the closure actually returns, even
/// after the document or view they were started for has been replaced. Only
/// [`Self::invalidate`] marks them stale; it never forgets them, because the
/// pixels and worker time are still in use.
#[derive(Default)]
pub(crate) struct InFlightTasks {
    next_id: u64,
    entries: HashMap<TaskId, TaskInfo>,
}

impl InFlightTasks {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Claims a slot for `target`. Callers deduplicate first with
    /// [`Self::contains`]; starting twice for one target is allowed so an
    /// invalidated job does not block a replacement.
    pub(crate) fn start(
        &mut self,
        target: TaskTarget,
        bytes: usize,
        epoch: RenderEpoch,
    ) -> TaskTicket {
        self.next_id = self.next_id.wrapping_add(1);
        let id = TaskId(self.next_id);
        self.entries.insert(
            id,
            TaskInfo {
                target,
                bytes,
                epoch,
            },
        );
        TaskTicket { id, target, epoch }
    }

    /// Releases the slot owned by `ticket`. Returns the job's description when
    /// this ticket was still active. A stale ticket returns `None`.
    pub(crate) fn finish(&mut self, ticket: TaskTicket) -> Option<TaskInfo> {
        let info = self.entries.remove(&ticket.id)?;
        Some(info)
    }

    /// Whether a usable job already targets this output in `epoch`. Stale
    /// jobs with the same target do not block a replacement.
    pub(crate) fn contains_current(&self, target: TaskTarget, epoch: RenderEpoch) -> bool {
        self.entries
            .values()
            .any(|info| info.target == target && info.epoch.matches(epoch))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Bytes reserved by tile jobs, for the tile cache budget. Includes stale
    /// jobs: their buffers still exist until the closure returns.
    pub(crate) fn tile_bytes(&self) -> usize {
        self.entries
            .values()
            .filter(|info| !info.target.is_page())
            .map(|info| info.bytes)
            .sum()
    }

    pub(crate) fn targets(&self) -> impl Iterator<Item = TaskTarget> + '_ {
        self.entries.values().map(|info| info.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32) -> TaskTarget {
        TaskTarget::tile(tiles::Key {
            page: 0,
            dpi: 96,
            x,
            y: 0,
        })
    }

    #[test]
    fn finishing_a_stale_ticket_leaves_the_replacement_alone() {
        let mut tasks = InFlightTasks::new();
        let target = TaskTarget::page(3, 192);
        let old_epoch = RenderEpoch {
            document: 1,
            view: 1,
        };
        let new_epoch = RenderEpoch {
            document: 2,
            view: 1,
        };
        let old = tasks.start(target, 0, old_epoch);
        // The old job is still running; a replacement for the new document
        // may start at the same target because deduplication is epoch-aware.
        assert!(!tasks.contains_current(target, new_epoch));
        let new = tasks.start(target, 0, new_epoch);

        assert!(tasks.finish(old).is_some());
        assert!(tasks.contains_current(target, new_epoch));

        assert!(tasks.finish(new).is_some());
        assert!(!tasks.contains_current(target, new_epoch));
    }

    #[test]
    fn a_stale_job_still_occupies_a_slot_until_it_returns() {
        let mut tasks = InFlightTasks::new();
        let old_epoch = RenderEpoch {
            document: 1,
            view: 1,
        };
        let new_epoch = RenderEpoch {
            document: 2,
            view: 1,
        };
        let old = tasks.start(TaskTarget::page(0, 96), 0, old_epoch);
        assert!(!tasks.contains_current(TaskTarget::page(0, 96), new_epoch));
        assert_eq!(tasks.len(), 1, "the running job still holds a slot");
        tasks.finish(old);
        assert!(tasks.is_empty());
    }

    #[test]
    fn deduplication_is_by_target_and_bytes_are_per_kind() {
        let mut tasks = InFlightTasks::new();
        assert!(!tasks.contains_current(tile(1), RenderEpoch::default()));
        let a = tasks.start(tile(1), 100, RenderEpoch::default());
        let b = tasks.start(TaskTarget::page(0, 96), 200, RenderEpoch::default());

        assert!(tasks.contains_current(tile(1), RenderEpoch::default()));
        assert!(!tasks.contains_current(tile(2), RenderEpoch::default()));
        assert_eq!(tasks.tile_bytes(), 100);
        assert_eq!(tasks.len(), 2);

        tasks.finish(a);
        assert_eq!(tasks.tile_bytes(), 0);
        assert_eq!(tasks.len(), 1);
        tasks.finish(b);
        assert!(tasks.is_empty());
    }

    #[test]
    fn finishing_twice_releases_the_slot_once() {
        let mut tasks = InFlightTasks::new();
        let ticket = tasks.start(tile(7), 10, RenderEpoch::default());
        assert!(tasks.finish(ticket).is_some());
        assert!(tasks.finish(ticket).is_none());
        assert!(tasks.is_empty());
    }

    #[test]
    fn epoch_comparison_covers_document_and_view() {
        let base = RenderEpoch {
            document: 1,
            view: 2,
        };
        assert!(base.matches(RenderEpoch {
            document: 1,
            view: 2
        }));
        assert!(!base.matches(RenderEpoch {
            document: 2,
            view: 2
        }));
        assert!(!base.matches(RenderEpoch {
            document: 1,
            view: 3
        }));
    }
}
