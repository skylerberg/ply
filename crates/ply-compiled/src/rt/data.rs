//! Lists, maps, arrays and records as compiled code walks and writes them.

use super::*;

/// A native list, or a failure naming the builtin that needed one.
pub(super) fn native_list(ctx: &mut Ctx, w: Word, what: &str) -> Option<*mut heap::Obj> {
    if heap::kind(w) == KIND_LIST {
        return Some(obj(w));
    }
    let d = error(format!(
        "`{what}` needs a List, and this is {}",
        ctx.type_name(w)
    ));
    ctx.fail(d);
    None
}

/// `xs.map(f)`: `f` on every element, in order. Takes the list and the function.
pub unsafe extern "C" fn rt_map(ctx: *mut Ctx, list: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "List::map") else {
        return 0;
    };
    // Walked in place and gathered on the heap, so nothing here is owned across `f`'s calls.
    let mut out = c.heap.list_from(&[]);
    let mut failed = false;
    list::for_each(items, |x| {
        if failed {
            return;
        }
        heap::inc(x);
        let r = call_value(ctx, f, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            failed = true;
            return;
        }
        out = c.heap.list_grown(out, r);
    });
    if failed {
        heap::dec(out);
        return 0;
    }
    heap::dec(list);
    heap::dec(f);
    out
}

/// `xs.filter(p)`: the elements `p` answers `true` for. Takes the list and the predicate.
pub unsafe extern "C" fn rt_filter(ctx: *mut Ctx, list: i64, p: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "List::filter") else {
        return 0;
    };
    // Walked in place and gathered on the heap, so nothing here is owned across `p`'s calls.
    let mut kept = c.heap.list_from(&[]);
    let mut failed = false;
    list::for_each(items, |x| {
        if failed {
            return;
        }
        heap::inc(x);
        let r = call_value(ctx, p, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            failed = true;
            return;
        }
        match heap::as_bool(r) {
            Some(true) => {
                heap::inc(x);
                kept = c.heap.list_grown(kept, x);
            }
            Some(false) => {}
            None => {
                let d = error(format!(
                    "the predicate given to `List::filter` answered {}, not a Bool",
                    c.type_name(r)
                ));
                c.fail(d);
                failed = true;
            }
        }
    });
    if failed {
        heap::dec(kept);
        return 0;
    }
    heap::dec(list);
    heap::dec(p);
    kept
}

/// `xs.fold(init, f)`. Takes all three.
pub unsafe extern "C" fn rt_fold(ctx: *mut Ctx, list: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "List::fold") else {
        return 0;
    };
    // The list is held until the walk ends, so `f` may run whatever it likes over it.
    let mut acc = init;
    let mut failed = false;
    list::for_each(items, &mut |x| {
        if failed {
            return;
        }
        heap::inc(x);
        acc = call_value(ctx, f, &[acc, x]);
        failed = unsafe { (*ctx).failed } != 0;
    });
    if failed {
        return 0;
    }
    heap::dec(list);
    heap::dec(f);
    acc
}

