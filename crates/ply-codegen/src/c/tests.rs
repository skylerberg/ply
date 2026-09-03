//! What the emitted C must agree with, checked rather than commented.

use super::*;

/// Every helper the prelude declares has an address, and the two tables are the same length: a
/// declaration with no address is a null call at run time, which is a crash rather than a decline.
#[test]
fn every_declared_helper_has_an_address() {
    let addrs = helper_addresses();
    assert_eq!(addrs.len(), HELPERS.len());
    for (h, a) in HELPERS.iter().zip(&addrs) {
        assert!(!a.is_null(), "`{}` has no address", h.name);
    }
}

/// The layouts the prelude mirrors, against the Rust they mirror. A header that drifts is a wrong
/// answer, not a slow one.
#[test]
fn the_prelude_agrees_with_the_layouts_it_mirrors() {
    assert_eq!(crate::heap::HEADER, 16, "PLY_HEADER");
    assert_eq!(std::mem::size_of::<crate::heap::Obj>(), 16, "PlyObj");
    assert_eq!(crate::heap::FLAT, 1, "PLY_FLAT");
    assert_eq!(
        std::mem::offset_of!(crate::rt::Ctx, failed),
        0,
        "PlyCtx.failed"
    );
    assert_eq!(std::mem::offset_of!(crate::rt::Ctx, fuel), 8, "PlyCtx.fuel");
    assert!(PRELUDE.contains("#define PLY_HEADER 16"));
}

/// The whole pipeline, on the smallest unit there is: emit, compile, load, bind, call.
#[test]
fn a_unit_compiles_loads_binds_and_answers() {
    let mut src = String::from(PRELUDE);
    src.push_str(&runtime_decls());
    src.push_str(
        r#"
Word ply_probe(PlyCtx *ctx, const Word *args) {
  (void)ctx;
  return ply_imm(ply_imm_value(args[0]) + ply_imm_value(args[1]));
}
"#,
    );
    let lib = match load::compile_and_load(&src, "probe") {
        Ok(l) => l,
        // A machine with no C compiler is a machine this tier is not for; the test says so
        // rather than failing the suite for everyone.
        Err(e) if e.to_string().contains("could not run") => return,
        Err(e) => panic!("{e}"),
    };
    let bind = lib.symbol("ply_bind").expect("the unit exports `ply_bind`");
    let bind: unsafe extern "C" fn(*const *mut std::ffi::c_void) =
        unsafe { std::mem::transmute(bind) };
    let addrs = helper_addresses();
    unsafe { bind(addrs.as_ptr()) };
    let probe = lib
        .symbol("ply_probe")
        .expect("the unit exports `ply_probe`");
    let probe: crate::jit::Entry = unsafe { std::mem::transmute(probe) };
    let args = [crate::heap::imm(20), crate::heap::imm(22)];
    let answer = unsafe { probe(std::ptr::null_mut(), args.as_ptr()) };
    assert_eq!(crate::heap::imm_value(answer), 42);
}

/// The whole tier on a real program: emit, compile, load, and answer what the interpreter answers.
#[test]
fn the_tier_answers_what_the_interpreter_answers() {
    let source = r#"
fn double(x: Int) -> Int = x * 2
fn even(x: Int) -> Bool = x % 2 == 0
fn clamp(x: Int, lo: Int, hi: Int) -> Int =
  if x < lo { lo } else { if x > hi { hi } else { x } }
fn collatz(n: Int) -> Int =
  if n <= 1 { 0 } else { if even(n) { 1 + collatz(n / 2) } else { 1 + collatz(3 * n + 1) } }
pub fn width(a: Int, b: Int) -> Int =
  int_of_u32(wrap_add(u32_of_int(a), u32_of_int(b)) ^ rotr(u32_of_int(b), 8))
pub fn shaped(n: Int) -> Int = { let r = {x: n, y: n + 1}; r.x * 10 + r.y }
"#;
    let Some((loaded, native)) = tests_support::unit(source) else {
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
        let entry: crate::jit::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let words: Vec<i64> = args.iter().map(|a| crate::heap::imm(*a)).collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        assert_eq!(
            crate::heap::imm_value(answer),
            *want,
            "`{name}{args:?}` answered wrongly"
        );
    }
    let _ = loaded;
}

