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

impl Ctx {
    /// What this entry has cost so far, memoized answers charged as if computed again.
    pub(super) fn cost(&self) -> Cost {
        Cost {
            steps: self.ticks,
            allocations: i64::try_from(self.heap.allocated()).unwrap_or(i64::MAX),
            bytes: i64::try_from(self.heap.allocated_bytes()).unwrap_or(i64::MAX),
        }
        .plus(self.charged)
    }
}

/// `metered(f)`: `f()` and what it cost, as `{allocations, bytes, live, performs, steps, value}`
/// with each performed atom's count in `performs`, ordered by the atom.
pub(super) fn metered(ctx: &mut Ctx, f: Word) -> Word {
    let before = ctx.cost();
    crate::detached::drain(ctx);
    let held = ctx.live();
    let performed = ctx.performed.len();
    let value = call_value(std::ptr::from_mut(ctx), f, &[]);
    crate::detached::drain(ctx);
    let live = ctx.live() as i64 - held as i64;
    heap::dec(f);
    if ctx.failed != 0 {
        return 0;
    }
    let spent = ctx.cost().minus(before);
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for atom in &ctx.performed[performed..] {
        *counts.entry(atom.to_string()).or_insert(0) += 1;
    }
    let tables = Arc::clone(&ctx.tables);
    let items: Vec<Word> = counts
        .into_iter()
        .map(|(atom, count)| {
            let text = ctx.heap.str(&atom);
            ctx.heap.record(
                &tables.layouts,
                &[
                    (Symbol::new("atom"), text),
                    (Symbol::new("count"), heap::imm(count)),
                ],
            )
        })
        .collect();
    let performs = ctx.heap.list_from(&items);
    ctx.heap.record(
        &tables.layouts,
        &[
            (Symbol::new("allocations"), heap::imm(spent.allocations)),
            (Symbol::new("bytes"), heap::imm(spent.bytes)),
            (Symbol::new("live"), heap::imm(live)),
            (Symbol::new("performs"), performs),
            (Symbol::new("steps"), heap::imm(spent.steps)),
            (Symbol::new("value"), value),
        ],
    )
}

/// What an entry had cost when [`rt_cost_mark`] marked it: its cost, the objects live, and how many
/// atoms it had performed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CostMark {
    cost: Cost,
    live: i64,
    performed: usize,
}

/// Marks what this entry has cost so far, the stops nothing can resume any longer dropped first as
/// `metered` drops them, and answers the mark, which [`rt_cost_spent`] reads back.
pub unsafe extern "C" fn rt_cost_mark(ctx: *mut Ctx) -> I64 {
    crate::detached::drain(ctx);
    let c = unsafe { &mut *ctx };
    let mark = CostMark {
        cost: c.cost(),
        live: c.live() as i64,
        performed: c.performed.len(),
    };
    c.cost_marks.push(mark);
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
    let tables = Arc::clone(&unsafe { &*ctx }.tables);
    let c = unsafe { &mut *ctx };
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
    if let Some(w) = tables.memoized(index) {
        c.charged = c.charged.plus(tables.memo_cost(index));
        return Handed::of(w);
    }
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

/// Applies `callee` to `args`, or answers 0 with the context failed. Reads the callee, takes the
/// arguments.
pub(crate) fn call_value(ctx: *mut Ctx, callee: Word, args: &[Word]) -> i64 {
    applied(ctx, callee, args, false)
}

/// `call_value`, a closure's captures taken out of it where `give` is set: the caller holds the
/// closure alone and lets it go after.
pub(super) fn applied(ctx: *mut Ctx, callee: Word, args: &[Word], give: bool) -> i64 {
    let c = unsafe { &mut *ctx };
    match heap::kind(callee) {
        KIND_CLOSURE => {
            let o = obj(callee);
            let (arity, len) = unsafe { ((*o).layout as usize, (*o).len as usize) };
            if args.len() != arity {
                let d = error(format!(
                    "a compiled function takes {arity} arguments and was given {}",
                    args.len()
                ));
                return c.fail(d);
            }
            // Captures are held once more, or given; the arguments are already taken.
            // Uninitialised on purpose, as zeroing costs every call;
            // exactly `total` words are written and read.
            let mut handles = [const { std::mem::MaybeUninit::<i64>::uninit() }; 64];
            let captures = len - CLOSURE_CAPTURES;
            let total = captures + args.len();
            let inline = total <= handles.len();
            let spill = if inline { 0 } else { uncounted(c, total) };
            let mut push = |w: Word, i: usize| {
                if inline {
                    handles[i].write(w);
                } else {
                    unsafe { set_word(obj(spill), i, w) };
                }
            };
            for i in 0..captures {
                let w = unsafe { word_at(o, CLOSURE_CAPTURES + i) };
                if give {
                    unsafe { set_word(o, CLOSURE_CAPTURES + i, heap::unit()) };
                } else {
                    heap::inc(w);
                }
                push(w, i);
            }
            for (i, w) in args.iter().enumerate() {
                push(*w, captures + i);
            }
            let ptr = if inline {
                handles.as_ptr() as *const i64
            } else {
                unsafe { heap::words(obj(spill)) as *const i64 }
            };
            let code = unsafe { word_at(o, CLOSURE_CODE) } as usize;
            // SAFETY: `code` is a finalized address from `Tables::functions`, which `Bodies`
            // keeps alive as long as this context, with the signature every compiled function has.
            let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(code) };
            let r = unsafe { f(ctx, ptr) };
            if spill != 0 {
                heap::dec(spill);
            }
            r
        }
        KIND_BRIDGE => {
            let value = unsafe { bridged(obj(callee)) };
            let Value::Closure(closure) = value else {
                let d = error(format!(
                    "a call needs a function, and this is {}",
                    value.type_name()
                ));
                return c.fail(d);
            };
            match &closure.kind {
                ClosureKind::Builtin(b) => builtin(c, *b, args),
                ClosureKind::Ctor { name, arity } => {
                    if args.len() != *arity {
                        let d = error(format!(
                            "the constructor `{name}` takes {arity} fields and was given {}",
                            args.len()
                        ));
                        return c.fail(d);
                    }
                    match c.tables.layouts.ctor_index(name) {
                        Some(index) => {
                            let tables = Arc::clone(&c.tables);
                            c.heap.ctor(&tables.layouts, index, args)
                        }
                        None => {
                            let values = values_taken(c, args);
                            c.word(&Value::ctor(name.clone(), values))
                        }
                    }
                }
                ClosureKind::Synth { arity, rule } => {
                    if args.len() != usize::from(*arity) {
                        let d = error(format!(
                            "a generated function takes {arity} arguments and was given {}",
                            args.len()
                        ));
                        return c.fail(d);
                    }
                    let values = values_taken(c, args);
                    match rule.apply(&values) {
                        Ok(v) => c.word(&v),
                        Err(d) => c.fail(d),
                    }
                }
                // `Heap::to_word` rebuilds a native closure as one, so it never crosses as a bridge.
                ClosureKind::Native { .. }
                | ClosureKind::Continuation { .. }
                | ClosureKind::Hold { .. } => {
                    let d = error("a compiled closure arrived bridged rather than native");
                    c.fail(d)
                }
            }
        }
        _ => {
            let d = error(format!(
                "a call needs a function, and this is {}",
                c.type_name(callee)
            ));
            c.fail(d)
        }
    }
}
