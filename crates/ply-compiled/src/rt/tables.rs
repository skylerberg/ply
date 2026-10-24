//! A unit's tables, which every thread running the unit reads: its memo, roots and the identities of
//! what it keeps.

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
    /// Every builtin a compiled body may call.
    pub builtins: Vec<Builtin>,
    /// Every compiled function's finalized address by index, for building native closures.
    pub functions: Vec<usize>,
    /// Each pure nullary function's memoized answer by index, as an immortal word; `0` is none.
    pub memo: Box<[AtomicI64]>,
    /// What computing each memoized answer cost, in calls, allocations and their bytes, stored
    /// before the answer is: a later read charges it, so a cost is the same whether or not it was
    /// memoized.
    pub memo_costs: Box<[(AtomicI64, AtomicI64, AtomicI64)]>,
    /// Owns the memo's blocks for as long as the unit lives.
    pub immortals: Mutex<Heap>,
    /// Every root, sorted by `id`: what a body's stored site names.
    pub roots: Vec<Root>,
}

/// A root a body's sites name by `id`, the `root_id` of `name`.
pub struct Root {
    pub id: u64,
    pub name: Symbol,
    /// Its definition's span in the text the unit runs over: a site is an offset from its start.
    /// Never cached: definitions move.
    pub span: Span,
}

/// How many calls of roots over memo words a unit remembers.
pub const CALL_MEMO_LIMIT: usize = 64;

/// How many of an answer's direct parts get identities of their own.
pub(super) const PARTS_LIMIT: usize = 64;

/// A handed-out value's allocation, plus a list's window onto it: an identity without a walk.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identity {
    Record(usize),
    Str(usize),
    Bytes(usize),
    List(usize),
    Array(usize),
}

/// The identities of `v` and of its direct parts, the first [`PARTS_LIMIT`] of them: what a body
/// that takes a value apart may hand back in.
pub(crate) fn identities(v: &Value) -> Vec<Identity> {
    let parts: Box<dyn Iterator<Item = &Value>> = match v {
        Value::Record(fields) => Box::new(fields.iter().map(|(_, part)| part)),
        Value::Ctor { args, .. } => Box::new(args.iter()),
        Value::List(items) if items.len() <= PARTS_LIMIT => Box::new(items.iter()),
        Value::Array(items) if items.len() <= PARTS_LIMIT => Box::new(items.iter()),
        _ => Box::new(std::iter::empty()),
    };
    identity(v)
        .into_iter()
        .chain(parts.take(PARTS_LIMIT).filter_map(identity))
        .collect()
}

pub(crate) fn identity(v: &Value) -> Option<Identity> {
    Some(match v {
        Value::Record(fields) => Identity::Record(Arc::as_ptr(fields) as usize),
        Value::Str(s) => Identity::Str(Arc::as_ptr(s) as *const u8 as usize),
        Value::Bytes(b) => Identity::Bytes(Arc::as_ptr(b) as *const u8 as usize),
        Value::List(items) => Identity::List(items.identity()),
        Value::Array(items) => Identity::Array(Arc::as_ptr(items) as usize),
        _ => return None,
    })
}

impl Tables {
    /// The memo's word for the pure nullary function at `index`, if it has one.
    pub fn memoized(&self, index: usize) -> Option<Word> {
        let w = self.memo.get(index)?.load(Acquire);
        (w != 0).then_some(w)
    }

    /// Remembers `kept`, an immortal copy emitted code made, as pure nullary function `index`'s
    /// answer, with what computing it cost, which every later read charges. The first kept stays;
    /// one another thread keeps after it stays in the unit's persistent heap, unread.
    pub fn keep(&self, index: usize, kept: Word, cost: Cost) {
        let (Some((steps, allocations, bytes)), Some(slot)) =
            (self.memo_costs.get(index), self.memo.get(index))
        else {
            return;
        };
        steps.store(cost.steps, Release);
        allocations.store(cost.allocations, Release);
        bytes.store(cost.bytes, Release);
        let _ = slot.compare_exchange(0, kept, AcqRel, Acquire);
    }

    /// What computing the memoized answer at `index` cost.
    pub fn memo_cost(&self, index: usize) -> Cost {
        match self.memo_costs.get(index) {
            Some((steps, allocations, bytes)) => Cost {
                steps: steps.load(Acquire),
                allocations: allocations.load(Acquire),
                bytes: bytes.load(Acquire),
            },
            None => Cost::default(),
        }
    }
}
