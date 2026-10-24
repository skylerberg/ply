//! The builtins compiled code calls, each through an entry of its own, and their bodies.

use super::*;

/// `Word`, once for each name it is given: a native's parameters as its type spells them.
macro_rules! word {
    ($param:ident) => {
        Word
    };
}

macro_rules! natives {
    ($($variant:ident $name:literal ($($param:ident),*) $answer:ident;)*) => {
        /// The type of the function compiled code calls each builtin through.
        mod native_type {
            use super::{Ctx, Raising, Word};
            $( pub type $variant = unsafe extern "C" fn(*mut Ctx $(, word!($param))*) -> $answer; )*
        }

        /// Each builtin as compiled code calls it: its words in, each one the callee's, and its
        /// answer out.
        #[allow(non_snake_case)]
        mod called {
            use super::{Answered, Builtin, Ctx, Raising, Word, builtin};
            $(
                pub unsafe extern "C" fn $variant(ctx: *mut Ctx $(, $param: Word)*) -> $answer {
                    <$answer as Answered>::of(builtin(
                        unsafe { &mut *ctx },
                        Builtin::$variant,
                        &[$($param),*],
                    ))
                }
            )*
        }

        /// What compiled code calls `b` through takes and answers, as that function's type says.
        pub fn signature(b: Builtin) -> Signature {
            match b {
                $( Builtin::$variant => <native_type::$variant as Native>::SIGNATURE, )*
            }
        }

        /// What a unit binds a builtin's helper to: the builtin's own road past the dispatch where
        /// it has one, else its call through [`builtin`]. Either has the type [`signature`] reads.
        pub fn builtin_address(b: Builtin) -> *const () {
            road(b).unwrap_or_else(|| match b {
                $( Builtin::$variant => <native_type::$variant as Native>::address(called::$variant), )*
            })
        }
    };
}

ply_core::each_builtin!(natives);

/// A builtin's own road past the dispatch, which must have the type of its call through
/// [`builtin`].
pub(super) fn road(b: Builtin) -> Option<*const ()> {
    Some(match b {
        Builtin::ByteOfInt => <native_type::ByteOfInt as Native>::address(rt_byte_of_int),
        _ => return None,
    })
}

/// Every call of a builtin, whether named or called through a value: natively over words where it
/// can be, else over the values they denote. Takes the arguments.
pub(super) fn builtin(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    if !ctx.sims.is_empty()
        && let Some(access) = crate::simulate::shared_access(ctx, b, args)
    {
        ctx.record_access(access);
    }
    match native_builtin(ctx, b, args) {
        Some(w) => w,
        None => builtin_over_values(ctx, b, args),
    }
}

/// `b` over the values the words denote. Takes the arguments.
pub(super) fn builtin_over_values(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    let values = values_taken(ctx, args);
    match called_over_values(ctx, b, values) {
        Some(v) => ctx.word(&v),
        None => 0,
    }
}

/// `b` over `values`: its answer, or `None` where it raised, as it raises, or failed the context.
pub(super) fn called_over_values(ctx: &mut Ctx, b: Builtin, values: Vec<Value>) -> Option<Value> {
    let site = ctx.site();
    let panicked = match (b, values.first()) {
        (Builtin::Panic, Some(Value::Str(s))) => Some(s.to_string()),
        _ => None,
    };
    // An operator over an `integer` parameter raises for a zero divisor, as one written at its
    // type does; its overflow, like theirs, ends the run.
    let divides_by_zero = match (b, values.as_slice()) {
        (Builtin::NumericBinary, [Value::Int(op), _, _, divisor]) => {
            matches!(
                usize::try_from(*op)
                    .ok()
                    .and_then(|i| ply_core::builtins::NUMERIC_OPS.get(i)),
                Some(BinOp::Div | BinOp::Rem)
            ) && is_zero(divisor)
        }
        _ => false,
    };
    match ply_core::builtins::call(b, values, site) {
        Ok(v) => Some(v),
        Err(d) if signature(b).raises || divides_by_zero => {
            let message = panicked.unwrap_or_else(|| raised_message(&d));
            ctx.raise(d, message);
            None
        }
        Err(d) => {
            ctx.fail(d);
            None
        }
    }
}

/// A runtime error's message as a clause for `abort.raise` is given it, each value the text names
/// told by its kind: nothing in Rust renders one.
pub(super) fn raised_message(d: &Diagnostic) -> String {
    d.values
        .iter()
        .enumerate()
        .fold(d.message.clone(), |text, (i, v)| {
            text.replace(&ply_core::slot(i), v.describe())
        })
}

