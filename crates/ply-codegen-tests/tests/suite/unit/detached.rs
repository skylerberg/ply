use ply_codegen::c::Native;
use ply_codegen::heap::{Heap, Word};
use ply_codegen::rt::Ctx;
use ply_eval::codes;

const PARKED: &str = r#"
effect amb { read flip[coin]() -> Bool }

type Saved = Nothing | Just((Bool) -> Int)

pub fn parked() -> Saved = with_cell[slot](Nothing) { s -> {
  let inner = handle {
    if amb.flip[coin]() { 41 } else { 0 }
  } with { amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 } };
  assert_eq(inner, 0);
  cell_get(s)
} }

pub fn resume_it(s: Saved) -> Int = match s { Just(k) -> k(true), Nothing -> 0 }

pub fn beside(s: Saved) -> Int = {
  let mine = parked();
  resume_it(s) + resume_it(mine)
}
"#;

/// `parked()` run in an entry of its own, its answer copied out as the memo copies one.
fn carried_out(native: &Native, ctx: &mut Ctx, kept: &mut Heap) -> Word {
    let entry = native.entry("m.parked").expect("`parked` compiled");
    ctx.begin(10_000);
    let out = unsafe { entry(ctx, std::ptr::null()) };
    assert_eq!(ctx.failed, 0, "`parked`: {:?}", ctx.diagnostic);
    let saved = kept.adopt(out);
    ctx.end();
    saved
}

/// Nothing a program writes carries `k` out of its entry, so one that arrives is Ply's defect:
/// it fails the entry rather than index a body that is gone and take the process down. `beside`
/// parks a body of its own first, under the very index the carried `k` names; in a fresh context
/// over the same unit, which reads the same memo, it runs in that context's first entry, as `k`
/// was captured in its own.
#[test]
fn a_continuation_resumed_after_its_entry_fails_that_entry_as_plys_fault() {
    let Some((_source, native)) = crate::fixture::unit(PARKED) else {
        return;
    };
    let layouts = &native.tables().layouts;
    let mut kept = Heap::persistent();
    let mut carrier = native.context();
    let saved = carried_out(&native, &mut carrier, &mut kept);
    let mut fresh = native.context();

    for (name, in_fresh) in [
        ("m.resume_it", false),
        ("m.beside", false),
        ("m.beside", true),
    ] {
        let ctx = if in_fresh { &mut fresh } else { &mut carrier };
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` compiled"));
        ctx.begin(10_000);
        let answer = unsafe { entry(ctx, [saved].as_ptr()) };
        let answered = (ctx.failed == 0).then(|| Heap::to_value(layouts, answer));
        let raised = ctx.take_failure();
        ctx.end();

        assert_eq!(
            answered, None,
            "`{name}` (fresh context: {in_fresh}) resumed a body through another entry's continuation"
        );
        let raised = raised.expect("the failed entry says why");
        assert_eq!(raised.code, codes::INTERNAL_ERROR, "`{name}`: {raised:#?}");
        assert!(
            raised
                .message
                .contains("outside the entry that captured it"),
            "`{name}`: {raised:#?}"
        );
    }
}