/// `Map::fold(m, init, f)` in ascending key order over a snapshot of the entries. Takes all three.
pub unsafe extern "C" fn rt_map_fold(ctx: *mut Ctx, map: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    if heap::kind(map) == KIND_MAP {
        // Walked in place, so nothing here is owned across `f`'s calls.
        let mut acc = init;
        let mut failed = false;
        map::for_each(obj(map), |k, v| {
            if failed {
                return;
            }
            heap::inc(k);
            heap::inc(v);
            acc = call_value(ctx, f, &[acc, k, v]);
            failed = unsafe { &*ctx }.failed != 0;
        });
        if failed {
            return 0;
        }
        heap::dec(map);
        heap::dec(f);
        return acc;
    }
    let bridged = (heap::kind(map) == KIND_BRIDGE).then(|| unsafe { bridged(obj(map)) });
    let Some(Value::Map(entries)) = bridged else {
        let d = error(format!(
            "`Map::fold` needs a Map, and this is {}",
            c.type_name(map)
        ));
        return c.fail(d);
    };
    let mut acc = init;
    for (k, v) in entries.iter() {
        let c = unsafe { &mut *ctx };
        let kw = c.word(k);
        let vw = c.word(v);
        acc = call_value(ctx, f, &[acc, kw, vw]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
    }
    heap::dec(map);
    heap::dec(f);
    acc
}

/// `iterate(seed, budget, f)`: `f` until it answers `Stop` or the budget runs out. Takes all three.
pub unsafe extern "C" fn rt_iterate(ctx: *mut Ctx, seed: i64, budget: i64, f: i64) -> Raising {
    Raising(iterated(ctx, seed, budget, f))
}

pub(super) fn iterated(ctx: *mut Ctx, seed: i64, budget: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let budget = match heap::as_int(budget) {
        Some(n) if n >= 1 => n,
        Some(n) => {
            let d = error(format!(
                "`iterate` needs a budget of at least 1, and this is {n}"
            ));
            return raise_error(c, d);
        }
        None => {
            let d = error(format!(
                "`iterate` needs an Int budget, and this is {}",
                c.type_name(budget)
            ));
            return c.fail(d);
        }
    };
    let stop = c.tables.layouts.stop;
    let go = c.tables.layouts.go;
    let mut state = seed;
    let mut left = budget;
    loop {
        if left <= 0 {
            let c = unsafe { &mut *ctx };
            let d = error(format!(
                "`iterate` did not stop within its budget of {budget}"
            ));
            return raise_error(c, d);
        }
        left -= 1;
        let r = call_value(ctx, f, &[state]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        // The answer gives up its payload so the threaded state stays uniquely held.
        let tag = if heap::kind(r) == KIND_CTOR {
            Some(unsafe { ((*obj(r)).layout, (*obj(r)).len) })
        } else {
            None
        };
        match tag {
            Some((index, 1)) if Some(index) == go || Some(index) == stop => {
                let payload = unsafe { word_at(obj(r), 0) };
                unsafe { set_word(obj(r), 0, heap::unit()) };
                heap::dec(r);
                if Some(index) == stop {
                    heap::dec(f);
                    return payload;
                }
                state = payload;
            }
            _ => {
                let d = error(format!(
                    "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                    c.type_name(r)
                ));
                return c.fail(d);
            }
        }
    }
}

/// `bracket(acquire, release, body)`: what `body` answers for what `acquire` answered, with
/// `release` run on it however `body` ends: by returning, by a raise or a clause that did not
/// resume it unwinding through, or by a cancel. It runs where the bracket stands, with the
/// handlers around it, and a failure in it replaces whatever was unwinding. A runtime failure ends
/// the entry, so nothing more runs then. A cancel takes no answer from `acquire` and no wait from
/// `release`: it lands when that one returns, so what was acquired is released, and wholly.
pub(super) fn rt_bracket(ctx: *mut Ctx, acquire: Word, release: Word, body: Word) -> Word {
    let entered = crate::simulate::shield(unsafe { &mut *ctx }, Shield::Acquire);
    let held = call_value(ctx, acquire, &[]);
    heap::dec(acquire);
    let c = unsafe { &mut *ctx };
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 {
        if cancelled {
            cancel_here(c);
        }
        heap::dec(release);
        heap::dec(body);
        return 0;
    }
    let standing = c.fuel;
    let answer = if cancelled {
        c.failed = FAILED_CANCELLED;
        0
    } else {
        heap::inc(held);
        call_value(ctx, body, &[held])
    };
    heap::dec(body);
    let c = unsafe { &mut *ctx };
    // `release` runs as deep as the bracket stands, however deep the body was when it ended.
    c.fuel = standing;
    let ending = c.failed;
    if ending != 0
        && ending != FAILED_UNWIND
        && ending != FAILED_CANCELLED
        && ending != FAILED_ABORT
    {
        heap::dec(held);
        heap::dec(release);
        return 0;
    }
    let unwinding = c.unwind.take();
    let raising = c.aborting.take();
    c.failed = 0;
    let entered = crate::simulate::shield(c, Shield::Release);
    let raise = raising.map(|a| c.keep(Kept::Raising(a)));
    let released = call_value(ctx, release, &[held]);
    heap::dec(release);
    let c = unsafe { &mut *ctx };
    let raising = raise.map(|here| match c.kept_back(here) {
        Kept::Raising(a) => a,
        _ => unreachable!("a release keeps the raise it runs during"),
    });
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 || cancelled {
        if let Some((_, _, carried)) = unwinding {
            heap::dec(carried);
        }
        if let Some(raise) = raising {
            raise.into_failure();
        }
        heap::dec(answer);
        if c.failed == 0 {
            heap::dec(released);
            c.failed = FAILED_CANCELLED;
        } else if cancelled {
            cancel_here(c);
        }
        return 0;
    }
    heap::dec(released);
    c.failed = ending;
    c.unwind = unwinding;
    c.aborting = raising;
    answer
}

/// [`rt_bracket`] over closures emitted code made: `body` given what `acquire` answered and
/// `cleanup` given it after, however `body` ends, each closure let go once called. `held` is the
/// type of what `acquire` answers, which both take a count of, and `answer` the type of what `body`
/// answers, let go of where `cleanup` fails. A value a clause unwinds with is abandoned with the
/// unwind, uncounted: its type is not known here. What this holds across each call is in a record
/// on the stack's chain, so a stop under the call that is resumed more than once holds it once a
/// resume.
pub unsafe extern "C" fn rt_with_cleanup(
    ctx: *mut Ctx,
    acquire: Block,
    body: Block,
    cleanup: Block,
    held: *const PlyType,
    answer: *const PlyType,
) -> Handed {
    use crate::frames::{Held, held_across};
    let entered = crate::simulate::shield(unsafe { &mut *ctx }, Shield::Acquire);
    let f = acquire.word();
    let holding = [
        Held::Closure(acquire),
        Held::Closure(body),
        Held::Closure(cleanup),
    ];
    let (acquired, back) = unsafe { held_across(ctx, holding, || Block::of(f).call(ctx, &[])) };
    let [
        Held::Closure(acquire),
        Held::Closure(body),
        Held::Closure(cleanup),
    ] = back
    else {
        unreachable!("a record gives back what it held");
    };
    unsafe { acquire.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 {
        if cancelled {
            cancel_carried(c);
        }
        unsafe {
            cleanup.let_go_function(ctx);
            body.let_go_function(ctx);
        }
        return Handed::of(0);
    }
    let held = unsafe { Carried::new(Handed::of(acquired), held) };
    let standing = c.fuel;
    let (answered, body, cleanup, held) = if cancelled {
        c.failed = FAILED_CANCELLED;
        (None, body, cleanup, held)
    } else {
        let given = held.dup(ctx).handed().word();
        let f = body.word();
        let holding = [
            Held::Closure(body),
            Held::Closure(cleanup),
            Held::Carried(held),
        ];
        let (r, back) = unsafe { held_across(ctx, holding, || Block::of(f).call(ctx, &[given])) };
        let [
            Held::Closure(body),
            Held::Closure(cleanup),
            Held::Carried(held),
        ] = back
        else {
            unreachable!("a record gives back what it held");
        };
        let r = unsafe { Carried::new(Handed::of(r), answer) };
        (Some(r), body, cleanup, held)
    };
    unsafe { body.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    // `cleanup` runs as deep as the bracket stands, however deep the body was when it ended.
    c.fuel = standing;
    let ending = c.failed;
    if ending != 0
        && ending != FAILED_UNWIND
        && ending != FAILED_CANCELLED
        && ending != FAILED_ABORT
    {
        held.release(ctx);
        unsafe { cleanup.let_go_function(ctx) };
        return Handed::of(0);
    }
    let answered = answered.filter(|_| ending == 0);
    let unwinding = c.unwind.take();
    let raising = c.aborting.take();
    c.failed = 0;
    let entered = crate::simulate::shield(c, Shield::Release);
    let raise = raising.map(|a| c.keep(Kept::Raising(a)));
    let (f, given) = (cleanup.word(), held.handed().word());
    let call = || unsafe { Block::of(f).call(ctx, &[given]) };
    let (cleanup, answered) = match answered {
        Some(a) => {
            let (_, back) =
                unsafe { held_across(ctx, [Held::Closure(cleanup), Held::Carried(a)], call) };
            let [Held::Closure(cleanup), Held::Carried(a)] = back else {
                unreachable!("a record gives back what it held");
            };
            (cleanup, Some(a))
        }
        None => {
            let (_, back) = unsafe { held_across(ctx, [Held::Closure(cleanup)], call) };
            let [Held::Closure(cleanup)] = back else {
                unreachable!("a record gives back what it held");
            };
            (cleanup, None)
        }
    };
    unsafe { cleanup.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    let raising = raise.map(|here| match c.kept_back(here) {
        Kept::Raising(a) => a,
        _ => unreachable!("a cleanup keeps the raise it runs during"),
    });
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 || cancelled {
        if let Some(raise) = raising {
            raise.into_failure();
        }
        if let Some(a) = answered {
            a.release(ctx);
        }
        let c = unsafe { &mut *ctx };
        if c.failed == 0 {
            c.failed = FAILED_CANCELLED;
        } else if cancelled {
            cancel_carried(c);
        }
        return Handed::of(0);
    }
    c.failed = ending;
    c.unwind = unwinding;
    c.aborting = raising;
    answered.map_or(Handed::of(0), Carried::handed)
}

/// [`cancel_here`] where what a clause unwinds with is emitted code's, which is abandoned uncounted.
fn cancel_carried(c: &mut Ctx) {
    if c.failed != FAILED_UNWIND && c.failed != FAILED_ABORT {
        return;
    }
    c.unwind = None;
    if let Some(raise) = c.aborting.take() {
        raise.into_failure();
    }
    c.failed = FAILED_CANCELLED;
}

/// A cancel held back until now lands on a task a raise or a clause is unwinding: the task unwinds
/// as cancelled from here, since a handler that answered the other would let it run on.
pub(super) fn cancel_here(c: &mut Ctx) {
    if c.failed != FAILED_UNWIND && c.failed != FAILED_ABORT {
        return;
    }
    if let Some((_, _, carried)) = c.unwind.take() {
        heap::dec(carried);
    }
    if let Some(raise) = c.aborting.take() {
        raise.into_failure();
    }
    c.failed = FAILED_CANCELLED;
}

/// The builtins emitted code writes in place whose refusal of their operands it hands the runtime,
/// each numbered by its place here: through [`rt_refused`] where the refusal names numbers, and
/// through [`rt_refused_text`](crate::machine::rt_refused_text) where it names text.
pub const REFUSED: [Builtin; 23] = [
    Builtin::ArrayNew,
    Builtin::ColumnsNew,
    Builtin::ArrayOfList,
    Builtin::ColumnsOfList,
    Builtin::ArrayGet,
    Builtin::ArraySet,
    Builtin::ColumnsGet,
    Builtin::ColumnsSet,
    Builtin::BytesAt,
    Builtin::BytesU32Le,
    Builtin::BytesSlice,
    Builtin::StringSlice,
    Builtin::ByteOfInt,
    Builtin::BytesIndexOfByte,
    Builtin::BytesIndexOfFrom,
    Builtin::BytesScan,
    Builtin::BytesScanUntil,
    Builtin::Range,
    Builtin::BytesSplit,
    Builtin::StringSplit,
    Builtin::StringFind,
    Builtin::StringOfBytes,
    Builtin::Assert,
];

/// The builtin [`REFUSED`] numbers `what`.
pub(crate) fn refused_builtin(what: I64) -> Option<Builtin> {
    usize::try_from(what.0)
        .ok()
        .and_then(|at| REFUSED.get(at).copied())
}

/// Builtin `b`'s refusal `d`, left in the context as `b` leaves it over values: raised for
/// `abort.raise` to answer where `b` raises, else failing the run.
pub(crate) fn refuse(c: &mut Ctx, b: Builtin, d: Diagnostic) {
    if signature(b).raises {
        raise_error(c, d);
    } else {
        c.fail(d);
    }
}

/// The entry failed as Ply's fault: emitted code handed a refusal `what` that names no builtin the
/// native refuses.
pub(crate) fn refused_unknown(c: &mut Ctx, what: I64) {
    c.fail(
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("a refused operation numbered {}, which none is", what.0),
        )
        .note("this is Ply's fault, not the program's"),
    );
}

/// An operation emitted code writes in place, refused numbers among its operands: left in the
/// context as [`refuse`] leaves the builtin's own refusal of them. `what` numbers the builtin as
/// [`REFUSED`] does, and `a`, `b` and `c` are what its refusal names: `Array::new`, `Columns::new`,
/// `Array::of_list` and `Columns::of_list` a length `a`; `Array::get`, `Array::set`,
/// `Columns::get`, `Columns::set`, `Bytes::at` and `Bytes::u32_le` an index `a` into `b` elements or
/// bytes; `Bytes::slice` and `String::slice` a range `a..b` of `c` bytes or characters;
/// `byte_of_int` and `Bytes::index_of_byte` a byte `a`; `Bytes::index_of_from` a start `a` in `b`
/// bytes, and `Bytes::scan` and `Bytes::scan_until` that and a budget `c`; `range` its ends `a` and
/// `b`; `Bytes::split` and `String::split` nothing. Any other `what` fails the entry as Ply's fault.
#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn rt_refused(ctx: *mut Ctx, what: I64, a: I64, b: I64, c: I64) -> Raises {
    use ply_core::builtins as said;
    let cx = unsafe { &mut *ctx };
    let site = cx.site();
    let len = |n: I64| usize::try_from(n.0).unwrap_or(0);
    let Some(builtin) = refused_builtin(what) else {
        refused_unknown(cx, what);
        return Raises(());
    };
    let name = builtin.name();
    let refused = match builtin {
        Builtin::ArrayNew | Builtin::ColumnsNew | Builtin::ArrayOfList | Builtin::ColumnsOfList => {
            said::array_length(site, name, a.0)
        }
        Builtin::ArrayGet | Builtin::ArraySet | Builtin::ColumnsGet | Builtin::ColumnsSet => {
            said::out_of_range(site, name, a.0, len(b), "elements")
        }
        Builtin::BytesAt => said::out_of_range(site, name, a.0, len(b), "bytes"),
        // Told at the last index it would read, as the builtin tells it.
        Builtin::BytesU32Le => said::out_of_range(site, name, a.0.wrapping_add(3), len(b), "bytes"),
        Builtin::BytesSlice => said::range_outside(site, name, a.0, b.0, len(c), "bytes"),
        Builtin::StringSlice => said::range_outside(site, name, a.0, b.0, len(c), "characters"),
        Builtin::ByteOfInt => said::byte_of_int_refused(site, a.0),
        Builtin::BytesIndexOfByte => said::not_a_byte(site, name, a.0),
        Builtin::BytesIndexOfFrom => said::start_outside(a.0, len(b), site, name),
        Builtin::BytesScan | Builtin::BytesScanUntil => {
            if usize::try_from(a.0).is_ok_and(|from| from <= len(b)) {
                said::negative_budget(site, name, c.0)
            } else {
                said::start_outside(a.0, len(b), site, name)
            }
        }
        Builtin::Range => said::range_too_long(site, a.0, b.0),
        Builtin::BytesSplit | Builtin::StringSplit => said::empty_separator(site, builtin),
        _ => {
            refused_unknown(cx, what);
            return Raises(());
        }
    };
    refuse(cx, builtin, refused);
    Raises(())
}

/// A fused `iterate`'s failure: `what` 0 a budget under one, 1 the budget spent, 2 a bad step
/// answer `n`, which it takes.
pub unsafe extern "C" fn rt_iterate_bad(ctx: *mut Ctx, what: i64, n: i64) {
    let ctx = unsafe { &mut *ctx };
    match what {
        0 => raise_error(
            ctx,
            error(format!(
                "`iterate` needs a budget of at least 1, and this is {n}"
            )),
        ),
        1 => raise_error(
            ctx,
            error(format!("`iterate` did not stop within its budget of {n}")),
        ),
        _ => {
            let d = error(format!(
                "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                ctx.type_name(n)
            ));
            heap::dec(n);
            ctx.fail(d)
        }
    };
}

/// A shift count outside the word; `which` indexes [`ply_core::INT_TYPES`], or is `-1` for `Int`.
pub unsafe extern "C" fn rt_shift_count(ctx: *mut Ctx, n: i64, which: i64) {
    let ctx = unsafe { &mut *ctx };
    let (ty, width) = match usize::try_from(which)
        .ok()
        .and_then(|i| ply_core::INT_TYPES.get(i))
    {
        Some(t) => (t.name(), i64::from(t.bits())),
        None => ("Int", 64),
    };
    // The machine says this on a label at the shift; compiled code has no span for one.
    let d = error("shift count out of range")
        .note(format!("{n} is not in 0..={}", width - 1))
        .note(format!(
            "a `{ty}` is {width} bits, so no other count names a shift of it"
        ));
    ctx.fail(d);
}

/// One field of a record by name. `own`: 0 reads the base and holds the field once more; 2 moves
/// the field out of a unique base that stays; 1 and 3 also take and release the base.
pub unsafe extern "C" fn rt_field(ctx: *mut Ctx, base: i64, index: i64, own: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(base) != KIND_RECORD {
        let d = error(format!(
            "a field access needs a record, and this is {}",
            ctx.type_name(base)
        ));
        return ctx.fail(d);
    }
    let o = obj(base);
    let shape = unsafe { (*o).layout };
    let Some(at) = ctx
        .tables
        .layouts
        .offset_by_index(shape, index as usize, &ctx.tables.fields)
    else {
        let d = error(format!(
            "this record has no field `{}`",
            ctx.tables.fields[index as usize]
        ));
        return ctx.fail(d);
    };
    let slot = ctx.tables.layouts.slot_at(shape, at);
    if slot != heap::Slot::Word {
        let tables = Arc::clone(&ctx.tables);
        let w = heap::field_word(&mut ctx.heap, &tables.layouts, o, at, slot);
        if own == 1 || own == 3 {
            heap::dec(base);
        }
        return w;
    }
    let w = unsafe { word_at(o, at) };
    match own {
        0 => heap::inc(w),
        2 => {
            if is_unique(base) {
                unsafe { set_word(o, at, heap::unit()) };
            } else {
                heap::inc(w);
            }
        }
        _ => {
            if is_unique(base) {
                unsafe { set_word(o, at, heap::unit()) };
            } else {
                heap::inc(w);
            }
            heap::dec(base);
        }
    }
    w
}

/// A record of `shape`'s fields holding `n` words at `args`, which it takes: laid out as a record of
/// the values they are, as one of its type is wherever it is made.
pub unsafe extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let tables = Arc::clone(&ctx.tables);
    let names = tables.layouts.shape_names(shape as u32);
    let fields: Vec<(Symbol, Word)> = names
        .iter()
        .cloned()
        .zip(args_of(args, n).iter().copied())
        .collect();
    ctx.heap.record(&tables.layouts, &fields)
}

/// An empty list with room in its tail for `n` elements, at most a leaf's: a loop pushing that
/// many onto it grows it in place.
pub unsafe extern "C" fn rt_list_room(ctx: *mut Ctx, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap
        .alloc_list(n.clamp(0, crate::list::WIDTH as i64) as u32) as Word
}

/// One element of a list whose length admits `i`: held once more. Reads.
pub unsafe extern "C" fn rt_list_at(ctx: *mut Ctx, value: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    let o = obj(value);
    if i < 0 || i >= list::len(o) as i64 {
        let d = error("a list pattern read past the end of the list");
        return ctx.fail(d);
    }
    let w = list::get(o, i as usize);
    heap::inc(w);
    w
}

/// What a `..rest` binds: the list from `from` on, sharing the trie. Reads.
pub unsafe extern "C" fn rt_list_rest(ctx: *mut Ctx, value: i64, from: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    ctx.heap.list_skip(value, from.max(0) as usize)
}

/// A fresh object of `kind` with `len` words, whose fields the compiled caller stores itself.
pub unsafe extern "C" fn rt_alloc(
    ctx: *mut Ctx,
    kind: i64,
    len: i64,
    layout: i64,
    flags: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap
        .alloc(kind as u8, flags as u8, len as u32, layout as u32) as Word
}

/// A write of one element the emitted code made: in place, or into a copy of `copied` elements.
pub unsafe extern "C" fn rt_note_update(_ctx: *mut Ctx, in_place: i64, copied: i64) {
    ply_core::rc::note_update_of(in_place != 0, copied as usize, Span::DUMMY);
}

/// An empty node of a list's trie or a map's tree, of `kind`, with room for a full one.
pub unsafe extern "C" fn rt_node(ctx: *mut Ctx, kind: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match kind as u8 {
        KIND_MLEAF => ctx.heap.alloc_leaf() as Word,
        KIND_MBRANCH => ctx.heap.alloc_branch() as Word,
        kind => ctx.heap.alloc_node(kind, &[]),
    }
}

pub unsafe extern "C" fn rt_push(ctx: *mut Ctx, xs: i64, x: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_LIST {
        return ctx.heap.list_push(xs, x);
    }
    builtin(ctx, Builtin::Push, &[xs, x])
}

pub unsafe extern "C" fn rt_map_insert(ctx: *mut Ctx, m: i64, k: i64, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(&ctx.tables.layouts, k) {
        let tables = Arc::clone(&ctx.tables);
        return ctx.heap.map_insert(&tables.layouts, m, k, v);
    }
    builtin(ctx, Builtin::MapInsert, &[m, k, v])
}

pub unsafe extern "C" fn rt_map_contains(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(&ctx.tables.layouts, k) {
        let found = map::get(&ctx.tables.layouts, obj(m), k).is_some();
        heap::dec(m);
        heap::dec(k);
        return heap::bool(found);
    }
    builtin(ctx, Builtin::MapContains, &[m, k])
}

pub unsafe extern "C" fn rt_byte_of_int(ctx: *mut Ctx, n: i64) -> Raising {
    let ctx = unsafe { &mut *ctx };
    Raising(match heap::as_int(n).and_then(|v| u8::try_from(v).ok()) {
        Some(b) => ctx.tables.byte(b),
        None => builtin(ctx, Builtin::ByteOfInt, &[n]),
    })
}