/// [`Ctx::raise`] for a runtime error that is the program's to answer.
pub(crate) fn raise_error(ctx: &mut Ctx, d: Diagnostic) -> i64 {
    let message = raised_message(&d);
    ctx.raise(d, message)
}

/// A witnessed operator over two immediates, when the witness is `Int` or a width compiled code
/// holds as its `Int`: the answer, or `None` for the value path to give, or to raise, instead.
pub(super) fn narrow_binary(op: i64, w: i64, a: i64, b: i64) -> Option<Word> {
    let range = if w == ply_core::builtins::INT_WITNESS {
        None
    } else {
        let t = *ply_core::INT_TYPES.get(usize::try_from(w).ok()?)?;
        if t.bits() >= 64 {
            return None;
        }
        Some((
            i64::try_from(ply_core::IntTy::min(t)).ok()?,
            i64::try_from(ply_core::IntTy::max(t)).ok()?,
        ))
    };
    let op = ply_core::builtins::NUMERIC_OPS.get(usize::try_from(op).ok()?)?;
    let n = match op {
        BinOp::Add => a.checked_add(b)?,
        BinOp::Sub => a.checked_sub(b)?,
        BinOp::Mul => a.checked_mul(b)?,
        BinOp::Div => a.checked_div(b)?,
        BinOp::Rem => a.checked_rem(b)?,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Lt => return Some(heap::bool(a < b)),
        BinOp::Le => return Some(heap::bool(a <= b)),
        BinOp::Gt => return Some(heap::bool(a > b)),
        BinOp::Ge => return Some(heap::bool(a >= b)),
        _ => return None,
    };
    match range {
        Some((lo, hi)) if n < lo || n > hi => None,
        _ => heap::fits_imm(n).then(|| heap::imm(n)),
    }
}

/// The entry's next hold, holding `held`.
pub(super) fn hold_new(c: &mut Ctx, held: Word) -> Word {
    c.holds_made += 1;
    hold_word(&mut c.heap, Some(held), c.holds_made)
}

/// A hold of `held`, which it takes.
pub unsafe extern "C" fn rt_hold_made(ctx: *mut Ctx, held: Handed) -> crate::emitted::Handle {
    let c = unsafe { &mut *ctx };
    crate::emitted::Handle::of(hold_new(c, held.word()))
}

/// What the lent hold `h` holds, lent from it: whoever reads it counts it as its type does. Fails
/// the entry where the hold has let go.
pub unsafe extern "C" fn rt_hold_read(ctx: *mut Ctx, h: crate::emitted::Handle) -> Handed {
    let c = unsafe { &mut *ctx };
    Handed::of(held_in(c, &h, Mode::Read).map_or(0, |o| unsafe { word_at(o, HOLD_HELD) }))
}

/// What the lent hold `h` holds, taken out of it, which lets it go. Fails the entry where the hold
/// has let go already.
pub unsafe extern "C" fn rt_hold_taken(ctx: *mut Ctx, h: crate::emitted::Handle) -> Handed {
    let c = unsafe { &mut *ctx };
    Handed::of(held_in(c, &h, Mode::Write).map_or(0, |o| unsafe {
        let held = word_at(o, HOLD_HELD);
        set_word(o, HOLD_HELD, 0);
        set_word(o, HOLD_LET_GO, heap::imm(1));
        held
    }))
}

/// The object of hold `h`, a read of it (`mode`) noted for a simulation; none where it has let go,
/// or is no hold, which fails the entry.
fn held_in(c: &mut Ctx, h: &crate::emitted::Handle, mode: Mode) -> Option<*mut heap::Obj> {
    let Some(o) = hold_object(h.word()) else {
        c.fail(
            error("a hold that is no runtime hold").note("this is Ply's fault, not the program's"),
        );
        return None;
    };
    if !c.sims.is_empty()
        && let Some(id) = hold_id(h.word())
    {
        c.record_access(Access::Hold { id, mode });
    }
    if hold_let_go(o) {
        let site = c.site();
        c.fail(hold_released(
            site,
            if mode == Mode::Read { "read" } else { "let go" },
        ));
        return None;
    }
    Some(o)
}

/// The payload word a hold keeps what it holds in, which emitted code reads and empties itself.
pub const HOLD_HELD: usize = CLOSURE_CAPTURES;

