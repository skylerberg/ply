mod dump;
mod sweep;
mod toolchain;

use crate::fixture;

use ply_codegen::c::exports::HelperShape;
use ply_codegen::c::tables::{BUCKETS, bucket_of};
use ply_codegen::c::{
    Native, PRELUDE, RUNTIME_MARK, builtin_helper_name, compile_and_load, helpers, runtime_header,
    runtime_object, split,
};

/// A declaration with no address is a null call at run time: a crash rather than a decline.
#[test]
fn every_declared_helper_has_an_address() {
    for h in helpers() {
        assert!(!h.address.is_null(), "`{}` has no address", h.name);
    }
}

/// A builtin's helper takes the words a call of it passes, under the name the emitter writes.
#[test]
fn every_builtin_has_a_helper_of_its_own() {
    for b in ply_eval::Builtin::all() {
        let name = builtin_helper_name(b.name());
        assert!(
            name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
            "`{name}` is no C name"
        );
        let found: Vec<_> = helpers().iter().filter(|h| h.name == name).collect();
        assert_eq!(found.len(), 1, "`{name}`");
        assert_eq!(found[0].args, b.arity(), "`{name}`");
        assert!(found[0].answers, "`{name}`");
    }
}

#[test]
fn the_prelude_agrees_with_the_layouts_it_mirrors() {
    assert_eq!(ply_codegen::heap::HEADER, 16, "PLY_HEADER");
    assert_eq!(std::mem::size_of::<ply_codegen::heap::Obj>(), 16, "PlyObj");
    assert_eq!(ply_codegen::heap::FLAT, 1, "PLY_FLAT");
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, failed),
        0,
        "PlyCtx.failed"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, fuel),
        8,
        "PlyCtx.fuel"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, stack_floor),
        16,
        "PlyCtx.stack_floor"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, ticks),
        48,
        "PlyCtx.ticks"
    );
    assert_eq!(
        std::mem::offset_of!(ply_codegen::rt::Ctx, next_tick),
        56,
        "PlyCtx.next_tick"
    );
    assert!(PRELUDE.contains("#define PLY_HEADER 16"));
}

#[test]
fn a_unit_compiles_loads_binds_and_answers() {
    let mut src = String::from(PRELUDE);
    src.push_str(&runtime_header());
    src.push_str(&runtime_object());
    src.push_str(
        r#"
Word ply_probe(PlyCtx *ctx, const Word *args) {
  (void)ctx;
  return ply_imm(ply_imm_value(args[0]) + ply_imm_value(args[1]));
}
"#,
    );
    let lib = match compile_and_load(&src, "probe") {
        Ok(l) => l,
        // A machine with no C compiler is not one this backend is for.
        Err(e) if e.to_string().contains("could not run") => return,
        Err(e) => panic!("{e}"),
    };
    let bind = lib.symbol("ply_bind").expect("the unit exports `ply_bind`");
    let bind: unsafe extern "C" fn(*const *mut std::ffi::c_void) =
        unsafe { std::mem::transmute(bind) };
    let addrs: Vec<*mut std::ffi::c_void> = helpers()
        .iter()
        .map(|h| h.address as *mut std::ffi::c_void)
        .collect();
    unsafe { bind(addrs.as_ptr()) };
    let probe = lib
        .symbol("ply_probe")
        .expect("the unit exports `ply_probe`");
    let probe: ply_codegen::rt::Entry = unsafe { std::mem::transmute(probe) };
    let args = [ply_codegen::heap::imm(20), ply_codegen::heap::imm(22)];
    let answer = unsafe { probe(std::ptr::null_mut(), args.as_ptr()) };
    assert_eq!(ply_codegen::heap::imm_value(answer), 42);
}

#[test]
fn the_c_backend_answers_calls_recursion_widths_and_records() {
    let source = r#"
fn double(x: Int) -> Int = x * 2
fn even(x: Int) -> Bool = x % 2 == 0
fn clamp(x: Int, lo: Int, hi: Int) -> Int =
  if x < lo { lo } else { if x > hi { hi } else { x } }
fn collatz(n: Int) -> Int / {diverges} =
  if n <= 1 { 0 } else { if even(n) { 1 + collatz(n / 2) } else { 1 + collatz(3 * n + 1) } }
pub fn width(a: Int, b: Int) -> Int / {abort.raise} =
  int_of_u32(wrap_add(u32_of_int(a), u32_of_int(b)) ^ rotr(u32_of_int(b), 8))
pub fn shaped(n: Int) -> Int = { let r = {x: n, y: n + 1}; r.x * 10 + r.y }
"#;
    let Some((loaded, native)) = fixture::unit(source) else {
        return;
    };
    let cases: &[(&str, Vec<i64>, i64)] = &[
        ("m.double", vec![21], 42),
        ("m.clamp", vec![150, 0, 100], 100),
        ("m.collatz", vec![27], 111),
        (
            "m.width",
            vec![7, 9],
            7i64.wrapping_add(9) ^ (9u32.rotate_right(8) as i64),
        ),
        ("m.shaped", vec![4], 45),
    ];
    for (name, args, want) in cases {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let words: Vec<i64> = args.iter().map(|a| ply_codegen::heap::imm(*a)).collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        assert_eq!(
            ply_codegen::heap::imm_value(answer),
            *want,
            "`{name}{args:?}` answered wrongly"
        );
    }
    let _ = loaded;
}

/// A frame that would cross the floor gets a stack of its own, so how deep a program nests is
/// what its fuel says and not what the thread it runs on happens to have been given. The floor is
/// moved up to just under this frame, so the guard fires on any machine rather than on a lucky one.
#[test]
fn a_recursion_past_what_the_stack_holds_grows_onto_another_and_answers() {
    const LADDER: &str = "fn ladder(n: Int) -> Int = if n <= 0 { 0 } else { 1 + ladder(n - 1) }";
    const DEEP: i64 = 20_000;
    let Some((_loaded, native)) = fixture::unit(LADDER) else {
        return;
    };
    let entry: ply_codegen::rt::Entry = native.entry("m.ladder").expect("`m.ladder` compiled");
    let mut ctx = native.context();
    ctx.begin(DEEP * 2);
    let here = 0u8;
    ctx.stack_floor = std::ptr::from_ref(&here) as usize - 64 * 1024;
    let args = [ply_codegen::heap::imm(DEEP)];
    let answer = unsafe { entry(&mut ctx, args.as_ptr()) };
    let (failed, grown) = (ctx.failed, ctx.grown);
    let raised = ctx.take_failure().map(|d| d.message);
    let value = ply_codegen::heap::imm_value(answer);
    ctx.end();
    assert_eq!(failed, 0, "`m.ladder({DEEP})` raised: {raised:?}");
    assert_eq!(value, DEEP);
    assert!(
        grown > 0,
        "the call never crossed the floor, so nothing grew"
    );
}

/// Growing is not a licence to recurse for ever: depth is the fuel's to bound, and at ten
/// thousand it fires long before the step budget counts the same calls as work.
#[test]
fn a_recursion_with_no_base_case_still_stops_at_the_fuel() {
    const SPIN: &str = "fn spin(n: Int) -> Int / {diverges} = 1 + spin(n + 1)";
    let Some((_loaded, native)) = fixture::unit(SPIN) else {
        return;
    };
    let entry: ply_codegen::rt::Entry = native.entry("m.spin").expect("`m.spin` compiled");
    let mut ctx = native.context();
    ctx.begin(10_000);
    let here = 0u8;
    ctx.stack_floor = std::ptr::from_ref(&here) as usize - 64 * 1024;
    let args = [ply_codegen::heap::imm(0)];
    let _ = unsafe { entry(&mut ctx, args.as_ptr()) };
    let failed = ctx.failed;
    let raised = ctx.take_failure().map(|d| d.message).unwrap_or_default();
    ctx.end();
    assert_eq!(
        failed,
        ply_codegen::rt::FAILED_OUT_OF_FUEL,
        "a runaway recursion ended some other way: {raised}"
    );
    assert!(raised.contains("bound on nested calls"), "{raised}");
}

