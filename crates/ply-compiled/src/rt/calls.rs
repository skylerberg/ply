//! What a call costs, the constants a unit reads, and calls of a function value.

use super::*;

/// What a computation cost: the calls it made, the objects it allocated and the bytes they took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub steps: i64,
    pub allocations: i64,
    pub bytes: i64,
}

impl Cost {
    pub(super) fn plus(self, other: Cost) -> Cost {
        Cost {
            steps: self.steps.saturating_add(other.steps),
            allocations: self.allocations.saturating_add(other.allocations),
            bytes: self.bytes.saturating_add(other.bytes),
        }
    }

    pub(super) fn minus(self, other: Cost) -> Cost {
        Cost {
            steps: self.steps - other.steps,
            allocations: self.allocations - other.allocations,
            bytes: self.bytes - other.bytes,
        }
    }
}

/// What the program had cost as it last gave way to the scheduler: its calls and what memoized
/// answers charged. The heap counts no block the scheduler makes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProgramCost {
    ticks: i64,
    charged: Cost,
}

impl Ctx {
    /// The scheduler's own work begins, where it had not: the program's costs are kept as they
    /// stand. Answers whether it had.
    pub(crate) fn scheduler_entered(&mut self) -> bool {
        if self.heap.scheduling {
            return true;
        }
        self.left_at = ProgramCost {
            ticks: self.ticks,
            charged: self.charged,
        };
        self.heap.scheduling = true;
        self.arm_tick_in_chunk();
        false
    }

    /// The program's own work runs again, where the scheduler's ran: its costs go on from where it
    /// left them, and the clock is read as many calls on as it was to be.
    pub(crate) fn program_entered(&mut self) {
        if !self.heap.scheduling {
            return;
        }
        let to_clock = (self.clock_at - self.ticks).max(1);
        let at = self.left_at;
        self.ticks = at.ticks;
        self.clock_at = self.ticks.saturating_add(to_clock);
        self.charged = at.charged;
        self.heap.scheduling = false;
        self.arm_tick_in_chunk();
    }

    /// What this entry has cost so far, memoized answers charged as if computed again.
    pub fn cost(&self) -> Cost {
        Cost {
            steps: self.ticks,
            allocations: i64::try_from(self.heap.allocated()).unwrap_or(i64::MAX),
            bytes: i64::try_from(self.heap.allocated_bytes()).unwrap_or(i64::MAX),
        }
        .plus(self.charged)
    }
}

/// The scheduler's own work begins, entered from a perform or a call of the program: answers 1 where
/// it already ran, for [`rt_scheduler_left`].
pub unsafe extern "C" fn rt_scheduler_entered(ctx: *mut Ctx) -> Word {
    Word::from(unsafe { &mut *ctx }.scheduler_entered())
}

/// Leaves what [`rt_scheduler_entered`] entered: the program runs again where `was` is 0.
pub unsafe extern "C" fn rt_scheduler_left(ctx: *mut Ctx, was: Word) {
    if was == 0 {
        unsafe { &mut *ctx }.program_entered();
    }
}

/// What an entry had cost when [`rt_cost_mark`] marked it: its cost, the objects live, and how many
/// atoms it had performed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CostMark {
    cost: Cost,
    live: i64,
    performed: usize,
}

// `?cost_mark` and `?cost_spent` are bound by the release's emitter, which still declares them;
// nothing this compiler emits calls them.

/// Marks what this entry has cost so far, the stops nothing can resume any longer dropped first,
/// and answers the mark, which [`rt_cost_spent`] reads back.
pub unsafe extern "C" fn rt_cost_mark(ctx: *mut Ctx) -> I64 {
    crate::detached::drain(ctx);
    let c = unsafe { &mut *ctx };
    let mark = CostMark {
        cost: c.cost(),
        live: c.live() as i64,
        performed: c.performed.len(),
    };
    c.cost_marks.push(mark);
    c.rearm_recording();
    I64(c.cost_marks.len() as i64 - 1)
}