/// The word a hold marks itself let go in, apart from what it holds, whose word may be 0 itself (a
/// `()`, or a `None` held as one pointer).
const HOLD_LET_GO: usize = CLOSURE_CAPTURES + 2;

/// A hold, as a closure no call enters: what it holds in its first capture, emptied once it has let
/// go; `id`, its number in the entry, in its second, which names it to the scheduler; and in its
/// third whether it has let go, as one made with nothing held has, so a second release or a read
/// after is refused. A continuation resumed twice shares it.
pub(crate) fn hold_word(heap: &mut Heap, held: Option<Word>, id: u64) -> Word {
    let o = heap.alloc(KIND_CLOSURE, 0, (3 + CLOSURE_CAPTURES) as u32, 0);
    unsafe {
        set_word(o, CLOSURE_CODE, rt_hold_entry as *const () as usize as Word);
        set_word(o, HOLD_HELD, held.unwrap_or(0));
        set_word(o, CLOSURE_CAPTURES + 1, heap::imm(id as i64));
        set_word(o, HOLD_LET_GO, heap::imm(i64::from(held.is_none())));
    }
    o as Word
}

/// Whether the hold `o` has let go of what it held.
pub(crate) fn hold_let_go(o: *mut heap::Obj) -> bool {
    heap::imm_value(unsafe { word_at(o, HOLD_LET_GO) }) != 0
}

/// The object a hold is, where `w` is one.
pub(super) fn hold_object(w: Word) -> Option<*mut heap::Obj> {
    let entry = rt_hold_entry as *const () as usize as Word;
    (heap::kind(w) == KIND_CLOSURE && unsafe { word_at(obj(w), CLOSURE_CODE) } == entry)
        .then(|| obj(w))
}

/// A hold's number in its entry, where `w` is a hold.
pub(crate) fn hold_id(w: Word) -> Option<u64> {
    hold_object(w).map(|o| heap::imm_value(unsafe { word_at(o, CLOSURE_CAPTURES + 1) }) as u64)
}

/// The entry of a hold, which nothing calls: a hold is no function.
pub(super) unsafe extern "C" fn rt_hold_entry(ctx: *mut Ctx, _args: *const i64) -> i64 {
    let d = Diagnostic::error(codes::INTERNAL_ERROR, "a hold was called as a function")
        .note("this is Ply's fault, not the program's");
    unsafe { &mut *ctx }.fail(d)
}

/// A handler's variables read where no clause of a handler that has some runs: the lowering of
/// `var` writes the reads inside clauses alone, so this is Ply's defect.
#[cold]
#[inline(never)]
pub(super) fn no_handler_vars() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a handler's variable was read where no clause of its handler runs",
    )
    .note("this is Ply's fault, not the program's")
}