/// A unit the builder made: its C, and what it says about itself.
pub struct Produced {
    pub text: String,
    pub exports: ply_codegen::c::Exports,
}

impl Produced {
    fn of(answer: &ply_machine::runnable::Runnable) -> Produced {
        let exports =
            ply_codegen::c::Exports::from_text(&answer.unit).expect("a unit says what it holds");
        Produced {
            text: answer.unit.clone(),
            exports,
        }
    }
}

/// The unit the builder makes of `modules`, every root offered.
fn produced_of(modules: &[(&str, &str)]) -> Produced {
    Produced::of(&fixture::answered(modules))
}

/// Module `m` built over every root of `text`, or nothing where no C compiler runs.
fn built(text: &str) -> Option<Native> {
    let (_, native, refused) = fixture::with_refusals(text)?;
    assert!(refused.is_empty(), "{refused:?}");
    Some(native)
}

/// What a unit says about itself reads back from its text as it was embedded, so whether it serves
/// this runtime is known before anything is compiled: its helpers bind by name, in its own order.
#[test]
fn a_units_table_reads_back_from_its_text_and_says_whether_it_serves() {
    let produced = produced_of(&[(
        "m",
        "pub type Shape = | Dot | Line(Int)\nfn double(x: Int) -> Int = x * 2\n\
         fn named(s: Shape) -> Bytes = match s { Dot -> b\"dot\", Line(_) -> b\"a \\\"line\\\"?\" }\n",
    )]);
    let read = ply_codegen::c::Exports::from_text(&produced.text).expect("the table reads back");
    assert_eq!(read.encode(), produced.exports.encode());
    let bound = read
        .bound(&produced.text)
        .expect("the unit serves this runtime");
    for (helper, address) in read.helpers.iter().zip(&bound) {
        let runtime = helpers()
            .iter()
            .find(|h| h.name == helper.name)
            .expect("the runtime has every helper it handed the emitter");
        assert_eq!(*address as *const (), runtime.address, "`{}`", helper.name);
    }
    // A helper this runtime does not have binds to nothing while the unit's C never calls it.
    let mut retired = read.clone();
    retired.helpers.push(HelperShape {
        name: "rt_retired".to_string(),
        args: 1,
        answers: true,
    });
    let bound = retired
        .bound(&produced.text)
        .expect("a helper nothing calls is no reason to refuse the unit");
    assert!(bound.last().is_some_and(|address| address.is_null()));
    let calling = format!(
        "{}\nWord gone(PlyCtx *ctx) {{ return rt_retired_p(ctx, 0); }}\n",
        produced.text
    );
    let refused = retired
        .bound(&calling)
        .expect_err("a unit that calls what the runtime lacks does not serve it");
    assert!(refused.to_string().contains("`rt_retired`"), "{refused}");
    // One the runtime has under another shape is another helper.
    let mut reshaped = read.clone();
    reshaped.helpers[0].args += 1;
    let refused = reshaped
        .bound(&produced.text)
        .expect_err("a helper taking other words is not the one the unit was emitted against");
    assert!(
        refused.to_string().contains(&reshaped.helpers[0].name),
        "{refused}"
    );
    assert!(ply_codegen::c::Exports::from_text("int main(void) { return 0; }").is_none());
}

/// Each bucket's table reads back as the loader fills it, and a table whose C writes the unit's
/// positions itself names none.
#[test]
fn a_units_bucket_tables_read_back_and_a_table_without_them_names_none() {
    let tables = "helpers 0\nctors 0\ntaken 0\nconstants 0\nmodules 0\nrefused 0\n\
                  consts 0\nbuiltins 0\nfields 0\nshapes 0\nlambdas 0\n";
    let with = format!("{tables}buckets 2\n3 0 2\n58 1\n");
    let read = ply_codegen::c::Exports::decode(&with).expect("the table reads");
    assert_eq!(read.buckets, vec![(3, vec![0, 2]), (58, vec![1])]);
    assert_eq!(read.encode(), with);
    let without = ply_codegen::c::Exports::decode(tables).expect("the table reads");
    assert!(without.buckets.is_empty());
    assert!(without.instances.is_empty());
    // What a type's module states its values are read through follows the buckets, in a unit
    // that holds such a type: `-` for none, `!` for a function the unit does not take.
    let stating = format!("{with}instances 2\nm.Box m.area !\nm.Date - m.written\n");
    let read = ply_codegen::c::Exports::decode(&stating).expect("the table reads");
    assert_eq!(
        read.instances,
        vec![
            ("m.Box".into(), "m.area".to_string(), "!".to_string()),
            ("m.Date".into(), "-".to_string(), "m.written".to_string()),
        ]
    );
    assert_eq!(read.encode(), stating);
}

/// A stated function is called with the value alone, so a unit that states one taking more words
/// is not loaded.
#[test]
fn a_unit_stating_a_function_of_more_than_the_value_is_refused() {
    let mut made = fixture::answered(&[(
        "m",
        "type Box = | Box(Int)\n\
         fn area(b: Box) -> Int = match b { Box(n) -> n }\n\
         pub fn scaled(b: Box, by: Int) -> Int = area(b) * by\n\
         key for Box by area\n",
    )]);
    let stated = "\"m.Box m.area -\\n\"";
    assert!(made.unit.contains(stated), "the unit states `Box`'s key");
    made.unit = made.unit.replace(stated, "\"m.Box m.scaled -\\n\"");
    let front: &'static ply_eval::Analysis = Box::leak(Box::new(made.front.answer));
    let source = ply_codegen::source::Source::from_analysis(front);
    let _config = fixture::CONFIG.read().unwrap_or_else(|e| e.into_inner());
    match ply_codegen::c::load_unit(&made.unit, Some(&source), "unit") {
        Ok(_) => panic!("a key that takes two words was bound"),
        Err(e) if e.to_string().contains("could not run") => {}
        Err(e) => assert!(e.to_string().contains("`m.scaled`"), "{e}"),
    }
}

/// A unit handed over is loaded once, to read what it holds; the first backend on that thread takes
/// that load rather than mapping the image again, and a later one maps it anew.
#[test]
fn the_first_backend_on_the_handing_thread_takes_the_unit_it_loaded() {
    use ply_eval::{Provider, Symbol, Value};
    use std::sync::atomic::Ordering::Relaxed;
    let answer = fixture::answered(&[("m", "fn double(x: Int) -> Int = x * 2\n")]);
    let front: &'static ply_eval::Analysis = Box::leak(Box::new(answer.front.answer));
    // Writing: `UNITS_MAPPED` below counts every load in the process.
    let _config = fixture::CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let unit = ply_codegen::Unit::handed(front, answer.unit).expect("this host has a C compiler");
    let mapped = || ply_codegen::c::UNITS_MAPPED.load(Relaxed);
    let before = mapped();
    let first = unit.attach();
    assert_eq!(mapped(), before, "the first backend mapped the unit again");
    let second = unit.attach();
    assert_eq!(mapped(), before + 1, "a later backend maps the unit anew");
    assert_eq!(unit.compilation().units, 2);
    for backend in [&first, &second] {
        assert_eq!(
            backend.enter(&Symbol::new("m.double"), &[Value::Int(21)], 10_000),
            Some(Value::Int(42))
        );
    }
}