/// Begins a lifting of what this entry spent since `mark`, the marks taken after it let go: the
/// record `{allocations, bytes, live, performs, steps}` of the calls it made, what it allocated and
/// the bytes that took, the objects live since, and each atom it performed with how many times, in
/// the atoms' order. `f` is the word of the function the span ran, lent and never read as an
/// object, only handed so the caller lets the function go after the counts. A mark this entry never
/// took fails it.
pub unsafe extern "C" fn rt_cost_spent(ctx: *mut Ctx, mark: I64, _f: I64) {
    crate::detached::drain(ctx);
    let c = unsafe { &mut *ctx };
    let Some(m) = usize::try_from(mark.0)
        .ok()
        .and_then(|at| c.cost_marks.get(at).copied().map(|m| (at, m)))
    else {
        c.fail(super::crossing::err_crossing(&format!(
            "a cost read back from mark {}, which this entry never took",
            mark.0
        )));
        return;
    };
    let (at, m) = m;
    c.cost_marks.truncate(at);
    c.rearm_recording();
    let spent = c.cost().minus(m.cost);
    let live = c.live() as i64 - m.live;
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for atom in &c.performed[m.performed.min(c.performed.len())..] {
        *counts.entry(atom.to_string()).or_insert(0) += 1;
    }
    let mut w = ply_core::codec::Writer::new();
    w.count(ply_core::codec::tag::RECORD, 5);
    w.name(b"allocations");
    w.int(spent.allocations);
    w.name(b"bytes");
    w.int(spent.bytes);
    w.name(b"live");
    w.int(live);
    w.name(b"performs");
    w.count(ply_core::codec::tag::LIST, counts.len() as u64);
    for (atom, count) in counts {
        w.count(ply_core::codec::tag::RECORD, 2);
        w.name(b"atom");
        w.str(atom.as_bytes());
        w.name(b"count");
        w.int(count);
    }
    w.name(b"steps");
    w.int(spent.steps);
    c.lift_from(w.finish(), Vec::new());
}

/// A read of the pure nullary definition at `index` in the unit's code table, which emitted code
/// writes for a constant's read: its value's word form. The first read evaluates the definition and
/// hands `keep`, the `keep#T` root of its type, the value, whose immortal copy the unit then keeps
/// for good; each later read answers that copy, charged what evaluating it cost, so a read costs
/// the same whether or not it was kept. A value `keep` answers 0 for, one that may not outlive its
/// entry, is kept nowhere, and each read evaluates the definition again. Where the evaluation fails
/// or raises, this answers 0 with the context as the evaluation left it, and keeps nothing.
/// Contexts reading one definition at once, on `parallel` branches or other threads, may each
/// evaluate it; the first copy kept is the one every later read answers.
pub unsafe extern "C" fn rt_memo(ctx: *mut Ctx, index: I64, keep: KeepRoot) -> Handed {
    let c = unsafe { &mut *ctx };
    if let Ok(i) = usize::try_from(index.0)
        && let Some(w) = c.tables.memoized(i)
    {
        let cost = c.tables.memo_cost(i);
        c.charged = c.charged.plus(cost);
        return Handed::of(w);
    }
    let tables = Arc::clone(&c.tables);
    let Some(&code) = usize::try_from(index.0)
        .ok()
        .and_then(|i| tables.functions.get(i))
    else {
        c.fail(err_crossing(&format!(
            "definition {} of a code table of {}",
            index.0,
            tables.functions.len()
        )));
        return Handed::of(0);
    };
    let index = index.0 as usize;
    let before = c.cost();
    // SAFETY: as in `call_value`; a nullary function never reads the null argument pointer.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(code) };
    let w = unsafe { f(ctx, std::ptr::null()) };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return Handed::of(0);
    }
    let cost = c.cost().minus(before);
    let kept = unsafe { keep(ctx, w) };
    if kept != 0 && unsafe { (*ctx).failed } == 0 {
        tables.keep(index, kept, cost);
    }
    Handed::of(w)
}