/// The builtins answered over native words; `None` answers the call over values instead.
pub(super) fn native_builtin(ctx: &mut Ctx, which: Builtin, args: &[Word]) -> Option<Word> {
    match (which, args) {
        (Builtin::Len, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let n = unsafe { (*obj(*xs)).len } as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::WithCapacity, [n]) if heap::is_imm(*n) => {
            let room = heap::imm_value(*n).clamp(0, crate::list::WIDTH as i64) as u32;
            Some(ctx.heap.alloc_list(room) as Word)
        }
        (Builtin::ListLeafOf, [xs, j]) if heap::kind(*xs) == KIND_LIST && heap::is_imm(*j) => {
            let leaf = list::leaf_of(obj(*xs), heap::imm_value(*j));
            if leaf != 0 {
                heap::inc(leaf);
            }
            heap::dec(*xs);
            Some(leaf)
        }
        (Builtin::HoldGet, [h]) => {
            let o = hold_object(*h)?;
            let held = unsafe { word_at(o, HOLD_HELD) };
            let answer = if hold_let_go(o) {
                let site = ctx.site();
                ctx.fail(hold_released(site, "read"))
            } else {
                heap::inc(held);
                held
            };
            heap::dec(*h);
            Some(answer)
        }
        (Builtin::HoldTake, [h]) => {
            let o = hold_object(*h)?;
            let held = unsafe { word_at(o, HOLD_HELD) };
            let answer = if hold_let_go(o) {
                let site = ctx.site();
                ctx.fail(hold_released(site, "let go"))
            } else {
                unsafe {
                    set_word(o, HOLD_HELD, 0);
                    set_word(o, HOLD_LET_GO, heap::imm(1));
                }
                held
            };
            heap::dec(*h);
            Some(answer)
        }
        (Builtin::HoldNew, [v]) => Some(hold_new(ctx, *v)),
        (Builtin::HandlerInit, [i, w]) => {
            let at = heap::imm_value(*i) as usize;
            let c = ctx.current;
            let Some(f) = ctx.stacks[c].list.last_mut() else {
                heap::dec(*w);
                return Some(ctx.fail(no_handler_vars()));
            };
            f.vars
                .get_or_insert_with(|| shared(HandlerVars(Vec::new())))
                .borrow_mut()
                .set(at, Var::Word(*w));
            Some(heap::unit())
        }
        (Builtin::HandlerVar, [i]) => {
            let at = heap::imm_value(*i) as usize;
            let Some((_, v)) = ctx.stacks[ctx.current].held.last() else {
                return Some(ctx.fail(no_handler_vars()));
            };
            Some(v.borrow().get(at))
        }
        (Builtin::HandlerSet, [i, w]) => {
            let at = heap::imm_value(*i) as usize;
            let Some((_, v)) = ctx.stacks[ctx.current].held.last() else {
                heap::dec(*w);
                return Some(ctx.fail(no_handler_vars()));
            };
            v.borrow_mut().set(at, Var::Word(*w));
            Some(heap::unit())
        }
        (Builtin::ArrayLen, [xs]) if heap::kind(*xs) == KIND_ARRAY => {
            let n = array::len(obj(*xs)) as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::ArrayGet, [xs, i]) if heap::kind(*xs) == KIND_ARRAY => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            if index >= array::len(obj(*xs)) {
                return None;
            }
            let tables = Arc::clone(&ctx.tables);
            let item = array::element(&mut ctx.heap, &tables.layouts, obj(*xs), index);
            heap::dec(*xs);
            Some(item)
        }
        (Builtin::ColumnsOfList, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let items = list::to_vec(obj(*xs));
            for w in &items {
                heap::inc(*w);
            }
            let tables = Arc::clone(&ctx.tables);
            match ctx.heap.columns_of(&tables.layouts, &items) {
                Some(c) => {
                    heap::dec(*xs);
                    Some(c)
                }
                None => {
                    for w in &items {
                        heap::dec(*w);
                    }
                    None
                }
            }
        }
        (Builtin::ColumnsNew, [n, x]) if heap::kind(*x) == KIND_RECORD => {
            let n = heap::as_int(*n)?;
            if !(1..=ply_core::builtins::MAX_ARRAY_LEN).contains(&n) {
                return None;
            }
            let items = vec![*x; n as usize];
            for _ in 1..n {
                heap::inc(*x);
            }
            let tables = Arc::clone(&ctx.tables);
            let made = ctx.heap.columns_of(&tables.layouts, &items);
            if made.is_none() {
                for w in &items[1..] {
                    heap::dec(*w);
                }
            }
            made
        }
        (Builtin::ColumnsLen, [xs]) if heap::kind(*xs) == KIND_COLUMNS => {
            let n = columns::len(obj(*xs)) as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::ColumnsGet, [xs, i]) if heap::kind(*xs) == KIND_COLUMNS => {
            let at = usize::try_from(heap::as_int(*i)?)
                .ok()
                .filter(|at| *at < columns::len(obj(*xs)))?;
            let tables = Arc::clone(&ctx.tables);
            let r = columns::record(&mut ctx.heap, &tables.layouts, obj(*xs), at);
            heap::dec(*xs);
            Some(r)
        }
        (Builtin::ColumnsAt, [xs, i]) if heap::kind(*xs) == KIND_COLUMNS => {
            let index = heap::as_int(*i)?;
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let tables = Arc::clone(&ctx.tables);
            let held = usize::try_from(index)
                .ok()
                .filter(|at| *at < columns::len(obj(*xs)))
                .map(|at| columns::record(&mut ctx.heap, &tables.layouts, obj(*xs), at));
            let answer = match held {
                Some(item) => {
                    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                    unsafe { set_word(c, 0, item) };
                    c as Word
                }
                None => ctx.nullary(none),
            };
            heap::dec(*xs);
            Some(answer)
        }
        (Builtin::ColumnsSet, [xs, i, v])
            if heap::kind(*xs) == KIND_COLUMNS
                && heap::kind(*v) == KIND_RECORD
                && unsafe { (*obj(*v)).layout == (*obj(*xs)).layout } =>
        {
            let at = usize::try_from(heap::as_int(*i)?)
                .ok()
                .filter(|at| *at < columns::len(obj(*xs)))?;
            let tables = Arc::clone(&ctx.tables);
            Some(ctx.heap.columns_set(&tables.layouts, *xs, at, *v))
        }
        (Builtin::ArrayAt, [xs, i]) if heap::kind(*xs) == KIND_ARRAY => {
            let index = heap::as_int(*i)?;
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let tables = Arc::clone(&ctx.tables);
            let held = usize::try_from(index)
                .ok()
                .filter(|at| *at < array::len(obj(*xs)))
                .map(|at| array::element(&mut ctx.heap, &tables.layouts, obj(*xs), at));
            let answer = match held {
                Some(item) => {
                    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                    unsafe { set_word(c, 0, item) };
                    c as Word
                }
                None => ctx.nullary(none),
            };
            heap::dec(*xs);
            Some(answer)
        }
        (Builtin::ArraySet, [xs, i, v]) if heap::kind(*xs) == KIND_ARRAY => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            if index >= array::len(obj(*xs)) {
                return None;
            }
            let tables = Arc::clone(&ctx.tables);
            Some(ctx.heap.array_set(&tables.layouts, *xs, index, *v))
        }
        (Builtin::ArrayNew, [n, x]) => {
            let n = heap::as_int(*n)?;
            if !(0..=ply_core::builtins::MAX_ARRAY_LEN).contains(&n) {
                return None;
            }
            let tables = Arc::clone(&ctx.tables);
            Some(ctx.heap.array_new(&tables.layouts, n as usize, *x))
        }
        (Builtin::ArrayOfList, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let items = list::to_vec(obj(*xs));
            for w in &items {
                heap::inc(*w);
            }
            heap::dec(*xs);
            let tables = Arc::clone(&ctx.tables);
            Some(ctx.heap.array_of(&tables.layouts, &items))
        }
        (Builtin::NumericBinary, [op, w, x, y])
            if heap::is_int(*op) && heap::is_int(*w) && heap::is_int(*x) && heap::is_int(*y) =>
        {
            narrow_binary(
                heap::imm_value(*op),
                heap::imm_value(*w),
                heap::imm_value(*x),
                heap::imm_value(*y),
            )
        }
        (Builtin::Min | Builtin::Max, [x, y]) if heap::is_int(*x) && heap::is_int(*y) => {
            let (a, b) = (heap::imm_value(*x), heap::imm_value(*y));
            Some(heap::imm(if (a <= b) == (which == Builtin::Min) {
                a
            } else {
                b
            }))
        }
        (Builtin::Range, [lo, hi]) => {
            let (a, b) = (heap::as_int(*lo)?, heap::as_int(*hi)?);
            if b - a > (1 << 20) {
                return None;
            }
            let items: Vec<Word> = (a..b).map(heap::imm).collect();
            Some(ctx.heap.list_from(&items))
        }
        // Anything the value builtins would raise on answers `None` before touching a count.
        (Builtin::BytesLen, [b]) if heap::kind(*b) == KIND_BYTES => {
            let n = unsafe { (*obj(*b)).len } as i64;
            heap::dec(*b);
            Some(heap::imm(n))
        }
        (Builtin::BytesAt, [b, i]) if heap::kind(*b) == KIND_BYTES => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            let byte = *unsafe { bytes_of(obj(*b)) }.get(index)?;
            heap::dec(*b);
            Some(heap::imm(i64::from(byte)))
        }
        (Builtin::BytesU32Le, [b, i]) if heap::kind(*b) == KIND_BYTES => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            let four = unsafe { bytes_of(obj(*b)) }.get(index..index + 4)?;
            let w = u32::from_le_bytes(four.try_into().ok()?);
            heap::dec(*b);
            Some(heap::imm(i64::from(w)))
        }
        (Builtin::BytesSlice, [b, s, e]) if heap::kind(*b) == KIND_BYTES => {
            let (start, end) = slice_range(*s, *e, unsafe { (*obj(*b)).len } as usize)?;
            let tables = Arc::clone(&ctx.tables);
            Some(ctx.heap.slice(&tables.layouts, *b, start, end))
        }
        (Builtin::BytesBlake3, [b]) if heap::kind(*b) == KIND_BYTES => {
            let digest = blake3::hash(unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(ctx.heap.bytes(digest.as_bytes()))
        }
        (Builtin::BytesConcat, [a, b])
            if heap::kind(*a) == KIND_BYTES && heap::kind(*b) == KIND_BYTES =>
        {
            let out = ctx.heap.append(*a, unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesConcatAll, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let mut total = 0;
            let mut all_bytes = true;
            list::for_each(o, &mut |w| {
                if heap::kind(w) != KIND_BYTES {
                    all_bytes = false;
                } else {
                    total += unsafe { (*obj(w)).len } as usize;
                }
            });
            if !all_bytes {
                return None;
            }
            // A unique first piece of a unique list grows in place.
            if heap::is_unique(*xs) {
                let items = list::to_vec(o);
                if let Some(&first) = items.first()
                    && heap::grows_in_place(first)
                {
                    for &w in &items {
                        heap::inc(w);
                    }
                    heap::dec(*xs);
                    let mut out = first;
                    for &w in &items[1..] {
                        out = ctx.heap.append(out, unsafe { bytes_of(obj(w)) });
                        heap::dec(w);
                    }
                    return Some(out);
                }
            }
            if total <= 1 {
                let mut one = None;
                list::for_each(o, &mut |w| {
                    one = one.or(unsafe { bytes_of(obj(w)) }.first().copied());
                });
                heap::dec(*xs);
                return Some(ctx.heap.bytes(one.as_slice()));
            }
            let out = ctx.heap.alloc_bytes(KIND_BYTES, total as u32);
            let mut at = 0;
            list::for_each(o, &mut |w| {
                let piece = unsafe { bytes_of(obj(w)) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        piece.as_ptr(),
                        heap::bytes_ptr(out).add(at),
                        piece.len(),
                    );
                }
                at += piece.len();
            });
            unsafe { (*out).len = total as u32 };
            heap::dec(*xs);
            Some(out as Word)
        }
        (Builtin::ByteOfInt, [n]) => {
            let byte = u8::try_from(heap::as_int(*n)?).ok()?;
            Some(ctx.tables.byte(byte))
        }
        (Builtin::BytesOfString, [s]) if heap::kind(*s) == KIND_STR => {
            let out = ctx.heap.bytes(unsafe { bytes_of(obj(*s)) });
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringOfBytes, [b]) if heap::kind(*b) == KIND_BYTES => {
            let text = std::str::from_utf8(unsafe { bytes_of(obj(*b)) }).ok()?;
            let out = ctx.heap.str(text);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesIndexOf, [hay, needle])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*needle) == KIND_BYTES =>
        {
            let at = find_bytes(
                unsafe { bytes_of(obj(*hay)) },
                unsafe { bytes_of(obj(*needle)) },
                0,
            );
            let out = position(ctx, at);
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfFrom, [hay, needle, from])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*needle) == KIND_BYTES =>
        {
            let h = unsafe { bytes_of(obj(*hay)) };
            let from = usize::try_from(heap::as_int(*from)?).ok()?;
            if from > h.len() {
                return None;
            }
            let at = find_bytes(h, unsafe { bytes_of(obj(*needle)) }, from);
            let out = position(ctx, at);
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfByte, [hay, byte]) if heap::kind(*hay) == KIND_BYTES => {
            let byte = u8::try_from(heap::as_int(*byte)?).ok()?;
            let at = memchr::memchr(byte, unsafe { bytes_of(obj(*hay)) });
            let out = position(ctx, at);
            heap::dec(*hay);
            Some(out)
        }
        (Builtin::BytesStartsWith | Builtin::BytesEndsWith, [a, b])
            if heap::kind(*a) == KIND_BYTES && heap::kind(*b) == KIND_BYTES =>
        {
            let (x, y) = unsafe { (bytes_of(obj(*a)), bytes_of(obj(*b))) };
            let answer = if which == Builtin::BytesStartsWith {
                x.starts_with(y)
            } else {
                x.ends_with(y)
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(heap::bool(answer))
        }
        (Builtin::StringConcat, [a, b])
            if heap::kind(*a) == KIND_STR && heap::kind(*b) == KIND_STR =>
        {
            let out = ctx.heap.append(*a, unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(out)
        }
        (Builtin::StringLen, [s]) if heap::kind(*s) == KIND_STR => {
            let n = unsafe { str_of(obj(*s)) }.chars().count() as i64;
            heap::dec(*s);
            Some(heap::imm(n))
        }
        (Builtin::IntToString, [n]) => {
            let text = heap::as_int(*n)?.to_string();
            Some(ctx.heap.str(&text))
        }
        (Builtin::StringStartsWith | Builtin::StringEndsWith | Builtin::StringContains, [a, b])
            if heap::kind(*a) == KIND_STR && heap::kind(*b) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*a)), str_of(obj(*b))) };
            let answer = match which {
                Builtin::StringStartsWith => x.starts_with(y),
                Builtin::StringEndsWith => x.ends_with(y),
                _ => x.contains(y),
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(heap::bool(answer))
        }
        (Builtin::Len, [s]) if heap::kind(*s) == KIND_STR => {
            let n = unsafe { str_of(obj(*s)) }.chars().count() as i64;
            heap::dec(*s);
            Some(heap::imm(n))
        }
        // Within `max` bytes of `from`: the first byte in (or off) the class, or the window's end.
        (Builtin::BytesScan | Builtin::BytesScanUntil, [hay, from, members, max])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*members) == KIND_BYTES =>
        {
            let h = unsafe { bytes_of(obj(*hay)) };
            let from = usize::try_from(heap::as_int(*from)?).ok()?;
            if from > h.len() {
                return None;
            }
            let max = usize::try_from(heap::as_int(*max)?).ok()?;
            let window = &h[from..h.len().min(from.saturating_add(max))];
            let want = which == Builtin::BytesScanUntil;
            let found = match (want, unsafe { bytes_of(obj(*members)) }) {
                (true, []) => None,
                (true, [a]) => memchr::memchr(*a, window),
                (true, [a, b]) => memchr::memchr2(*a, *b, window),
                (true, [a, b, c]) => memchr::memchr3(*a, *b, *c, window),
                (_, set) => {
                    let mut bits = [0u64; 4];
                    for &b in set {
                        bits[usize::from(b >> 6)] |= 1 << (b & 63);
                    }
                    window
                        .iter()
                        .position(|&b| (bits[usize::from(b >> 6)] >> (b & 63) & 1 == 1) == want)
                }
            };
            let at = match found {
                Some(at) => from + at,
                None => from + window.len(),
            };
            heap::dec(*hay);
            heap::dec(*members);
            Some(heap::imm(at as i64))
        }
        (Builtin::BytesIsUtf8, [b]) if heap::kind(*b) == KIND_BYTES => {
            let ok = std::str::from_utf8(unsafe { bytes_of(obj(*b)) }).is_ok();
            heap::dec(*b);
            Some(heap::bool(ok))
        }
        (Builtin::StringOfBytesLossy, [b]) if heap::kind(*b) == KIND_BYTES => {
            let text = String::from_utf8_lossy(unsafe { bytes_of(obj(*b)) });
            let out = ctx.heap.str(&text);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesSplit, [b, sep])
            if heap::kind(*b) == KIND_BYTES && heap::kind(*sep) == KIND_BYTES =>
        {
            let (x, y) = unsafe { (bytes_of(obj(*b)), bytes_of(obj(*sep))) };
            if y.is_empty() {
                return None;
            }
            let mut pieces = Vec::new();
            let mut at = 0;
            for found in memchr::memmem::find_iter(x, y) {
                pieces.push(ctx.heap.bytes(&x[at..found]));
                at = found + y.len();
            }
            pieces.push(ctx.heap.bytes(&x[at..]));
            let out = list_of(ctx, &pieces);
            heap::dec(*b);
            heap::dec(*sep);
            Some(out)
        }
        (Builtin::StringSlice, [s, start, end]) if heap::kind(*s) == KIND_STR => {
            let text = unsafe { str_of(obj(*s)) };
            let chars = text.chars().count();
            let (from, to) = slice_range(*start, *end, chars)?;
            let (from, to) = (char_offset(text, from), char_offset(text, to));
            let out = ctx.heap.str(&text[from..to]);
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringSplit, [s, sep])
            if heap::kind(*s) == KIND_STR && heap::kind(*sep) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*s)), str_of(obj(*sep))) };
            if y.is_empty() {
                return None;
            }
            let pieces: Vec<Word> = x.split(y).map(|piece| ctx.heap.str(piece)).collect();
            let out = list_of(ctx, &pieces);
            heap::dec(*s);
            heap::dec(*sep);
            Some(out)
        }
        (Builtin::StringTrim | Builtin::StringLower | Builtin::StringUpper, [s])
            if heap::kind(*s) == KIND_STR =>
        {
            let text = unsafe { str_of(obj(*s)) };
            let out = match which {
                Builtin::StringTrim => ctx.heap.str(text.trim()),
                Builtin::StringLower => ctx.heap.str(&text.to_lowercase()),
                _ => ctx.heap.str(&text.to_uppercase()),
            };
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringFind, [s, needle])
            if heap::kind(*s) == KIND_STR && heap::kind(*needle) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*s)), str_of(obj(*needle))) };
            // Absent is the value builtins' diagnostic to raise.
            let at = x.find(y)?;
            let n = x[..at].chars().count() as i64;
            heap::dec(*s);
            heap::dec(*needle);
            Some(heap::imm(n))
        }
        (Builtin::Compare | Builtin::CompareValues, [a, b])
            if heap::native_key(&ctx.tables.layouts, *a)
                && heap::native_key(&ctx.tables.layouts, *b) =>
        {
            let layouts = &ctx.tables.layouts;
            let index = match heap::cmp_words(layouts, *a, *b) {
                std::cmp::Ordering::Less => layouts.less?,
                std::cmp::Ordering::Equal => layouts.equal?,
                std::cmp::Ordering::Greater => layouts.greater?,
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(ctx.nullary(index))
        }
        (Builtin::MapNew, []) => Some(ctx.tables.empty_map),
        (Builtin::MapLen, [m]) if heap::kind(*m) == KIND_MAP => {
            let n = unsafe { (*obj(*m)).len } as i64;
            heap::dec(*m);
            Some(heap::imm(n))
        }
        // At `Int`, which is all a call from compiled code holds as a native integer: a narrower
        // width is computed in place, and a 64-bit or wider one is no integer word.
        (Builtin::WrapAdd | Builtin::WrapSub | Builtin::WrapMul, [x, y]) => {
            let (a, b) = (heap::as_int(*x)?, heap::as_int(*y)?);
            let n = match which {
                Builtin::WrapAdd => a.wrapping_add(b),
                Builtin::WrapSub => a.wrapping_sub(b),
                _ => a.wrapping_mul(b),
            };
            heap::dec(*x);
            heap::dec(*y);
            Some(ctx.heap.boxed_int(n))
        }
        (Builtin::Rotr | Builtin::Rotl, [x, n]) => {
            let (a, turn) = (heap::as_int(*x)?, heap::as_int(*n)?);
            let k = turn.rem_euclid(64) as u32;
            let right = if which == Builtin::Rotl {
                (64 - k) % 64
            } else {
                k
            };
            heap::dec(*x);
            heap::dec(*n);
            Some(ctx.heap.boxed_int((a as u64).rotate_right(right) as i64))
        }
        (Builtin::Rotr32, [x, n]) => {
            let (a, turn) = (heap::as_int(*x)?, heap::as_int(*n)?);
            heap::dec(*x);
            heap::dec(*n);
            Some(heap::imm(i64::from(
                (a as u32).rotate_right((turn & 31) as u32),
            )))
        }
        _ => None,
    }
}

