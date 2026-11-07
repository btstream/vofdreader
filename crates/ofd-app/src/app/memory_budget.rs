//! Accounting for the bitmap memory the reader can actually control.
//!
//! This is not a process-RSS limit. It tracks the pixels of page and tile
//! bitmaps that the reader allocates, keeps on screen, still renders, or
//! briefly holds while swapping one frame for another. Fonts, parsed pages,
//! decoded images and GPU textures are outside it.
//!
//! The ledger has two parts:
//!
//! - **Resident**: bitmaps reachable from a cache, the on-screen slot or the
//!   registry waiting for a window frame to retire it.
//! - **Reserved**: pixels a running job will produce. Reserved before the
//!   work starts and released when the result is discarded or admitted. A
//!   completed foreground replacement keeps its reservation until the frame.

use std::collections::HashMap;

/// Caller-chosen limits. Values come from the user's cache setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct BudgetLimits {
    /// Pixels the reader may keep in the page/tile caches and on screen.
    pub(crate) resident_bytes: usize,
    /// Transient pixels background jobs may reserve before they start.
    pub(crate) reserved_bytes: usize,
}

impl BudgetLimits {
    /// Splits the cache setting into the pools the reader accounts for.
    ///
    /// The setting bounds **retained** bitmaps: page cache, tile cache and the
    /// on-screen frame. Running jobs may reserve a further quarter of the
    /// setting so work can start before its pixels exist; that in-flight
    /// allowance is transient and is released when the job returns.
    ///
    /// These are accounting limits, not a bound on allocation peaks. Existing
    /// owners and reservations can exceed newly lowered limits until retired.
    pub(crate) fn split(total_bytes: usize) -> Self {
        Self {
            resident_bytes: total_bytes,
            reserved_bytes: total_bytes / 4,
        }
    }
}

/// Stable identity of a resident bitmap. The same allocation can appear in a
/// cache, on screen, and in the image registry; charging it once is the whole
/// point of tracking by id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct BitmapId(pub(crate) u64);

/// Why the ledger refused an action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BudgetRefusal {
    /// Not enough space in the resident pool.
    NoResidentRoom,
    /// Not enough space in the reservation pool.
    NoReserveRoom,
    /// A single allocation is larger than its whole pool.
    ExceedsPool,
}

/// A reservation held while a job runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Reservation {
    id: u64,
    bytes: usize,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Usage {
    pub(crate) resident_bytes: usize,
    pub(crate) reserved_bytes: usize,
}

/// Tracks two pools. Every mutation goes through a method so the totals
/// cannot drift from the entries that justify them.
#[derive(Default)]
pub(crate) struct BudgetLedger {
    limits: BudgetLimits,
    next_reservation: u64,
    resident: HashMap<BitmapId, usize>,
    reservations: HashMap<u64, usize>,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self::split(256 * 1024 * 1024)
    }
}