#[test]
fn the_c_backend_answers_what_the_program_means() {
    let source = r#"
type Quad = { a: U32, b: U32, c: U32, d: U32 }
fn g(q: Quad, mx: U32) -> Quad = {
  let a1 = wrap_add(wrap_add(q.a, q.b), mx);
  let d1 = rotr(q.d ^ a1, 16);
  let c1 = wrap_add(q.c, d1);
  let b1 = rotr(q.b ^ c1, 12);
  {a: a1, b: b1, c: c1, d: d1}
}
pub fn mixed(n: Int) -> Int / {abort.raise} = {
  let w = u32_of_int(n);
  let q = g({a: w, b: 1u32, c: 0x3C6E_F372u32, d: 0xA54F_F53Au32}, w);
  int_of_u32(q.a ^ q.b ^ q.c ^ q.d)
}
pub fn counted(n: Int) -> Int / {abort.raise} =
  iterate({i: 0, acc: 0}, n + 1, |s: {i: Int, acc: Int}|
    if s.i >= n { Stop(s.acc) } else { Continue({i: s.i + 1, acc: s.acc + s.i * s.i}) })
pub fn bytes_sum(b: Bytes) -> Int / {abort.raise} =
  iterate({i: 0, acc: 0}, bytes_len(b) + 1, |s: {i: Int, acc: Int}|
    if s.i >= bytes_len(b) { Stop(s.acc) }
    else { Continue({i: s.i + 1, acc: s.acc + bytes_at(b, s.i)}) })
pub fn shifted(a: Int, n: Int) -> Int = (a << n) + (a >> n) + (a >>> n)
pub fn matched(n: Int) -> Int = match n { 0 -> 100, 1 -> 200, _ -> n * 3 }
pub fn looped(n: Int) -> Int / {abort.raise} =
  iterate({i: 0, q: {a: 1u32, b: 2u32, c: 3u32, d: 4u32}}, n + 1, |s: {i: Int, q: Quad}|
    if s.i >= n { Stop(int_of_u32(s.q.a ^ s.q.b ^ s.q.c ^ s.q.d)) }
    else { Continue({i: s.i + 1, q: g(s.q, u32_of_int(s.i))}) })
"#;
    let Some((_, native)) = fixture::unit(source) else {
        return;
    };
    let int = ply_eval::Value::Int;
    let cases: &[(&str, Vec<ply_eval::Value>, i64)] = &[
        ("m.mixed", vec![int(0xDEAD_BEEF)], 0x4896_3B0D),
        ("m.mixed", vec![int(0)], 0x4892_2726),
        ("m.counted", vec![int(40)], 20_540),
        // Built at one site but described once per iteration: reusing the first iteration's object is a wrong answer, not a crash.
        ("m.looped", vec![int(1)], 0x0010_0070),
        ("m.looped", vec![int(7)], 0x627E_6B17),
        (
            "m.bytes_sum",
            vec![ply_eval::Value::bytes(b"the quick brown fox")],
            1843,
        ),
        (
            "m.shifted",
            vec![int(-9), int(3)],
            2_305_843_009_213_693_876,
        ),
        ("m.matched", vec![int(0)], 100),
        ("m.matched", vec![int(1)], 200),
        ("m.matched", vec![int(7)], 21),
    ];
    for (name, args, want) in cases {
        let want = ply_eval::Value::Int(*want);
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts_ptr: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts_ptr }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C backend");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(got, want, "`{name}{args:?}`");
    }
}

/// A `U64` past `2^62` is not an immediate (tagging eats its top bit), so it is held as the machine's own value.
#[test]
fn a_width_the_c_backend_cannot_carry_in_a_register_still_answers() {
    let source = r#"
pub fn wide(n: Int) -> Int / {abort.raise} = {
  let a = u64_of_int(n);
  let b = wrap_mul(wrap_add(a, a), 0x9E37_79B9_7F4A_7C15u64);
  int_of_u64(rotr(b, 7) & 0xFFFFu64)
}
pub fn narrow(n: Int) -> Int / {abort.raise} = int_of_u32(rotr(wrap_mul(u32_of_int(n), 2654435761u32), 7))
"#;
    let Some((_, native, _)) = fixture::with_refusals(source) else {
        return;
    };
    // `None` raises: `_of_int` refuses a value outside its width rather than truncating it.
    let answers: [(&str, [Option<i64>; 5]); 2] = [
        (
            "m.wide",
            [Some(0), Some(10_736), Some(26_178), Some(0), None],
        ),
        (
            "m.narrow",
            [
                Some(0),
                Some(1_664_904_947),
                Some(3_544_340_112),
                None,
                None,
            ],
        ),
    ];
    for (name, wants) in answers {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        for (n, want) in [0i64, 1, 12_345, 1 << 40, -7].into_iter().zip(wants) {
            let mut ctx = native.context();
            ctx.fuel = 1_000;
            let layouts_ptr: *const ply_codegen::heap::Layouts = &native.tables().layouts;
            let word = ctx
                .heap
                .to_word(unsafe { &*layouts_ptr }, &ply_eval::Value::Int(n));
            let answer = unsafe { entry(&mut ctx, [word].as_ptr()) };
            match want {
                Some(want) => {
                    assert_eq!(ctx.failed, 0, "`{name}({n})` raised in the C backend");
                    let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
                    assert_eq!(got, ply_eval::Value::Int(want), "`{name}({n})`");
                }
                None => assert_ne!(ctx.failed, 0, "`{name}({n})` answered out of its width"),
            }
        }
    }
}