/// A slicing builtin's half-open range over `len`, never clamped: out of range is `None`.
pub(super) fn slice_range(start: Word, end: Word, len: usize) -> Option<(usize, usize)> {
    let (start, end) = (heap::as_int(start)?, heap::as_int(end)?);
    if start < 0 || end < start || !usize::try_from(end).is_ok_and(|e| e <= len) {
        return None;
    }
    Some((start as usize, end as usize))
}

/// Where `needle` first occurs in `hay` at or after `from`; an empty needle occurs at `from`.
pub(super) fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    memchr::memmem::find(&hay[from..], needle).map(|at| from + at)
}

/// The byte offset of the `n`-th character boundary, as the value builtins' `char_offset` finds it.
pub(super) fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .nth(n)
        .unwrap_or(s.len())
}

/// A list of `items`, which it takes.
pub(super) fn list_of(ctx: &mut Ctx, items: &[Word]) -> Word {
    ctx.heap.list_from(items)
}

/// `Some(at)` or `None`, built over values when the unit does not know the prelude's constructors.
pub(super) fn position(ctx: &mut Ctx, at: Option<usize>) -> Word {
    let layouts = &ctx.tables.layouts;
    match (at, layouts.some, layouts.none) {
        (Some(i), Some(some), _) => {
            let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
            unsafe { set_word(c, 0, heap::imm(i as i64)) };
            c as Word
        }
        (None, _, Some(none)) => ctx.nullary(none),
        (Some(i), None, _) => ctx.word(&Value::ctor("Some", vec![Value::Int(i as i64)])),
        (None, _, None) => ctx.word(&Value::ctor("None", Vec::new())),
    }
}
