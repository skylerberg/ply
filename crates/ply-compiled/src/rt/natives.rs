//! What the runtime does for the builtins compiled code writes: raising their failures and holding
//! what a scoped value holds.

use super::*;

/// [`Ctx::pend`] for a runtime error that is the program's to handle: a clause for `abort.raise`
/// is given its message, each value the text names told by its kind, since nothing in Rust renders
/// one.
pub(crate) fn raise_error(ctx: &mut Ctx, d: Diagnostic) -> i64 {
    let message = d.described(&d.message);
    ctx.pend(d, message)
}

/// The entry's next scoped value, holding `held`.
pub(super) fn scoped_new(c: &mut Ctx, held: Word) -> Word {
    c.scoped_made += 1;
    scoped_word(&mut c.heap, Some(held), c.scoped_made)
}

/// A scoped value of `held`, which it takes.
pub unsafe extern "C" fn rt_hold_made(ctx: *mut Ctx, held: Handed) -> crate::emitted::Handle {
    let c = unsafe { &mut *ctx };
    crate::emitted::Handle::of(scoped_new(c, held.word()))
}

/// What the borrowed scoped value `h` holds, borrowed from it: whoever reads it counts it as its
/// type does. Fails the entry where the scoped value has let go.
pub unsafe extern "C" fn rt_hold_read(ctx: *mut Ctx, h: crate::emitted::Handle) -> Handed {
    let c = unsafe { &mut *ctx };
    Handed::of(
        live_scoped_object(c, &h, Mode::Read).map_or(0, |o| unsafe { word_at(o, SCOPED_VALUE) }),
    )
}

/// What the borrowed scoped value `h` holds, taken out of it, which lets it go. Fails the entry
/// has let go already.
pub unsafe extern "C" fn rt_hold_taken(ctx: *mut Ctx, h: crate::emitted::Handle) -> Handed {
    let c = unsafe { &mut *ctx };
    Handed::of(
        live_scoped_object(c, &h, Mode::Write).map_or(0, |o| unsafe {
            let held = word_at(o, SCOPED_VALUE);
            set_word(o, SCOPED_VALUE, 0);
            set_word(o, SCOPED_RELEASE, heap::imm(1));
            held
        }),
    )
}

/// The object of scoped value `h`, a read of it (`mode`) noted for a simulation; none where it
/// has let go, or is no scoped value, which fails the entry.
fn live_scoped_object(
    c: &mut Ctx,
    h: &crate::emitted::Handle,
    mode: Mode,
) -> Option<*mut heap::Obj> {
    let Some(o) = scoped_object(h.word()) else {
        c.fail(
            error("a scoped value that is no runtime scoped value")
                .note("this is Ply's fault, not the program's"),
        );
        return None;
    };
    if let Some(id) = scoped_id(h.word()) {
        c.touched(crate::tasks::Touched::ScopedValue {
            id,
            write: mode == Mode::Write,
        });
    }
    if scoped_let_go(o) {
        let site = c.site();
        c.fail(scoped_released(
            site,
            if mode == Mode::Read { "read" } else { "let go" },
        ));
        return None;
    }
    Some(o)
}

/// The payload word a scoped value keeps its value in, which emitted code reads and empties itself.
pub const SCOPED_VALUE: usize = CLOSURE_CAPTURES;

/// The word a scoped value marks itself let go in, apart from what it holds, whose word may be 0
/// itself (a `()`, or a `None` held as one pointer).
const SCOPED_RELEASE: usize = CLOSURE_CAPTURES + 2;

/// The payload bytes of a scoped value's block.
pub(crate) const SCOPED_BYTES: usize = 8 * (3 + CLOSURE_CAPTURES);

/// A scoped value, a closure no call enters: its value in its first capture, emptied once it lets
/// go; `id`, its number in the entry, in its second, which names it to the scheduler; and in its
/// third whether it has let go, as one made with nothing held has, so a second release or a read
/// after is rejected.
pub(crate) fn scoped_word(heap: &mut Heap, held: Option<Word>, id: u64) -> Word {
    let o = heap.raw_alloc(SCOPED_BYTES);
    unsafe {
        set_word(o, CLOSURE_CODE, rt_hold_entry as *const () as usize as Word);
        set_word(o, SCOPED_VALUE, held.unwrap_or(0));
        set_word(o, CLOSURE_CAPTURES + 1, heap::imm(id as i64));
        set_word(o, SCOPED_RELEASE, heap::imm(i64::from(held.is_none())));
    }
    o as Word
}

/// Whether the scoped value `o` has let go of what it held.
pub(crate) fn scoped_let_go(o: *mut heap::Obj) -> bool {
    heap::imm_value(unsafe { word_at(o, SCOPED_RELEASE) }) != 0
}

/// The block a scoped value is, where `w` is one: its first word names the entry no call enters.
pub(super) fn scoped_object(w: Word) -> Option<*mut heap::Obj> {
    let entry = rt_hold_entry as *const () as usize as Word;
    (heap::is_object(w) && unsafe { word_at(obj(w), CLOSURE_CODE) } == entry).then(|| obj(w))
}

/// A scoped value's number in its entry, where `w` is a scoped value.
pub(crate) fn scoped_id(w: Word) -> Option<u64> {
    scoped_object(w).map(|o| heap::imm_value(unsafe { word_at(o, CLOSURE_CAPTURES + 1) }) as u64)
}

/// The entry of a scoped value, which nothing calls: a scoped value is no function.
pub(super) unsafe extern "C" fn rt_hold_entry(ctx: *mut Ctx, _args: *const i64) -> i64 {
    let d = Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a scoped value was called as a function",
    )
    .note("this is Ply's fault, not the program's");
    unsafe { &mut *ctx }.fail(d)
}