#[test]
fn a_record_with_a_counted_field_survives_being_rebuilt() {
    let p = |depth: i64, diags: &[i64]| {
        ply_eval::Value::Record(std::sync::Arc::new(ply_eval::Fields::from_unsorted(vec![
            (ply_eval::Symbol::new("pos"), ply_eval::Value::Int(0)),
            (ply_eval::Symbol::new("depth"), ply_eval::Value::Int(depth)),
            (
                ply_eval::Symbol::new("diags"),
                ply_eval::Value::list(diags.iter().map(|&d| ply_eval::Value::Int(d)).collect()),
            ),
        ])))
    };
    for (which, body, want) in [
        (
            "plain",
            "pub fn probe(n: Int) -> P = {pos: 0, depth: n, diags: [n]}",
            p(4, &[4]),
        ),
        (
            "rebuilt",
            "pub fn probe(n: Int) -> P = with_depth({pos: 0, depth: n, diags: [n]}, 9)",
            p(9, &[4]),
        ),
        (
            "pushed",
            "pub fn probe(n: Int) -> P = noted({pos: 0, depth: n, diags: [n]}, 7)",
            p(4, &[4, 7]),
        ),
        (
            "let-bound",
            "pub fn probe(n: Int) -> P = { let p = {pos: 0, depth: n, diags: [n]}; with_depth(p, p.depth + 1) }",
            p(5, &[4]),
        ),
        (
            "wrapped",
            "pub fn probe(n: Int) -> Option<P> = Some({pos: 0, depth: n, diags: [n]})",
            ply_eval::Value::ctor("Some", vec![p(4, &[4])]),
        ),
    ] {
        let source = format!(
            r#"
type P = {{ pos: Int, depth: Int, diags: List<Int> }}
fn with_depth(p: P, d: Int) -> P = {{ pos: p.pos, depth: d, diags: p.diags }}
fn noted(p: P, x: Int) -> P = {{ pos: p.pos, depth: p.depth, diags: push(p.diags, x) }}
{body}
"#
        );
        eprintln!("--- shape: {which}");
        let Some((_, native)) = fixture::unit(&source) else {
            return;
        };
        let args = [ply_eval::Value::Int(4)];
        let entry: ply_codegen::rt::Entry = native.entry("m.probe").expect("compiled");
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts_ptr: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts_ptr }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{which}` raised in the C backend");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(got, want, "`{which}`");
    }
}

/// An emitted body carries its own name, so a cache keyed on the content hash alone would serve one body for both.
#[test]
fn two_definitions_that_say_the_same_thing_get_their_own_bodies() {
    let source = r#"
pub fn one(b: Bytes, i: Int) -> Int / {abort.raise} = bytes_at(b, i) + 1
pub fn two(b: Bytes, i: Int) -> Int / {abort.raise} = bytes_at(b, i) + 1
"#;
    let Some((_, native)) = fixture::unit(source) else {
        return;
    };
    assert!(native.entry("m.one").is_some(), "`one` has no body");
    assert!(native.entry("m.two").is_some(), "`two` has no body");
}

/// A user constructor is interned under its module-qualified name, while a body names it bare.
#[test]
fn a_constructor_a_program_declares_is_built_and_matched_like_a_preludes() {
    let source = r#"
type Tok = TEof | TNum(Int) | TName(Bytes)
pub fn code(t: Tok) -> Int =
  match t {
    TEof -> 0,
    TNum(n) -> n,
    TName(b) -> bytes_len(b),
  }
pub fn round(n: Int) -> Int = code(TNum(n))
pub fn eof() -> Int = code(TEof)
pub fn named(b: Bytes) -> Int = code(TName(b))
"#;
    let Some((_, native, refused)) = fixture::with_refusals(source) else {
        return;
    };
    assert!(
        refused.is_empty(),
        "nothing here is outside the fragment: {refused:?}"
    );
    let cases: &[(&str, Vec<ply_eval::Value>, i64)] = &[
        ("m.round", vec![ply_eval::Value::Int(7)], 7),
        ("m.eof", vec![], 0),
        ("m.named", vec![ply_eval::Value::bytes(b"abcd")], 4),
    ];
    for (name, args, want) in cases {
        let want = &ply_eval::Value::Int(*want);
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C backend");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(&got, want, "`{name}{args:?}`");
    }
}

/// `rt_dec` frees unconditionally (the `rc == 1` case), and its `debug_assert` is compiled out of the suite's profile.
#[test]
fn a_list_survives_a_fold_over_it_and_a_closure_survives_being_called() {
    let source = r#"
fn add(a: Int, x: Int) -> Int = a + x
pub fn twice(xs: List<Int>) -> Int = fold(xs, 0, add) + fold(xs, 0, add)
pub fn and_len(xs: List<Int>) -> Int = fold(xs, 0, add) + len(xs)
pub fn through_a_value(xs: List<Int>, k: Int) -> Int = fold(xs, 0, |a: Int, x: Int| a + x * k)
pub fn mapped(xs: List<Int>, k: Int) -> List<Int> = map(xs, |x: Int| x * k)
pub fn by_name(xs: List<Int>) -> Int = fold(map(xs, |x: Int| x + 1), 0, add)
pub fn adder(n: Int) -> (Int) -> Int = |x: Int| x + n
pub fn used_twice(n: Int, x: Int) -> Int = { let f = adder(n); f(x) + f(x) }
"#;
    let Some((_, native, refused)) = fixture::with_refusals(source) else {
        return;
    };
    assert!(
        refused.is_empty(),
        "the callback family is inside the fragment now: {refused:?}"
    );
    let list =
        |xs: &[i64]| ply_eval::Value::list(xs.iter().map(|n| ply_eval::Value::Int(*n)).collect());
    let int = ply_eval::Value::Int;
    let cases: &[(&str, Vec<ply_eval::Value>, ply_eval::Value)] = &[
        ("m.twice", vec![list(&[1, 2, 3])], int(12)),
        ("m.and_len", vec![list(&[1, 2, 3])], int(9)),
        (
            "m.through_a_value",
            vec![list(&[1, 2, 3]), int(10)],
            int(60),
        ),
        ("m.mapped", vec![list(&[1, 2, 3]), int(3)], list(&[3, 6, 9])),
        ("m.by_name", vec![list(&[1, 2, 3, 4])], int(14)),
        ("m.used_twice", vec![int(5), int(2)], int(14)),
    ];
    for (name, args, want) in cases {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C backend");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(&got, want, "`{name}{args:?}`");
    }
}

/// Nanoseconds now: a definition whose text carries it is in no earlier process's cache.
fn nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        % 1_000_000_000_000_000
}

/// Counted in calls, never timed: whatever a pure nullary root answers, reads ask the runtime for it.
#[test]
fn a_pure_nullary_root_runs_its_body_once_whatever_it_answers() {
    const READS: i64 = 64;
    let source = r#"
fn deep(n: Int) -> Int = if n <= 0 { 0 } else { 1 + deep(n - 1) }
pub fn a_list() -> List<Int> = [deep(32)]
pub fn an_int() -> Int = deep(32)
pub fn a_bool() -> Bool = deep(32) > 0
pub fn a_u8() -> U8 / {abort.raise} = u8_of_int(deep(32))
pub fn reads_a_list(n: Int) -> Int = fold(range(0, n), 0, |acc: Int, _x: Int| acc + len(a_list()))
pub fn reads_an_int(n: Int) -> Int = fold(range(0, n), 0, |acc: Int, _x: Int| acc + an_int())
pub fn reads_a_bool(n: Int) -> Int =
  fold(range(0, n), 0, |acc: Int, _x: Int| if a_bool() { acc + 1 } else { acc })
pub fn reads_a_u8(n: Int) -> Int / {abort.raise} = fold(range(0, n), 0, |acc: Int, _x: Int| acc + int_of_u8(a_u8()))
"#;
    let Some((loaded, native)) = fixture::unit(source) else {
        return;
    };
    // An entry's answer and the calls it made; entering a root itself leaves its memo slot alone.
    let run = |name: &str, args: &[i64]| -> (i64, i64) {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was refused"));
        let mut ctx = native.context();
        ctx.begin(100_000);
        let words: Vec<i64> = args.iter().map(|a| ply_codegen::heap::imm(*a)).collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        let (failed, ticks) = (ctx.failed, ctx.ticks);
        ctx.end();
        assert_eq!(failed, 0, "`{name}` raised");
        (answer, ticks)
    };
    for (root, reader, each) in [
        ("m.a_list", "m.reads_a_list", 1),
        ("m.an_int", "m.reads_an_int", 32),
        ("m.a_bool", "m.reads_a_bool", 1),
        ("m.a_u8", "m.reads_a_u8", 32),
    ] {
        let slot = native
            .constant_index(root)
            .unwrap_or_else(|| panic!("`{root}` was given no memo slot"));
        assert!(
            slot < native.tables().functions.len(),
            "`{root}`'s memo slot is not a row of the code table `rt_constant` calls through"
        );
        assert!(
            native.tables().memoized(slot).is_none(),
            "something was remembered before `{root}` ever ran"
        );
        let (_, body) = run(root, &[]);
        let (answer, paid) = run(reader, &[READS]);
        assert_eq!(
            ply_codegen::heap::imm_value(answer),
            READS * each,
            "`{reader}`"
        );
        assert!(
            native.tables().memoized(slot).is_some(),
            "`{reader}` called `{root}` directly instead of asking the runtime for its answer"
        );
        let (again, unpaid) = run(reader, &[READS]);
        assert_eq!(
            again, answer,
            "`{reader}` read the remembered answer differently"
        );
        assert_eq!(
            paid - unpaid,
            body,
            "`{reader}` ran `{root}`'s body of {body} calls {} times, not once",
            (paid - unpaid) / body.max(1)
        );
    }
    let _ = loaded;
}

/// At the body's tail nothing follows the update, so the read-once guard need not hold there; `wide` and `narrow` differ only in reads of `s`.
#[test]
fn an_accumulator_read_more_than_once_is_still_let_go() {
    let source = r#"
type S = { i: Int, tag: Bytes }
fn narrow(s: S, x: Int) -> S = {..s, i: x}
fn wide(s: S, x: Int) -> S = {..s, i: s.i + x}
pub fn with_narrow(n: Int) -> Int = fold(range(0, n), {i: 0, tag: b"z"}, narrow).i
pub fn with_wide(n: Int) -> Int = fold(range(0, n), {i: 0, tag: b"z"}, wide).i
"#;
    let Some((loaded, native)) = fixture::unit(source) else {
        return;
    };
    let rounds = 500i64;
    let allocations = |name: &str| -> usize {
        let entry: ply_codegen::rt::Entry = native.entry(name).expect("compiled");
        let mut ctx = native.context();
        ctx.fuel = 1_000_000;
        let before = ctx.heap.allocated();
        let args = [ply_codegen::heap::imm(rounds)];
        let w = unsafe { entry(&mut ctx, args.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        let _ = w;
        ctx.heap.allocated() - before
    };
    let narrow = allocations("m.with_narrow");
    let wide = allocations("m.with_wide");
    assert!(
        wide <= narrow,
        "reading the accumulator twice allocated {} more object(s) over {rounds} rounds than \
         reading it once: the update is not letting its base go",
        wide - narrow
    );
    let _ = loaded;
}

/// Asserted on the emitted text: a doubled count is not a wrong answer but an object that cannot die.
#[test]
fn a_helper_answer_handed_to_a_helper_is_not_counted_again() {
    let source = r#"
pub fn wrap(n: Int) -> List<Bytes> / {abort.raise} = [byte_of_int(n)]
"#;
    let produced = produced_of(&[("m", source)]);
    let text = numbering_support::body(
        &produced.text,
        &numbering_support::symbol(&produced, "m.wrap"),
    );
    assert!(
        text.contains("builtin_byte_of_int_p") && text.contains("rt_list_p"),
        "the body no longer has the shape this test is about:\n{text}"
    );
    assert_eq!(
        text.matches("ply_inc(").count(),
        0,
        "a count was taken on a word the helper had already counted:\n{text}"
    );
}

/// The unit's C up to its embedded table, and a body's text within it.
mod numbering_support {
    pub fn code(text: &str) -> &str {
        text.split("/* --- what this unit says about itself")
            .next()
            .expect("a split has a first piece")
    }

    pub fn body<'a>(text: &'a str, symbol: &str) -> &'a str {
        let at = text
            .find(&format!("Word {symbol}(PlyCtx *ctx"))
            .unwrap_or_else(|| panic!("the unit has no body for `{symbol}`"));
        let body = &text[at..];
        &body[..body.find("\n}\n").map_or(body.len(), |end| end + 3)]
    }

    /// The unit's position of each literal a body reads, through its bucket's table.
    pub fn literal_places(produced: &super::Produced, body: &str) -> Vec<u32> {
        body.match_indices("rt_lit_p(ctx, ply_bk_")
            .filter_map(|(at, read)| {
                let rest = &body[at + read.len()..];
                let id = u8::from_str_radix(rest.get(..2)?, 16).ok()?;
                let slot: usize = rest.get(3..rest.find(']')?)?.parse().ok()?;
                let (_, places) = produced.exports.buckets.iter().find(|(b, _)| *b == id)?;
                places.get(slot).copied()
            })
            .collect()
    }

    /// The C the unit published for `name`; a test reads a symbol rather than spelling one.
    pub fn symbol(produced: &super::Produced, name: &str) -> String {
        produced
            .exports
            .taken_by_name(name)
            .unwrap_or_else(|| panic!("the unit does not take `{name}`"))
            .symbol
            .clone()
    }
}

/// `zz` and everything it adds to the tables sort last, so no rank an earlier body resolved to moves.
#[test]
fn a_definition_added_to_a_program_leaves_every_other_body_as_it_was() {
    let base = r#"
type Pair = { left: Int, right: Int }
fn key(x: Int) -> Int = x + 1
pub fn many(xs: List<Int>) -> Int = fold(map(xs, key), 0, |a: Int, x: Int| a + x)
pub fn named(p: Pair) -> Bytes = if p.left > p.right { b"alpha" } else { b"beta" }
"#;
    let grown = format!(
        "{base}pub fn zz(xs: List<Int>) -> Bytes = \
         if fold(xs, many(xs), |a: Int, x: Int| a + x) > 0 {{ b\"~tilde\" }} else {{ b\"alpha\" }}\n"
    );
    let small = produced_of(&[("m", base)]);
    let large = produced_of(&[("m", &grown)]);
    let kept: std::collections::HashSet<&str> =
        numbering_support::code(&large.text).lines().collect();
    let moved: Vec<&str> = numbering_support::code(&small.text)
        .lines()
        .filter(|l| !kept.contains(l))
        .collect();
    assert!(
        moved.is_empty(),
        "adding `zz` changed lines of the unit that are not its own:\n{}",
        moved.join("\n")
    );
    let _ = numbering_support::body(&large.text, &numbering_support::symbol(&large, "m.zz"));
    assert_eq!(large.exports.consts.len(), small.exports.consts.len() + 1);
    assert_eq!(large.exports.fields, small.exports.fields);
    assert_eq!(large.exports.builtins, small.exports.builtins);
    assert_eq!(large.exports.shapes, small.exports.shapes);
    assert!(
        large.exports.lambdas.starts_with(&small.exports.lambdas),
        "the code table was not appended to: {:?} then {:?}",
        small.exports.lambdas,
        large.exports.lambdas
    );
}

#[test]
fn a_constant_two_definitions_share_is_one_entry_of_the_pool() {
    let source = r#"
pub fn one(n: Int) -> Bytes = if n > 0 { b"shared-constant" } else { b"one" }
pub fn two(n: Int) -> Bytes = if n > 0 { b"shared-constant" } else { b"two" }
"#;
    let produced = produced_of(&[("m", source)]);
    let shared: Vec<usize> = produced
        .exports
        .consts
        .iter()
        .enumerate()
        .filter(|(_, v)| matches!(v, ply_eval::Value::Bytes(b) if &b[..] == b"shared-constant"))
        .map(|(i, _)| i)
        .collect();
    let [at] = shared.as_slice() else {
        panic!("the pool holds the constant {} times", shared.len());
    };
    for name in ["m.one", "m.two"] {
        let symbol = numbering_support::symbol(&produced, name);
        let body = numbering_support::body(&produced.text, &symbol);
        assert!(
            numbering_support::literal_places(&produced, body).contains(&(*at as u32)),
            "`{name}` does not read the shared constant from its one entry:\n{body}"
        );
    }
}

/// The unit is one translation unit that is also a partition: its parts concatenate back to it,
/// and each body sits once, in the bucket its name alone decides.
#[test]
fn a_unit_splits_into_its_buckets_and_each_body_sits_in_its_name_s_bucket() {
    let source = r#"
type Pair = { left: Int, right: Int }
fn key(x: Int) -> Int = x + 1
pub fn sum(xs: List<Int>) -> Int = fold(map(xs, key), 0, |a: Int, x: Int| a + x)
pub fn pick(p: Pair) -> Bytes = if p.left > p.right { b"left" } else { b"right" }
pub fn steady() -> Int = 7
pub fn twice(x: Int) -> Int = key(key(x))
"#;
    let produced = produced_of(&[("m", source)]);
    let parts = split(&produced.text).expect("the unit splits on its marks");
    let joined: String = std::iter::once(parts.header)
        .chain(parts.buckets.iter().map(|(_, text)| *text))
        .chain(std::iter::once(parts.tail))
        .collect();
    assert_eq!(
        joined, produced.text,
        "the parts do not concatenate to the unit"
    );
    assert!(parts.tail.starts_with(RUNTIME_MARK));
    assert!(parts.tail.contains("void ply_bind("));
    assert!(parts.header.contains("extern Word ply_true"));
    for (id, _) in &parts.buckets {
        assert!(u64::from(*id) < BUCKETS);
    }
    for name in produced.exports.taken.iter().map(|t| t.name.clone()) {
        let definition = format!(
            "Word {}(PlyCtx *ctx",
            numbering_support::symbol(&produced, &name)
        );
        assert!(
            !parts.header.contains(&definition) && !parts.tail.contains(&definition),
            "`{name}` sits outside the buckets"
        );
        let holding: Vec<u8> = parts
            .buckets
            .iter()
            .filter(|(_, text)| text.contains(&definition))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            holding,
            vec![bucket_of(&name)],
            "the buckets holding `{name}`"
        );
        let copies: usize = parts
            .buckets
            .iter()
            .map(|(_, text)| text.matches(&definition).count())
            .sum();
        assert_eq!(copies, 1, "`{name}` is defined {copies} times");
    }
}

/// A body edited and no table moved: of the objects the unit links, the bucket holding that
/// body is the one compiled again; the other bucket and the runtime's object are reused.
#[test]
fn editing_one_body_compiles_its_bucket_alone_and_links_the_rest_from_the_cache() {
    let nonce = nonce();
    let edited = apart_from_steady();
    let program = |k: u128| {
        format!(
            "pub fn steady(x: Int) -> Int = x + {nonce}\n\
             pub fn {edited}(x: Int) -> Int = x * {k}\n"
        )
    };
    // Answered before the cache moves, so the builder's own objects stay where they are.
    let (three, five) = (
        fixture::answered(&[("m", &program(3))]),
        fixture::answered(&[("m", &program(5))]),
    );
    in_own_cache(|cache| {
        let parts = split(&three.unit).expect("the unit splits on its marks");
        assert_eq!(parts.buckets.len(), 2, "two names in two buckets");
        let expected = parts.buckets.len() + 1;
        let (cold_compiled, cold_objects) = (compiled(), objects(cache));
        let Some((_, first, _)) = fixture::load(three) else {
            return;
        };
        assert_eq!(
            (compiled() - cold_compiled, objects(cache) - cold_objects),
            (expected, expected),
            "a cold build compiles every bucket and the runtime's object"
        );
        let (before_compiled, before_objects) = (compiled(), objects(cache));
        let (_, second, _) = fixture::load(five).expect("the compiler ran once already");
        assert_eq!(
            (
                compiled() - before_compiled,
                objects(cache) - before_objects
            ),
            (1, 1),
            "editing one body compiled more than the bucket holding it"
        );
        assert_eq!(
            answer(&first, "m.steady", 1),
            answer(&second, "m.steady", 1),
            "`steady` changed under the edit"
        );
        let changed = format!("m.{edited}");
        assert_eq!(answer(&first, &changed, 4), 12);
        assert_eq!(
            answer(&second, &changed, 4),
            20,
            "the edit did not reach the image"
        );
    });
}

/// A definition nobody calls, added in a bucket of its own: that bucket compiles, and the
/// runtime's object, which embeds the exports; the bucket holding the other body is the same
/// text as before, since no part outside a bucket declares a definition, and is reused.
#[test]
fn adding_a_definition_compiles_its_bucket_and_the_runtime_object_alone() {
    let nonce = nonce();
    let added = apart_from_steady();
    let base = format!("pub fn steady(x: Int) -> Int = x + {nonce}\n");
    let grown = format!("{base}pub fn {added}(x: Int) -> Int = x * 3\n");
    let (small, large) = (
        fixture::answered(&[("m", &base)]),
        fixture::answered(&[("m", &grown)]),
    );
    in_own_cache(|cache| {
        let steadys = |unit: &Produced| -> String {
            let parts = split(&unit.text).expect("the unit splits on its marks");
            parts
                .buckets
                .iter()
                .find(|(id, _)| *id == bucket_of("m.steady"))
                .map(|(_, text)| text.to_string())
                .expect("`steady` has a bucket")
        };
        assert_eq!(
            steadys(&Produced::of(&small)),
            steadys(&Produced::of(&large)),
            "adding `{added}` changed the bucket holding `steady`"
        );
        let Some((_, first, _)) = fixture::load(small) else {
            return;
        };
        let before = (compiled(), reused(), objects(cache));
        let (_, second, _) = fixture::load(large).expect("the compiler ran once already");
        assert_eq!(
            (
                compiled() - before.0,
                reused() - before.1,
                objects(cache) - before.2
            ),
            (2, 1, 2),
            "adding a definition compiled more than its bucket and the runtime's object"
        );
        assert_eq!(
            answer(&first, "m.steady", 1),
            answer(&second, "m.steady", 1),
            "`steady` changed under the addition"
        );
        assert_eq!(answer(&second, &format!("m.{added}"), 4), 12);
    });
}

/// A name in another bucket than `m.steady`'s: an assertion about one bucket would hold
/// trivially with both bodies in one.
fn apart_from_steady() -> &'static str {
    [
        "changed", "altered", "revised", "turned", "moved", "shifted", "swapped",
    ]
    .into_iter()
    .find(|n| bucket_of(&format!("m.{n}")) != bucket_of("m.steady"))
    .expect("seven names do not all share `steady`'s bucket")
}

/// Runs `test` over a cache of its own, holding the configuration for writing: the counters
/// count every build in the process.
fn in_own_cache(test: impl FnOnce(&std::path::Path)) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let _config = fixture::CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let restore = std::env::var("PLY_C_CACHE").ok();
    unsafe { std::env::set_var("PLY_C_CACHE", dir.path()) };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(dir.path())));
    unsafe {
        match &restore {
            Some(had) => std::env::set_var("PLY_C_CACHE", had),
            None => std::env::remove_var("PLY_C_CACHE"),
        }
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

fn compiled() -> usize {
    ply_codegen::c::BUCKETS_COMPILED.load(std::sync::atomic::Ordering::Relaxed)
}

fn reused() -> usize {
    ply_codegen::c::BUCKETS_REUSED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The objects under `cache`'s `obj/`.
fn objects(cache: &std::path::Path) -> usize {
    std::fs::read_dir(cache.join("obj"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "o"))
                .count()
        })
        .unwrap_or(0)
}

fn answer(native: &Native, name: &str, x: i64) -> i64 {
    let entry: ply_codegen::rt::Entry = native.entry(name).expect("compiled");
    let mut ctx = native.context();
    ctx.fuel = 10_000;
    let words = [ply_codegen::heap::imm(x)];
    let w = unsafe { entry(&mut ctx, words.as_ptr()) };
    assert_eq!(ctx.failed, 0, "`{name}` raised");
    ply_codegen::heap::imm_value(w)
}

/// A module's name is its path, so `m_a/b.ply` and `m/a_b.ply` are a program apart; a symbol that
/// turned a dot into `_` spelled both of them `ply_m_a_b`, and the unit held one definition of it.
#[test]
fn two_definitions_a_dot_apart_answer_as_themselves() {
    let made = fixture::answered(&[
        ("m_a", "pub fn b(x: Int) -> Int = x + 1\n"),
        ("m", "pub fn a_b(x: Int) -> Int = x + 2\n"),
    ]);
    let produced = Produced::of(&made);
    let apart = produced
        .exports
        .taken_by_name("m_a.b")
        .expect("`b` is taken");
    let together = produced
        .exports
        .taken_by_name("m.a_b")
        .expect("`a_b` is taken");
    assert_ne!(
        apart.symbol, together.symbol,
        "`m_a.b` and `m.a_b` are emitted as one C function"
    );
    let Some((_, native, refused)) = fixture::loaded(made) else {
        return;
    };
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(answer(&native, "m_a.b", 10), 11);
    assert_eq!(answer(&native, "m.a_b", 10), 12);
}

/// The unit binds and calls by the symbols it publishes, so each one has to be the symbol its C
/// defines, and no two definitions may share one.
#[test]
fn a_unit_publishes_the_symbols_its_c_defines() {
    let produced = produced_of(&[
        ("m_a", "pub fn b(x: Int) -> Int = x + 1\n"),
        (
            "m",
            r#"
fn key(x: Int) -> Int = x + 1
pub fn a_b(xs: List<Int>) -> Int = fold(map(xs, key), 0, |a: Int, x: Int| a + x)
pub fn ping(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { pong(n - 1, acc + 1) }
fn pong(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { ping(n - 1, acc + 2) }
pub fn steady() -> Int = 7
"#,
        ),
    ]);
    let code = numbering_support::code(&produced.text);
    let mut spelled: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for taken in &produced.exports.taken {
        assert!(
            code.contains(&format!("Word {}(PlyCtx *ctx", taken.symbol)),
            "the unit publishes `{}` for `{}` and defines no such function",
            taken.symbol,
            taken.name
        );
        assert!(
            code.contains(&format!(
                "Word {}(PlyCtx *ctx, const Word *args)",
                taken.entry
            )),
            "the unit publishes `{}` as the entry of `{}` and defines no such function",
            taken.entry,
            taken.name
        );
        if let Some(other) = spelled.insert(taken.symbol.as_str(), taken.name.as_str()) {
            panic!("`{other}` and `{}` are both `{}`", taken.name, taken.symbol);
        }
    }
}

/// A bucket declares what its bodies reach: their own names, a group's members, and every
/// call, a lambda's and a definition taken as a value included; a definition none of them
/// reach is not declared there, or anywhere outside a bucket.
#[test]
fn a_bucket_declares_what_its_bodies_reach_and_no_other_definition() {
    let source = r#"
fn key(x: Int) -> Int = x + 1
pub fn sum(xs: List<Int>) -> Int = fold(map(xs, key), 0, |a: Int, x: Int| a + x)
pub fn each(xs: List<Int>) -> List<Int> = map(xs, |x: Int| key(x))
pub fn twice(x: Int) -> Int = key(key(x))
pub fn apart(x: Int) -> Int = x * 2
fn ping(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { pong(n - 1, acc + 1) }
fn pong(n: Int, acc: Int) -> Int = if n <= 0 { acc } else { ping(n - 1, acc + 2) }
pub fn volley(n: Int) -> Int = ping(n, 0)
"#;
    let produced = produced_of(&[("m", source)]);
    let parts = split(&produced.text).expect("the unit splits on its marks");
    let group = ["m.ping", "m.pong"]
        .into_iter()
        .find(|m| {
            produced.text.contains(&format!(
                "static Word {}_group(",
                numbering_support::symbol(&produced, m)
            ))
        })
        .expect("`ping` and `pong` are a group");
    // Each body placed, by the name it sits under, with what it calls.
    let placed: [(&str, &[&str]); 7] = [
        ("m.key", &[]),
        ("m.sum", &["m.key"]),
        ("m.each", &["m.key"]),
        ("m.twice", &["m.key"]),
        ("m.apart", &[]),
        (group, &["m.ping", "m.pong"]),
        ("m.volley", &["m.ping"]),
    ];
    let names = [
        "m.key", "m.sum", "m.each", "m.twice", "m.apart", "m.ping", "m.pong", "m.volley",
    ];
    let prototype = |name: &str| -> String {
        let taken = produced
            .exports
            .taken_by_name(name)
            .expect("every definition is taken");
        let params = std::iter::once("PlyCtx*")
            .chain(std::iter::repeat_n("Word", taken.arity))
            .collect::<Vec<_>>()
            .join(", ");
        format!("Word {}({params});\n", taken.symbol)
    };
    for name in names {
        assert!(
            !parts.header.contains(&prototype(name)) && !parts.tail.contains(&prototype(name)),
            "`{name}` is declared outside the buckets"
        );
    }
    let mut ids: Vec<u8> = placed.iter().map(|(name, _)| bucket_of(name)).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        parts.buckets.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ids,
        "the buckets are the placed bodies' buckets, in order"
    );
    for (id, text) in &parts.buckets {
        let reached: std::collections::HashSet<&str> = placed
            .iter()
            .filter(|(name, _)| bucket_of(name) == *id)
            .flat_map(|(name, calls)| std::iter::once(*name).chain(calls.iter().copied()))
            .collect();
        for name in names {
            assert_eq!(
                text.contains(&prototype(name)),
                reached.contains(name),
                "bucket {id:02x} and `{name}`:\n{text}"
            );
        }
    }
}

/// A label binder is a trailing `Word` of the emitted body, so every reader of the published
/// arity has to count it: a caller in another bucket writes the prototype from that arity, and a
/// narrower one is a redefinition the C compiler refuses for the whole unit.
#[test]
fn a_label_generic_body_and_a_caller_in_another_bucket_agree_on_its_width() {
    let caller = [
        "driver", "sender", "harness", "runner", "outer", "wrapper", "feeder",
    ]
    .into_iter()
    .find(|n| bucket_of(&format!("m.{n}")) != bucket_of("m.relay"))
    .expect("seven names do not all share `relay`'s bucket");
    let source = format!(
        r#"
effect net {{
  write send[s](payload: Int) -> Int
}}

fn relay<[l]>(payload: Int) -> Int / {{net.send[l]}} = net.send[l](payload)

pub fn {caller}(x: Int) -> Int =
  handle {{ relay[conn](x) }} with {{ net.send[conn](p) -> p + 1 }}
"#
    );
    let emitted = produced_of(&[("m", &source)]);
    let relay = emitted
        .exports
        .taken_by_name("m.relay")
        .expect("`relay` is taken");
    assert_eq!(relay.arity, 2, "`relay` takes its payload and its label");
    let parts = split(&emitted.text).expect("the unit splits on its marks");
    let prototype = format!("Word {}(PlyCtx*, Word, Word);\n", relay.symbol);
    let (_, bucket) = parts
        .buckets
        .iter()
        .find(|(id, _)| *id == bucket_of(&format!("m.{caller}")))
        .expect("the caller's bucket");
    assert!(
        bucket.contains(&prototype),
        "the caller's bucket declares `relay` narrower than it is emitted:\n{bucket}"
    );
    let Some(native) = built(&source) else {
        return;
    };
    assert_eq!(answer(&native, &format!("m.{caller}"), 1), 2);
}

/// One recursive group is checked with one label binder, so a call to a sibling passes the
/// caller's label along: the pair answers under whichever resource fills it, and the call stays
/// the jump a self call is — 100_000 of them inside one entry's fuel.
#[test]
fn a_sibling_call_carries_the_groups_label_through_its_jump() {
    let source = r#"
effect net {
  write send[s](payload: Int) -> Int
}

fn ping<[l]>(n: Int) -> Int / {net.send[l]} = if n <= 0 { net.send[l](0) } else { pong(n - 1) }
fn pong<[k]>(n: Int) -> Int / {net.send[k]} = if n <= 0 { net.send[k](1) } else { ping(n - 1) }

pub fn near(n: Int) -> Int = handle { ping[conn](n) } with { net.send[conn](p) -> p + 10 }
pub fn far(n: Int) -> Int = handle { ping[upstream](n) } with { net.send[upstream](p) -> p + 20 }
"#;
    let Some(native) = built(source) else {
        return;
    };
    assert_eq!(answer(&native, "m.near", 0), 10);
    assert_eq!(answer(&native, "m.near", 1), 11);
    assert_eq!(answer(&native, "m.far", 0), 20);
    assert_eq!(answer(&native, "m.far", 1), 21);
    assert_eq!(answer(&native, "m.near", 100_000), 10);
    assert_eq!(answer(&native, "m.far", 100_001), 21);
}

/// The members of a recursive group share one C function, so a tail call between them is a jump:
/// the fuel is far below the calls made, and a call that nested would spend it first.
#[test]
fn a_tail_call_between_members_of_a_recursive_group_is_a_jump() {
    let source = r#"
fn even(n: Int) -> Bool = if n <= 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n <= 0 { false } else { even(n - 1) }
pub fn parity(n: Int) -> Bool = even(n)
"#;
    let Some((loaded, native)) = fixture::unit(source) else {
        return;
    };
    let answer = |name: &str, n: i64| -> ply_codegen::heap::Word {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let args = [ply_codegen::heap::imm(n)];
        let w = unsafe { entry(&mut ctx, args.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}({n})` raised");
        w
    };
    assert_eq!(answer("m.even", 10_000_000), ply_codegen::heap::bool(true));
    assert_eq!(answer("m.even", 10_000_001), ply_codegen::heap::bool(false));
    // Entered from outside the group, a member answers through its own symbol.
    assert_eq!(answer("m.odd", 10_000_001), ply_codegen::heap::bool(true));
    assert_eq!(answer("m.parity", 7), ply_codegen::heap::bool(false));
    let _ = loaded;
}