/// Loading a module the way the tests need it: parse, resolve, check, then build a unit over
/// every function in it. `None` on a machine with no C compiler, which this tier is not for.
pub mod tests_support {
    use crate::c::Native;
    use crate::source::Source;
    use ply_syntax::ast::ModuleName;

    pub fn unit(text: &str) -> Option<(&'static Source, Native)> {
        let mut sources = ply_span::SourceMap::new();
        let owned: &'static str = Box::leak(text.to_string().into_boxed_str());
        let id = sources.add("m.ply", owned.to_string());
        let mut ast =
            ply_syntax::parse_program([(id, ModuleName::from_dotted("m"), owned)]).expect("parses");
        let resolved = ply_syntax::resolve::resolve(&mut ast).expect("resolves");
        let check = ply_core::check_program(&ast, &resolved).expect("checks");
        let source: &'static Source = Box::leak(Box::new(Source::new(
            Box::leak(Box::new(ast)),
            Box::leak(Box::new(resolved)),
            Box::leak(Box::new(check)),
        )));
        let names = source.functions();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        match crate::c::build(source, &refs, crate::jit::Opts::default()) {
            Ok((native, _refused)) => Some((source, native)),
            Err(e) if e.to_string().contains("could not run") => None,
            Err(e) => panic!("{e}"),
        }
    }
}

/// The two tiers answer the same thing over the constructs the fragment carries. This is the
/// property that matters: a second code generator is a second chance to be wrong, and the only
/// defence is that it is checked against the first and against the machine.
#[test]
fn the_two_tiers_agree() {
    let source = r#"
type Quad = { a: U32, b: U32, c: U32, d: U32 }
fn g(q: Quad, mx: U32) -> Quad = {
  let a1 = wrap_add(wrap_add(q.a, q.b), mx);
  let d1 = rotr(q.d ^ a1, 16);
  let c1 = wrap_add(q.c, d1);
  let b1 = rotr(q.b ^ c1, 12);
  {a: a1, b: b1, c: c1, d: d1}
}
pub fn mixed(n: Int) -> Int = {
  let w = u32_of_int(n);
  let q = g({a: w, b: 1u32, c: 0x3C6E_F372u32, d: 0xA54F_F53Au32}, w);
  int_of_u32(q.a ^ q.b ^ q.c ^ q.d)
}
pub fn counted(n: Int) -> Int =
  iterate({i: 0, acc: 0}, n + 1, |s: {i: Int, acc: Int}|
    if s.i >= n { Stop(s.acc) } else { Continue({i: s.i + 1, acc: s.acc + s.i * s.i}) })
pub fn bytes_sum(b: Bytes) -> Int =
  iterate({i: 0, acc: 0}, bytes_len(b) + 1, |s: {i: Int, acc: Int}|
    if s.i >= bytes_len(b) { Stop(s.acc) }
    else { Continue({i: s.i + 1, acc: s.acc + bytes_at(b, s.i)}) })
pub fn shifted(a: Int, n: Int) -> Int = (a << n) + (a >> n) + (a >>> n)
pub fn matched(n: Int) -> Int = match n { 0 -> 100, 1 -> 200, _ -> n * 3 }
"#;
    let Some((loaded, native)) = tests_support::unit(source) else {
        return;
    };
    let mut machine = ply_eval::Machine::new(loaded.program, loaded.resolved, loaded.check);
    let cases: &[(&str, Vec<ply_eval::Value>)] = &[
        ("m.mixed", vec![ply_eval::Value::Int(0xDEAD_BEEF)]),
        ("m.mixed", vec![ply_eval::Value::Int(0)]),
        ("m.counted", vec![ply_eval::Value::Int(40)]),
        (
            "m.bytes_sum",
            vec![ply_eval::Value::bytes(b"the quick brown fox")],
        ),
        (
            "m.shifted",
            vec![ply_eval::Value::Int(-9), ply_eval::Value::Int(3)],
        ),
        ("m.matched", vec![ply_eval::Value::Int(0)]),
        ("m.matched", vec![ply_eval::Value::Int(1)]),
        ("m.matched", vec![ply_eval::Value::Int(7)]),
    ];
    for (name, args) in cases {
        let want = machine
            .call(name, args.clone(), ply_span::Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
        let entry: crate::jit::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts_ptr: *const crate::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts_ptr }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = crate::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(got, want, "`{name}{args:?}`: the tiers disagree");
    }
}