impl BudgetLedger {
    pub(crate) fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            next_reservation: 0,
            resident: HashMap::new(),
            reservations: HashMap::new(),
        }
    }

    /// Applies new limits. Existing entries are kept; callers query
    /// [`Self::usage`] to find out what must be released to converge.
    pub(crate) fn set_limits(&mut self, limits: BudgetLimits) {
        self.limits = limits;
    }

    /// Whether the retained bitmaps are above the current resident limit.
    pub(crate) fn over_resident_limit(&self) -> bool {
        self.usage().resident_bytes > self.limits.resident_bytes
    }

    pub(crate) fn usage(&self) -> Usage {
        Usage {
            resident_bytes: self
                .resident
                .values()
                .fold(0usize, |a, b| a.saturating_add(*b)),
            reserved_bytes: self
                .reservations
                .values()
                .fold(0usize, |a, b| a.saturating_add(*b)),
        }
    }

    /// Whether a bitmap of `bytes` fits the resident pool. The same id is
    /// free: it is already counted and is being replaced in place.
    pub(crate) fn can_admit(&self, id: BitmapId, bytes: usize) -> Result<(), BudgetRefusal> {
        if bytes > self.limits.resident_bytes {
            return Err(BudgetRefusal::ExceedsPool);
        }
        let others = self
            .usage()
            .resident_bytes
            .saturating_sub(self.resident.get(&id).copied().unwrap_or(0));
        if bytes <= self.limits.resident_bytes.saturating_sub(others) {
            Ok(())
        } else {
            Err(BudgetRefusal::NoResidentRoom)
        }
    }

    /// Records a bitmap as resident. Returns `false` when it does not fit; the
    /// caller must then drop or downscale the bitmap rather than keep it.
    pub(crate) fn admit(&mut self, id: BitmapId, bytes: usize) -> bool {
        if self.can_admit(id, bytes).is_err() {
            return false;
        }
        self.resident.insert(id, bytes);
        true
    }

    /// Forgets a resident bitmap. If another owner still holds the pixels the
    /// real memory is not freed, which is why this only reflects the reader's
    /// accounted set.
    pub(crate) fn release(&mut self, id: BitmapId) {
        self.resident.remove(&id);
    }

    pub(crate) fn contains(&self, id: BitmapId) -> bool {
        self.resident.contains_key(&id)
    }

    pub(crate) fn fits_resident_capacity(&self, bytes: usize) -> bool {
        bytes <= self.limits.resident_bytes
    }

    /// Reserves pixels before a job starts. Fails when the reservation pool
    /// would be exceeded; a single reservation larger than the pool is also
    /// refused, so an oversized page reports that immediately instead of
    /// failing after a full render.
    pub(crate) fn can_reserve(&self, bytes: usize) -> Result<(), BudgetRefusal> {
        self.can_ever_reserve(bytes)?;
        if bytes
            > self
                .limits
                .reserved_bytes
                .saturating_sub(self.usage().reserved_bytes)
        {
            return Err(BudgetRefusal::NoReserveRoom);
        }
        Ok(())
    }

    /// Permanent output capacity, independent of temporary job contention.
    pub(crate) fn can_ever_reserve(&self, bytes: usize) -> Result<(), BudgetRefusal> {
        if bytes > self.limits.reserved_bytes.min(self.limits.resident_bytes) {
            Err(BudgetRefusal::ExceedsPool)
        } else {
            Ok(())
        }
    }

    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<Reservation, BudgetRefusal> {
        self.can_reserve(bytes)?;
        self.next_reservation = self.next_reservation.wrapping_add(1);
        let id = self.next_reservation;
        self.reservations.insert(id, bytes);
        Ok(Reservation { id, bytes })
    }

    /// Gives back a reservation that produced nothing.
    pub(crate) fn cancel(&mut self, reservation: Reservation) {
        self.reservations.remove(&reservation.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> BitmapId {
        BitmapId(n)
    }

    fn ledger() -> BudgetLedger {
        BudgetLedger::new(BudgetLimits {
            resident_bytes: 100,
            reserved_bytes: 20,
        })
    }

    #[test]
    fn admitting_the_same_bitmap_twice_charges_it_once() {
        let mut ledger = ledger();
        assert!(ledger.admit(id(1), 40));
        assert!(ledger.admit(id(1), 40));
        assert_eq!(ledger.usage().resident_bytes, 40);
    }

    #[test]
    fn the_resident_pool_is_a_real_limit() {
        let mut ledger = ledger();
        assert!(ledger.admit(id(1), 60));
        assert!(!ledger.admit(id(2), 50));
        assert_eq!(ledger.usage().resident_bytes, 60);
        ledger.release(id(1));
        assert!(ledger.admit(id(2), 50));
    }

    #[test]
    fn a_single_allocation_larger_than_the_pool_is_refused() {
        let mut ledger = ledger();
        assert_eq!(
            ledger.can_admit(id(1), 101),
            Err(BudgetRefusal::ExceedsPool)
        );
        assert_eq!(ledger.reserve(21), Err(BudgetRefusal::ExceedsPool));
    }

    #[test]
    fn reservations_are_bounded_and_released_exactly_once() {
        let mut ledger = ledger();
        let first = ledger.reserve(10).unwrap();
        assert_eq!(ledger.usage().reserved_bytes, 10);
        assert_eq!(
            ledger.reserve(15),
            Err(BudgetRefusal::NoReserveRoom),
            "the reservation pool is 20"
        );
        ledger.cancel(first);
        assert_eq!(ledger.usage().reserved_bytes, 0);
        ledger.cancel(first);
        assert_eq!(ledger.usage().reserved_bytes, 0, "double cancel is a no-op");
    }

    #[test]
    fn completing_and_admitting_are_separate_operations() {
        let mut ledger = ledger();
        let reservation = ledger.reserve(20).unwrap();
        ledger.cancel(reservation);
        assert_eq!(ledger.usage().resident_bytes, 0);
        assert!(ledger.admit(id(1), 20));
        assert_eq!(ledger.usage().reserved_bytes, 0);
        assert_eq!(ledger.usage().resident_bytes, 20);
    }

    #[test]
    fn a_finished_frame_that_does_not_fit_is_not_kept() {
        let mut ledger = ledger();
        assert!(ledger.admit(id(1), 90));
        let reservation = ledger.reserve(10).unwrap();
        ledger.cancel(reservation);
        assert!(!ledger.admit(id(2), 20));
        assert_eq!(ledger.usage().resident_bytes, 90);
        // The reservation is still released even when the pixels are dropped.
        assert_eq!(ledger.usage().reserved_bytes, 0);
    }

    #[test]
    fn lowering_the_limit_preserves_entries_and_reports_the_overage() {
        let mut ledger = ledger();
        assert!(ledger.admit(id(1), 80));
        ledger.set_limits(BudgetLimits {
            resident_bytes: 40,
            reserved_bytes: 0,
        });
        assert_eq!(ledger.usage().resident_bytes, 80);
        assert!(
            ledger.can_admit(id(2), 1).is_err(),
            "no room while the old frame is still resident"
        );
        ledger.release(id(1));
        assert!(ledger.admit(id(2), 40));
    }

    #[test]
    fn split_keeps_the_setting_as_the_retained_limit() {
        let limits = BudgetLimits::split(1024);
        assert_eq!(limits.resident_bytes, 1024);
        assert_eq!(limits.reserved_bytes, 256);
    }

    #[test]
    fn a_reservation_larger_than_the_pool_is_refused_up_front() {
        let mut ledger = ledger();
        assert_eq!(ledger.reserve(21), Err(BudgetRefusal::ExceedsPool));
    }

    #[test]
    fn arithmetic_cannot_wrap_into_admission() {
        let mut ledger = BudgetLedger::new(BudgetLimits {
            resident_bytes: usize::MAX,
            reserved_bytes: usize::MAX,
        });
        assert!(ledger.admit(id(1), usize::MAX));
        assert!(!ledger.admit(id(2), 1));
        let reservation = ledger.reserve(usize::MAX).unwrap();
        assert_eq!(ledger.reserve(1), Err(BudgetRefusal::NoReserveRoom));
        ledger.cancel(reservation);
    }

    #[test]
    fn reusing_an_id_cannot_hide_a_larger_charge() {
        let mut ledger = ledger();
        assert!(ledger.admit(id(1), 40));
        assert!(ledger.admit(id(2), 60));
        assert!(!ledger.admit(id(1), 50));
        assert_eq!(ledger.usage().resident_bytes, 100);
    }
}
