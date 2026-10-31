//! Reference counting for the values that outlive their region.

use crate::Span;
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RcStats {
    /// Updates that return a compound argument with one element changed.
    pub updates: u64,
    pub updates_in_place: u64,
    /// Slots copied by the updates that did not rewrite in place.
    pub elements_copied: u64,
}

impl RcStats {
    pub fn in_place(&self) -> Option<f64> {
        if self.updates == 0 {
            return None;
        }
        Some(self.updates_in_place as f64 / self.updates as f64)
    }
}

thread_local! {
    static COUNTERS: RefCell<RcStats> = const { RefCell::new(RcStats {
        updates: 0,
        updates_in_place: 0,
        elements_copied: 0,
    }) };
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<FxHashMap<Span, SiteCount>> = RefCell::new(FxHashMap::default());
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SiteCount {
    pub in_place: u64,
    pub copies: u64,
}

impl SiteCount {
    pub fn total(&self) -> u64 {
        self.in_place + self.copies
    }

    /// `None` when the site never ran, which is not zero.
    pub fn rate(&self) -> Option<f64> {
        if self.total() == 0 {
            return None;
        }
        Some(self.in_place as f64 / self.total() as f64)
    }
}

/// Arms or disarms per-site attribution of [`Stats::updates`], clearing the map either way.
pub fn record_sites(on: bool) {
    let _ = RECORDING.try_with(|c| c.set(on));
    let _ = SITES.try_with(|c| c.borrow_mut().clear());
}

pub fn sites() -> Vec<(Span, SiteCount)> {
    SITES
        .try_with(|c| c.borrow().iter().map(|(k, v)| (*k, *v)).collect())
        .unwrap_or_default()
}

fn bump(f: impl FnOnce(&mut RcStats)) {
    let _ = COUNTERS.try_with(|c| f(&mut c.borrow_mut()));
}

/// What this thread has counted since the last [`reset`].
pub fn stats() -> RcStats {
    COUNTERS.try_with(|c| *c.borrow()).unwrap_or_default()
}

pub fn reset() {
    let _ = COUNTERS.try_with(|c| *c.borrow_mut() = RcStats::default());
    let _ = SITES.try_with(|c| c.borrow_mut().clear());
}

pub fn note_update_of(in_place: bool, copied: usize, span: Span) {
    bump(|s| {
        s.updates += 1;
        s.updates_in_place += u64::from(in_place);
        s.elements_copied += copied as u64;
    });
    if RECORDING.try_with(Cell::get).unwrap_or(false) {
        let _ = SITES.try_with(|c| {
            let mut map = c.borrow_mut();
            let entry = map.entry(span).or_default();
            if in_place {
                entry.in_place += 1;
            } else {
                entry.copies += 1;
            }
        });
    }
}