/// A parameter the prologue converted — an `Int` unboxed at entry, so the body reads the converted
/// local and never the raw word again — is the call's, not the pass's. A self tail call restarts as
/// often as its counter likes and the epilogue runs once, so the raw word has to be released
/// exactly once. It was released on every restart as well, which freed a boxed `Int` (|v| >= 2^62,
/// the first value that does not fit an immediate) on the first restart and read it again on the
/// way out. Immediates hid it, and so did every `List`, `String` and record, whose words the move
/// spends rather than releases.
#[test]
fn a_self_tail_call_releases_a_converted_parameter_once() {
    let source = r#"
fn countdown(n: Int, k: Int) -> Int = if k <= 0 { n } else { countdown(n, k - 1) }
fn flip(b: Bool, k: Int) -> Bool = if k <= 0 { b } else { flip(b, k - 1) }
fn narrow(w: U32, k: Int) -> U32 = if k <= 0 { w } else { narrow(w, k - 1) }
pub fn down(k: Int) -> Int = countdown(4611686018427387904, k)
pub fn toggled(k: Int) -> Bool = flip(true, k)
pub fn narrowed(k: Int) -> U32 = narrow(7u32, k)
"#;
    let Some((_, native)) = fixture::unit(source) else {
        return;
    };
    let produced = produced_of(&[("m", source)]);
    let code = numbering_support::code(&produced.text);
    // Every prologue conversion -- `Int`, `Bool` and a sized integer alike -- leaves a raw word the
    // loop no longer reads, and each is released once, on the way out.
    for (name, raw) in [("m.countdown", "p0"), ("m.flip", "p0"), ("m.narrow", "p0")] {
        let body = numbering_support::body(code, &numbering_support::symbol(&produced, name));
        assert_eq!(
            body.matches(&format!("ply_dec(ctx, {raw})")).count(),
            1,
            "`{name}` releases its converted parameter {raw} other than once, on the way out:\n{body}"
        );
    }
    // And the value itself survives the loop, which is what the release discipline is for.
    let entry: ply_codegen::rt::Entry = native
        .entry("m.down")
        .unwrap_or_else(|| panic!("`down` was not compiled"));
    for k in [1_i64, 1_000] {
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let args = [ply_codegen::heap::imm(k)];
        let w = unsafe { entry(&mut ctx, args.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`down({k})` raised");
        assert_eq!(
            ply_codegen::heap::as_int(w),
            Some(4_611_686_018_427_387_904),
            "`down({k})` lost the boxed parameter"
        );
    }
    // The `Bool` and `U32` loops answer what they were handed, too.
    let run = |name: &str| -> ply_codegen::heap::Word {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let args = [ply_codegen::heap::imm(1_000)];
        let w = unsafe { entry(&mut ctx, args.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        w
    };
    assert_eq!(run("m.toggled"), ply_codegen::heap::bool(true));
    assert_eq!(ply_codegen::heap::imm_value(run("m.narrowed")), 7);
}

/// A `handle` lands failures on its own label, so a cycle holding one is emitted definition by definition.
#[test]
fn a_recursive_group_holding_a_handle_is_emitted_per_definition() {
    let source = r#"
fn a(n: Int) -> Int / {clock.now, diverges} = if n == 0 { 0 } else if n == 1 { handle { b(0) } with { clock.now() -> Instant(7), } } else { b(n - 1) }
fn b(n: Int) -> Int / {clock.now, diverges} = if n == 0 { match clock.now() { Instant(t) -> t } } else { a(n - 1) }
"#;
    let produced = produced_of(&[("m", source)]);
    let code = numbering_support::code(&produced.text);
    assert!(
        !code.contains("_group(PlyCtx *ctx, int which"),
        "a member holding a `handle` was grouped:\n{code}"
    );
    for name in ["m.a", "m.b"] {
        let _ = numbering_support::body(code, &numbering_support::symbol(&produced, name));
    }
    let Some((_, native, refused)) = fixture::with_refusals(source) else {
        return;
    };
    assert!(refused.is_empty(), "{refused:?}");
    let entry: ply_codegen::rt::Entry = native.entry("m.a").expect("`a` was not compiled");
    let mut ctx = native.context();
    ctx.fuel = 10_000;
    let args = [ply_codegen::heap::imm(5)];
    let w = unsafe { entry(&mut ctx, args.as_ptr()) };
    assert_eq!(ctx.failed, 0, "`a` raised");
    assert_eq!(ply_codegen::heap::imm_value(w), 7);
}

/// A pure computation whose value is only observed is still run. `observe` goes through a helper
/// the C compiler cannot see into, so it cannot drop the calls and answer the constant.
#[test]
fn an_observed_pure_computation_is_not_optimized_away() {
    const SOURCE: &str = r#"
fn step(n: Int) -> Int = n + 1

fn work(n: Int) -> Int = step(step(step(n)))

fn bench(n: Int) -> Int = {
  let _ = observe(work(n));
  0
}
"#;
    let Some((_source, native)) = fixture::unit(SOURCE) else {
        return;
    };
    let entry: ply_codegen::rt::Entry = native.entry("m.bench").expect("`m.bench` compiled");
    let mut ctx = native.context();
    ctx.begin(1_000);
    let args = [ply_codegen::heap::imm(1)];
    let answer = unsafe { entry(&mut ctx, args.as_ptr()) };
    let (failed, ticks) = (ctx.failed, ctx.ticks);
    ctx.end();
    assert_eq!(failed, 0, "`m.bench` raised");
    assert_eq!(ply_codegen::heap::imm_value(answer), 0);
    assert!(
        ticks >= 4,
        "the observed call was optimized away: {ticks} calls made"
    );
}
