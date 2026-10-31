use super::*;

pub(crate) fn raise_error(ctx: &mut Ctx, d: Diagnostic) -> i64 {
    let message = d.described(&d.message);
    ctx.fail_unhandled_abort(d, message)
}

pub(super) fn new_scoped_value(c: &mut Ctx, held: Word) -> Word {
    c.scoped_made += 1;
    scoped_word(&mut c.heap, Some(held), c.scoped_made)
}

pub unsafe extern "C" fn rt_hold_made(ctx: *mut Ctx, held: Handed) -> crate::emitted::Handle {
    let c = unsafe { &mut *ctx };
    crate::emitted::Handle::of(new_scoped_value(c, held.word()))
}

pub unsafe extern "C" fn rt_hold_read(ctx: *mut Ctx, h: crate::emitted::Handle) -> Handed {
    let c = unsafe { &mut *ctx };
    Handed::of(
        live_scoped_object(c, &h, Mode::Read).map_or(0, |o| unsafe { word_at(o, SCOPED_VALUE) }),
    )
}

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
        c.touched(crate::tasks::Access::ScopedValue {
            id,
            write: mode == Mode::Write,
        });
    }
    if scoped_already_released(o) {
        let site = c.site();
        c.fail(scoped_released(
            site,
            if mode == Mode::Read { "read" } else { "let go" },
        ));
        return None;
    }
    Some(o)
}

pub const SCOPED_VALUE: usize = CLOSURE_CAPTURES;

const SCOPED_RELEASE: usize = CLOSURE_CAPTURES + 2;

pub(crate) const SCOPED_BYTES: usize = 8 * (3 + CLOSURE_CAPTURES);

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

pub(crate) fn scoped_already_released(o: *mut heap::Obj) -> bool {
    heap::imm_value(unsafe { word_at(o, SCOPED_RELEASE) }) != 0
}

pub(super) fn scoped_object(w: Word) -> Option<*mut heap::Obj> {
    let entry = rt_hold_entry as *const () as usize as Word;
    (heap::is_object(w) && unsafe { word_at(obj(w), CLOSURE_CODE) } == entry).then(|| obj(w))
}

pub(crate) fn scoped_id(w: Word) -> Option<u64> {
    scoped_object(w).map(|o| heap::imm_value(unsafe { word_at(o, CLOSURE_CAPTURES + 1) }) as u64)
}

pub(super) unsafe extern "C" fn rt_hold_entry(ctx: *mut Ctx, _args: *const i64) -> i64 {
    let d = Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a scoped value was called as a function",
    )
    .note("this is Ply's fault, not the program's");
    unsafe { &mut *ctx }.fail(d)
}
