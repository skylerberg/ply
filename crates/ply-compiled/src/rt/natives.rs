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

        /// What builtin `b` takes and answers, as its native's type says.
        pub fn signature(b: Builtin) -> Signature {
            match b {
                $( Builtin::$variant => <native_type::$variant as Native>::SIGNATURE, )*
            }
        }
    };
}

ply_core::each_builtin!(natives);

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

/// The payload bytes of a hold's block.
pub(crate) const HOLD_BYTES: usize = 8 * (3 + CLOSURE_CAPTURES);

/// A hold, as a closure no call enters: what it holds in its first capture, emptied once it has let
/// go; `id`, its number in the entry, in its second, which names it to the scheduler; and in its
/// third whether it has let go, as one made with nothing held has, so a second release or a read
/// after is refused. A continuation resumed twice shares it.
pub(crate) fn hold_word(heap: &mut Heap, held: Option<Word>, id: u64) -> Word {
    let o = heap.raw_alloc(HOLD_BYTES);
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

/// The block a hold is, where `w` is one: its first word names the entry no call enters.
pub(super) fn hold_object(w: Word) -> Option<*mut heap::Obj> {
    let entry = rt_hold_entry as *const () as usize as Word;
    (heap::is_object(w) && unsafe { word_at(obj(w), CLOSURE_CODE) } == entry).then(|| obj(w))
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
