//! Counts, boxing, arithmetic, decimals and fixed widths, and the operations a type's module states.

use super::*;

/// Perceus's `drop`: one holder fewer.
pub unsafe extern "C" fn rt_dec(ctx: *mut Ctx, w: i64) {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.release_last(w);
}

/// Perceus's `reset`, as [`heap::reset`].
pub unsafe extern "C" fn rt_reset(_ctx: *mut Ctx, w: i64) -> i64 {
    heap::reset(w)
}

pub unsafe extern "C" fn rt_box_int(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.boxed_int(v)
}

pub unsafe extern "C" fn rt_unbox_int(ctx: *mut Ctx, w: i64) -> i64 {
    match heap::as_int(w) {
        Some(i) => i,
        None => {
            let ctx = unsafe { &mut *ctx };
            let d = error(format!("an `Int` operation on a {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

pub unsafe extern "C" fn rt_unbox_bool(ctx: *mut Ctx, w: i64) -> i64 {
    match heap::as_bool(w) {
        Some(b) => i64::from(b),
        None => {
            let ctx = unsafe { &mut *ctx };
            let d = error(format!("a condition of type {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

/// A `Float` held as a word, from its bits.
pub unsafe extern "C" fn rt_box_float(ctx: *mut Ctx, bits: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.boxed_float(f64::from_bits(bits as u64))
}

/// The bits of a `Float` held in a word compiled code cannot read in place.
pub unsafe extern "C" fn rt_unbox_float(ctx: *mut Ctx, w: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match (w != 0).then(|| ctx.value(w)) {
        Some(Value::Float(f)) => ply_core::float::bits(f) as i64,
        _ => {
            let d = error(format!("a `Float` operation on a {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

/// `a % b` over two `Float`s' bits, which C reaches only through `fmod`.
pub unsafe extern "C" fn rt_float_rem(_ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let (a, b) = (f64::from_bits(a as u64), f64::from_bits(b as u64));
    ply_core::float::bits(a % b) as i64
}

/// The operator codes compiled code hands [`rt_binary`]; `emit.ply`'s `binary_code` must match.
pub(super) const BINOPS: [BinOp; 18] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::Eq,
    BinOp::Ne,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::Concat,
    BinOp::BitAnd,
    BinOp::BitOr,
    BinOp::BitXor,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::Ushr,
];

/// The machine's own negation of a `Float`, a `Decimal` or a width past 32 bits, which compiled
/// code holds as the runtime's own words. Takes it.
pub unsafe extern "C" fn rt_negate(ctx: *mut Ctx, a: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let vals = values_taken(c, &[a]);
    let answer = match &vals[0] {
        v @ Value::Ctor { .. } => {
            match numeric_value(
                ctx.cast(),
                ply_core::instances::NEG,
                std::slice::from_ref(v),
            ) {
                Some(n) => n,
                None => return unsafe { &mut *ctx }.fail(ply_core::unstated(Span::DUMMY, v)),
            }
        }
        Value::Float(f) => Value::Float(-f),
        Value::Decimal(d) => Value::Decimal(-*d),
        Value::Fixed(f) => match f.checked_neg() {
            Some(n) => Value::Fixed(n),
            None => return c.fail(error("negation overflowed its width")),
        },
        other => match other.as_int(Span::DUMMY, "negation") {
            Ok(i) => match i.checked_neg() {
                Some(n) => Value::Int(n),
                None => return c.fail(error("integer overflow in negation")),
            },
            Err(d) => return c.fail(d),
        },
    };
    c.word(&answer)
}

/// `~` over a word the emitter cannot read as an `Int`: a width past 32 bits. Takes it.
pub unsafe extern "C" fn rt_bitnot(ctx: *mut Ctx, a: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let vals = values_taken(c, &[a]);
    let answer = match &vals[0] {
        Value::Fixed(f) => Value::Fixed(ply_core::Fixed::new(f.ty, !f.bits())),
        other => match other.as_int(Span::DUMMY, "`~`") {
            Ok(i) => Value::Int(!i),
            Err(d) => return c.fail(d),
        },
    };
    c.word(&answer)
}

/// The machine's own operator over two words of a `Float`, a `Decimal` or a width past 32 bits.
/// Takes both.
pub unsafe extern "C" fn rt_binary(ctx: *mut Ctx, op: i64, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let Some(op) = usize::try_from(op)
        .ok()
        .and_then(|i| BINOPS.get(i).copied())
    else {
        return ctx.fail(error("an operator this runtime has no code for"));
    };
    let vals = values_taken(ctx, &[a, b]);
    match ply_core::strict_binary(
        op,
        &vals[0],
        &vals[1],
        Span::DUMMY,
        Span::DUMMY,
        Span::DUMMY,
    ) {
        Ok(v) => ctx.word(&v),
        Err(d) if matches!(op, BinOp::Div | BinOp::Rem) && is_zero(&vals[1]) => raise_error(ctx, d),
        Err(d) => ctx.fail(d),
    }
}

/// `rt_binary` over two 64-bit widths' bits, the width at `t` in [`ply_core::INT_TYPES`]: the
/// answer's bits, or a `Bool`'s 1 or 0, raised as the evaluator raises.
pub unsafe extern "C" fn rt_wide_binary(ctx: *mut Ctx, op: i64, t: i64, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (Some(op), Some(ty)) = (
        usize::try_from(op)
            .ok()
            .and_then(|i| BINOPS.get(i).copied()),
        usize::try_from(t)
            .ok()
            .and_then(|i| ply_core::INT_TYPES.get(i).copied()),
    ) else {
        return ctx.fail(error("an operator this runtime has no code for"));
    };
    let of = |w: i64| Value::Fixed(ply_core::Fixed::new(ty, w as u64 as u128));
    let (x, y) = (of(a), of(b));
    match ply_core::strict_binary(op, &x, &y, Span::DUMMY, Span::DUMMY, Span::DUMMY) {
        Ok(Value::Fixed(f)) => f.bits() as u64 as i64,
        Ok(Value::Bool(b)) => i64::from(b),
        Ok(other) => ctx.fail(error(format!(
            "a width's operator answered {}",
            other.type_name()
        ))),
        Err(d) if matches!(op, BinOp::Div | BinOp::Rem) && is_zero(&y) => raise_error(ctx, d),
        Err(d) => ctx.fail(d),
    }
}

/// The object of a 64-bit width, the width at `t` in [`ply_core::INT_TYPES`], holding `bits`.
pub unsafe extern "C" fn rt_box_fixed(ctx: *mut Ctx, bits: i64, t: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let ty = ply_core::INT_TYPES[t as usize];
    ctx.heap
        .boxed_fixed(ply_core::Fixed::new(ty, bits as u64 as u128))
}

/// The `Decimal` two words of compiled code's `PlyDec` hold.
pub(super) fn decimal_of(lo: i64, hi: i64) -> ply_core::Decimal {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&lo.to_le_bytes());
    bytes[8..].copy_from_slice(&hi.to_le_bytes());
    ply_core::Decimal::deserialize(bytes)
}

/// Writes `d`'s two words at `out`, compiled code's `PlyDec`.
pub(super) unsafe fn write_decimal(out: i64, d: ply_core::Decimal) {
    let bytes = d.serialize();
    let words = out as *mut i64;
    unsafe {
        words.write(i64::from_le_bytes(
            bytes[..8].try_into().expect("eight bytes"),
        ));
        words.add(1).write(i64::from_le_bytes(
            bytes[8..].try_into().expect("eight bytes"),
        ));
    }
}

/// The object of the `Decimal` whose two words are `lo` and `hi`.
pub unsafe extern "C" fn rt_box_decimal(ctx: *mut Ctx, lo: i64, hi: i64) -> i64 {
    unsafe { &mut *ctx }.heap.boxed_decimal(decimal_of(lo, hi))
}

/// An arithmetic operator, `op` in `BINOPS`, over two `Decimal`s' words: its answer's two words
/// written at `out`, raised as the evaluator raises.
pub unsafe extern "C" fn rt_decimal_binary(
    ctx: *mut Ctx,
    op: i64,
    a_lo: i64,
    a_hi: i64,
    b_lo: i64,
    b_hi: i64,
    out: i64,
) {
    let ctx = unsafe { &mut *ctx };
    let Some(op) = usize::try_from(op)
        .ok()
        .and_then(|i| BINOPS.get(i).copied())
    else {
        ctx.fail(error("an operator this runtime has no code for"));
        return;
    };
    let (x, y) = (
        Value::Decimal(decimal_of(a_lo, a_hi)),
        Value::Decimal(decimal_of(b_lo, b_hi)),
    );
    match ply_core::strict_binary(op, &x, &y, Span::DUMMY, Span::DUMMY, Span::DUMMY) {
        Ok(Value::Decimal(d)) => unsafe { write_decimal(out, d) },
        Ok(other) => {
            ctx.fail(error(format!(
                "a `Decimal` operator answered {}",
                other.type_name()
            )));
        }
        Err(d) if matches!(op, BinOp::Div | BinOp::Rem) && is_zero(&y) => {
            raise_error(ctx, d);
        }
        Err(d) => {
            ctx.fail(d);
        }
    }
}

/// How two `Decimal`s' words order as `Decimal`s: -1, 0 or 1, equal at any scale that holds the
/// same number.
pub unsafe extern "C" fn rt_decimal_compare(
    _ctx: *mut Ctx,
    a_lo: i64,
    a_hi: i64,
    b_lo: i64,
    b_hi: i64,
) -> i64 {
    decimal_of(a_lo, a_hi).cmp(&decimal_of(b_lo, b_hi)) as i64
}

/// A `Decimal`'s negation, its two words written at `out`.
pub unsafe extern "C" fn rt_decimal_negate(_ctx: *mut Ctx, lo: i64, hi: i64, out: i64) {
    unsafe { write_decimal(out, -decimal_of(lo, hi)) }
}

/// Field `k`, in its records' shape's order, of record `i` of `Columns` `xs`, held once more: a
/// scalar boxed and a record in place the address of its header there. What is not a columnar
/// value's in-range record is read through `Columns::get`, which raises as it does. Borrows `xs`.
pub unsafe extern "C" fn rt_columns_field(ctx: *mut Ctx, xs: i64, i: i64, k: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let tables = Arc::clone(&c.tables);
    let k = k as usize;
    if heap::kind(xs) == KIND_COLUMNS
        && let Some(at) = usize::try_from(i)
            .ok()
            .filter(|at| *at < columns::len(obj(xs)))
    {
        return array::element(
            &mut c.heap,
            &tables.layouts,
            columns::column(obj(xs), k),
            at,
        );
    }
    heap::inc(xs);
    let index = c.heap.boxed_int(i);
    let r = builtin(c, Builtin::ColumnsGet, &[xs, index]);
    if c.failed != 0 {
        return 0;
    }
    let o = obj(r);
    let shape = unsafe { (*o).layout };
    let (at, slot) = (
        tables.layouts.shape_offsets(shape)[k],
        tables.layouts.shape_slots(shape)[k],
    );
    let w = heap::field_word(&mut c.heap, &tables.layouts, o, at, slot);
    heap::dec(r);
    w
}

/// A divisor `/` and `%` raise on rather than overflow.
pub(super) fn is_zero(v: &Value) -> bool {
    match v {
        Value::Int(n) => *n == 0,
        Value::Fixed(f) => f.raw() == 0,
        Value::Decimal(d) => d.is_zero(),
        _ => false,
    }
}

/// Whichever of `checked_mul`, `checked_div` and `checked_rem` the operator was.
pub unsafe extern "C" fn rt_arith(ctx: *mut Ctx, op: i64, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (result, what) = match op {
        0 => (a.checked_mul(b), "multiplication"),
        1 => (a.checked_div(b), "division"),
        _ => (a.checked_rem(b), "remainder"),
    };
    match result {
        Some(n) => n,
        None => {
            // The machine's own words, so a failure reads the same on either engine.
            if op != 0 && b == 0 {
                raise_error(ctx, error(format!("{what} by zero")))
            } else {
                ctx.fail(error(format!("integer overflow in {what}")))
            }
        }
    }
}

/// A literal: the unit's immortal word for it, built once at load.
pub unsafe extern "C" fn rt_lit(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &*ctx };
    ctx.tables.const_words[index as usize]
}

/// A `match` whose arms did not cover the value.
pub unsafe extern "C" fn rt_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("no arm of this `match` matched");
    ctx.fail(d);
}

/// A refutable `let` whose pattern did not match the value bound to it.
pub unsafe extern "C" fn rt_let_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("`let` pattern did not match the bound value");
    raise_error(ctx, d);
}

/// `what` is `emit.ply`'s `overflow_code`.
pub unsafe extern "C" fn rt_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = match what {
        0 => "addition",
        1 => "subtraction",
        2 => "negation",
        3 => "multiplication",
        _ => "division",
    };
    let d = error(format!("integer overflow in {name}"));
    ctx.fail(d);
}

/// An `Int` outside the target width; `which` indexes [`ply_core::INT_TYPES`].
pub unsafe extern "C" fn rt_not_that_width(ctx: *mut Ctx, which: i64, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let t = ply_core::INT_TYPES[which as usize];
    let d = error(format!(
        "`{}` was given {value}: `{t}` holds {} to {}",
        Builtin::of_int(t).name(),
        t.min(),
        t.max()
    ));
    raise_error(ctx, d);
}

/// How far the base of the stack a helper runs on may sit from the one the entry's floor was
/// taken from and still be that stack: a guard's width, and never a segment's.
pub(super) const SAME_STACK: usize = 256 * 1024;

/// A `(ctx, args)` entry the unit states of a type, called with one of its values. Takes `w`;
/// `None` when the call failed, which the context then holds. A value is compared wherever it is
/// read, which may be in frames that grew onto a segment the entry's floor knows nothing of: the
/// call then runs on a stack of its own.
pub(super) fn call_stated(ctx: *mut Ctx, entry: usize, w: Word) -> Option<Word> {
    call_stated_over(ctx, entry, &[w])
}

/// [`call_stated`] for an entry of any number of words. Takes each.
pub(super) fn call_stated_over(ctx: *mut Ctx, entry: usize, args: &[Word]) -> Option<Word> {
    let (floor, site) = {
        let c = unsafe { &*ctx };
        (c.stack_floor, (c.site_root, c.site_start, c.site_end))
    };
    let on_entry_stack = ply_core::limit::stack_base()
        .is_some_and(|base| base.abs_diff(floor.saturating_sub(STACK_MARGIN)) <= SAME_STACK);
    let out = if on_entry_stack {
        // SAFETY: `entry` is a taken root's address, which the unit keeps as long as its tables.
        let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(entry) };
        unsafe { f(ctx, args.as_ptr()) }
    } else {
        unsafe { rt_grow(ctx, entry as i64, args.as_ptr() as i64) }
    };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return None;
    }
    // What reads the value fails where the body that asked stored its site, not inside this call.
    (c.site_root, c.site_start, c.site_end) = site;
    Some(out)
}

/// The context of the unit running on this thread, which a reader holding only its layouts asks.
pub(super) fn running_ctx() -> Option<*mut Ctx> {
    ply_core::instances::running().map(|i| i.unit.cast::<Ctx>())
}

/// A value read through a function its type states, where the unit running does not hold it.
pub(super) fn absent(ctx: &mut Ctx, what: &str, ctor: &Symbol) -> i64 {
    let d = Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a `{ctor}` was read through its type's `{what}`, which this unit does not hold"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault: a unit that holds a type's values takes what the type states");
    ctx.fail(d)
}

/// The keys two constructors' values are compared by, when their type states one: each the
/// caller's. Reads both.
pub(crate) fn keys_of(layouts: &Layouts, a: Word, b: Word) -> Option<(Word, Word)> {
    if !layouts.states_any() {
        return None;
    }
    let ctor = |w: Word| unsafe { (*obj(w)).layout };
    let (ka, kb) = (layouts.key(ctor(a)), layouts.key(ctor(b)));
    if ka == Stated::No || kb == Stated::No {
        return None;
    }
    let ctx = running_ctx()?;
    let (Stated::By(f), Stated::By(g)) = (ka, kb) else {
        absent(
            unsafe { &mut *ctx },
            "key",
            &layouts.ctors[ctor(a) as usize].0,
        );
        return None;
    };
    heap::inc(a);
    let x = call_stated(ctx, f, a)?;
    heap::inc(b);
    match call_stated(ctx, g, b) {
        Some(y) => Some((x, y)),
        None => {
            heap::dec(x);
            None
        }
    }
}

pub(super) fn unit_keys(unit: *mut (), ctor: &Symbol) -> bool {
    let layouts = &unsafe { &*unit.cast::<Ctx>() }.tables.layouts;
    layouts
        .ctor_index(ctor)
        .is_some_and(|i| layouts.key(i) != Stated::No)
}

pub(super) fn stated_value(unit: *mut (), v: &Value, key: bool) -> Option<Value> {
    let ctx = unit.cast::<Ctx>();
    let c = unsafe { &mut *ctx };
    let Value::Ctor { name, .. } = v else {
        return None;
    };
    let index = c.tables.layouts.ctor_index(name)?;
    let (stated, what) = if key {
        (c.tables.layouts.key(index), "key")
    } else {
        (c.tables.layouts.show(index), "show")
    };
    match stated {
        Stated::No => None,
        Stated::Absent => {
            absent(c, what, name);
            None
        }
        Stated::By(entry) => {
            let tables = Arc::clone(&c.tables);
            let w = c.heap.to_word(&tables.layouts, v);
            let out = call_stated(ctx, entry, w)?;
            let answer = Heap::to_value(&tables.layouts, out);
            heap::dec(out);
            Some(answer)
        }
    }
}

pub(super) fn key_value(unit: *mut (), v: &Value) -> Option<Value> {
    stated_value(unit, v, true)
}

/// One of a `numeric`'s functions, for this constructor, called over `args`.
pub(super) fn numeric_call(ctx: *mut Ctx, ctor: u32, role: usize, args: &[Value]) -> Option<Value> {
    let c = unsafe { &mut *ctx };
    match c.tables.layouts.numeric(ctor, role) {
        Stated::No => None,
        Stated::Absent => {
            let name = c.tables.layouts.ctors[ctor as usize].0.clone();
            absent(c, "numeric", &name);
            None
        }
        Stated::By(entry) => {
            let tables = Arc::clone(&c.tables);
            let words: Vec<Word> = args
                .iter()
                .map(|v| c.heap.to_word(&tables.layouts, v))
                .collect();
            let out = call_stated_over(ctx, entry, &words)?;
            let answer = Heap::to_value(&tables.layouts, out);
            heap::dec(out);
            Some(answer)
        }
    }
}

pub(super) fn numeric_value(unit: *mut (), role: usize, args: &[Value]) -> Option<Value> {
    let ctx = unit.cast::<Ctx>();
    let Some(Value::Ctor { name, .. }) = args.first() else {
        return None;
    };
    let ctor = unsafe { &*ctx }.tables.layouts.ctor_index(name)?;
    numeric_call(ctx, ctor, role, args)
}

pub(super) fn stated_witness(unit: *mut (), ctor: &Symbol) -> Option<i64> {
    let layouts = &unsafe { &*unit.cast::<Ctx>() }.tables.layouts;
    let index = layouts.ctor_index(ctor)?;
    (layouts.numeric(index, ply_core::instances::OF_INT) != Stated::No)
        .then(|| ply_core::instances::STATED_WITNESS + i64::from(index))
}

pub(super) fn of_int_value(unit: *mut (), witness: i64, n: i64) -> Option<Value> {
    let ctor = u32::try_from(witness - ply_core::instances::STATED_WITNESS).ok()?;
    numeric_call(
        unit.cast::<Ctx>(),
        ctor,
        ply_core::instances::OF_INT,
        &[Value::Int(n)],
    )
}

pub(super) fn shown_value(unit: *mut (), v: &Value) -> Option<Value> {
    stated_value(unit, v, false)
}

/// `==` beyond two `Int`s or `Bool`s, deferring to the evaluator's comparison. Reads both.
pub unsafe extern "C" fn rt_equal(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    if heap::is_imm(a) && heap::is_imm(b) {
        return i64::from(a == b);
    }
    if !heap::is_imm(a) && !heap::is_imm(b) {
        let (ka, kb) = (heap::kind(a), heap::kind(b));
        if ka == kb && (ka == KIND_STR || ka == KIND_BYTES) {
            return i64::from(unsafe { bytes_of(obj(a)) == bytes_of(obj(b)) });
        }
        // Two values of a keyed type are equal as their keys are.
        if ka == KIND_CTOR && kb == KIND_CTOR && unsafe { &*ctx }.tables.layouts.states_any() {
            let tables = Arc::clone(&unsafe { &*ctx }.tables);
            if let Some((x, y)) = keys_of(&tables.layouts, a, b) {
                let equal = unsafe { rt_equal(ctx, x, y) };
                heap::dec(x);
                heap::dec(y);
                return equal;
            }
        }
    }
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.value(a), ctx.value(b));
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

/// Whether `lo <= v <= hi` as `<=` orders the three, for a range pattern over a width the words do
/// not carry. Reads all three.
pub unsafe extern "C" fn rt_between(ctx: *mut Ctx, v: i64, lo: i64, hi: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (x, l, h) = (ctx.value(v), ctx.value(lo), ctx.value(hi));
    let le = |a: &Value, b: &Value| {
        ply_core::strict_binary(BinOp::Le, a, b, Span::DUMMY, Span::DUMMY, Span::DUMMY)
    };
    match le(&l, &x).and_then(|low| Ok((low, le(&x, &h)?))) {
        Ok(both) => i64::from(matches!(both, (Value::Bool(true), Value::Bool(true)))),
        Err(d) => ctx.fail(d),
    }
}

/// `++`: two strings or two byte strings append natively, answering the kind they share; anything
/// else raises what `strict_binary` raises. Takes both.
pub unsafe extern "C" fn rt_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let kind = heap::kind(a);
    if kind == heap::kind(b) && (kind == KIND_STR || kind == KIND_BYTES) {
        let out = ctx.heap.append(a, unsafe { bytes_of(obj(b)) });
        heap::dec(b);
        return out;
    }
    let (l, r) = (ctx.value(a), ctx.value(b));
    heap::dec(a);
    heap::dec(b);
    match ply_core::strict_binary(BinOp::Concat, &l, &r, Span::DUMMY, Span::DUMMY, Span::DUMMY) {
        Ok(v) => ctx.word(&v),
        Err(d) => ctx.fail(d),
    }
}
