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
    let held = ctx.heap.live();
    let performed = ctx.performed.len();
    let value = call_value(std::ptr::from_mut(ctx), f, &[]);
    crate::detached::drain(ctx);
    let live = ctx.heap.live() as i64 - held as i64;
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

/// The value of the pure nullary function at `index`, memoized when world-independent.
pub unsafe extern "C" fn rt_constant(ctx: *mut Ctx, index: i64) -> i64 {
    let tables = Arc::clone(&unsafe { &*ctx }.tables);
    if let Some(w) = tables.memoized(index as usize) {
        let c = unsafe { &mut *ctx };
        c.charged = c.charged.plus(tables.memo_cost(index as usize));
        return w;
    }
    let before = unsafe { &*ctx }.cost();
    // SAFETY: as in `call_value`; a nullary function never reads the null argument pointer.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(tables.functions[index as usize]) };
    let w = unsafe { f(ctx, std::ptr::null()) };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return 0;
    }
    // The memo keeps a copy; the entry keeps using its own word.
    if heap::world_independent(w) {
        tables.memoize_costing(index as usize, w, c.cost().minus(before));
    }
    w
}

/// The value a unit holds as `len` bytes of text at `text`: what a build kept of a `const`
/// definition, read into the unit's own heap, so the entry that reads it is charged nothing.
pub unsafe extern "C" fn rt_baked(ctx: *mut Ctx, text: i64, len: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let tables = Arc::clone(&c.tables);
    let text = unsafe { std::slice::from_raw_parts(text as *const u8, len as usize) };
    let read = crate::stored::read(
        &tables.layouts,
        &tables.nullaries,
        &mut lock(&tables.immortals),
        text,
    );
    match read {
        Ok(w) => w,
        Err(why) => c.fail(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the value a build kept of a `const` definition does not read: {why}"),
            )
            .primary(Span::DUMMY, "in compiled code")
            .note("this is Ply's fault: a unit holds only values this runtime wrote"),
        ),
    }
}

/// `w` as the text [`rt_baked`] reads back, a `Bytes`. Reads `w`.
pub unsafe extern "C" fn rt_stored(ctx: *mut Ctx, w: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let tables = Arc::clone(&c.tables);
    match crate::stored::text(&tables.layouts, w) {
        Ok(text) if u32::try_from(text.len()).is_ok() => c.heap.bytes(&text),
        Ok(text) => c.fail(error(format!(
            "a value of {} stored bytes has no stored form",
            text.len()
        ))),
        Err(what) => c.fail(error(format!("{what} has no stored form"))),
    }
}

/// A call through a value. Takes the callee and the arguments. A closure this call holds the only
/// count of is never called again, so it gives the call its captures rather than sharing them.
pub unsafe extern "C" fn rt_call(ctx: *mut Ctx, callee: i64, args: *const i64, n: i64) -> i64 {
    let args = args_of(args, n);
    let give = heap::kind(callee) == KIND_CLOSURE && is_unique(callee);
    let r = applied(ctx, callee, args, give);
    heap::dec(callee);
    r
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
