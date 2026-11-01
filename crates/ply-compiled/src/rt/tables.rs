//! A unit's tables, which every thread running the unit reads: its memo and its roots.

use super::*;

/// A lock nothing poisons: a panic while one is held is already the end of the run.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A compiled function: `extern "C" fn(ctx, args) -> handle`.
pub type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;

/// A unit's tables, which every thread running the unit reads: what a `parallel` branch may change
/// is behind an atomic or a lock.
pub struct Tables {
    /// Every field name a compiled body reads, so a field access is an index.
    pub fields: Vec<Symbol>,
    /// Every compiled function's finalized address by index, for building native closures.
    pub functions: Vec<usize>,
    /// Each pure nullary function's memoized result by index, as an immortal word; `0` is none.
    pub memo: Box<[AtomicI64]>,
    /// Owns the memo's blocks for as long as the unit lives.
    pub immortals: Mutex<Heap>,
    /// Every root, sorted by `id`: what a body's stored site names.
    pub roots: Vec<Root>,
}

/// A root a body's sites name by `id`, the `root_id` of `name`.
pub struct Root {
    pub id: u64,
    pub name: Symbol,
    /// What its sites are placed against: the unit holds no source, so the span names the root by
    /// its index among the unit's roots, and a site is an offset from its definition's start.
    pub span: Span,
}

impl Tables {
    /// Remembers `kept`, an immortal copy emitted code made, as pure nullary function `index`'s
    /// result. The first kept stays; one another thread keeps after it stays in the unit's
    /// persistent heap, unread.
    pub fn keep(&self, index: usize, kept: Word) {
        if let Some(slot) = self.memo.get(index) {
            let _ = slot.compare_exchange(0, kept, AcqRel, Acquire);
        }
    }
}
