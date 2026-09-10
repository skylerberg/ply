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
    assert_eq!(
        std::mem::offset_of!(crate::rt::Ctx, stack_floor),
        16,
        "PlyCtx.stack_floor"
    );
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
    let probe: crate::rt::Entry = unsafe { std::mem::transmute(probe) };
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
        let entry: crate::rt::Entry = native
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
/// Process-wide state a build reads or writes, which a test binary shares between its threads.
///
/// Two kinds. `PLY_C_CACHE` and `PLY_C_SKIP` are read from the environment on whichever thread
/// reaches them -- rayon workers included -- so a test that changes one changes it under every
/// build running beside it. `cache::UNITS_REUSED` is a counter every build adds to, so a test
/// that reads it before and after its own build is measuring the whole binary.
///
/// Under `cargo nextest` neither can bite, because each test is its own process; under
/// `cargo test` every test in this binary shares one, and both did -- a build picked up another
/// test's cache directory and failed to `dlopen` what it had just written.
///
/// So: a test that changes the environment, or counts what a build did, takes [`CONFIG`] for
/// writing; every other build here takes it for reading.
pub(super) static CONFIG: std::sync::RwLock<()> = std::sync::RwLock::new(());

pub mod tests_support {
    use crate::c::Native;
    use crate::source::Source;
    use ply_syntax::ast::ModuleName;

    pub fn unit(text: &str) -> Option<(&'static Source, Native)> {
        with_refusals(text).map(|(s, n, _)| (s, n))
    }

    /// A machine over `source` with the default tier attached, so what the reference fragment
    /// answers is checked against what the whole emitter answers.
    pub fn machine(source: &'static Source, text: &str) -> ply_eval::Machine<'static> {
        crate::c::producer::ensure_default();
        let texts = std::collections::HashMap::from([("m".to_string(), text.to_string())]);
        let unit = {
            let _config = super::CONFIG.read().unwrap_or_else(|e| e.into_inner());
            crate::Unit::over_with_texts(source.program, source.resolved, source.check, texts)
                .expect("this host has a C compiler")
        };
        let mut machine = ply_eval::Machine::new(source.program, source.resolved, source.check);
        let spec = ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
            ..Default::default()
        };
        machine.set_compiled(ply_eval::Provider::attach(unit, &spec));
        machine
    }

    /// The same, with a key per definition so the emit cache is live.
    ///
    /// The key has to move when the text does. A name alone is stable and distinct, which is all
    /// the cache asks of the *shape* of a key, but two tests that both define `m.f` would then
    /// share an entry and the second would be served the first one's C.
    pub fn keyed(text: &str) -> Option<&'static Source> {
        let mut sources = ply_span::SourceMap::new();
        let owned: &'static str = Box::leak(text.to_string().into_boxed_str());
        let id = sources.add("m.ply", owned.to_string());
        let mut ast =
            ply_syntax::parse_program([(id, ModuleName::from_dotted("m"), owned)]).expect("parses");
        let resolved = ply_syntax::resolve::resolve(&mut ast).expect("resolves");
        let check = ply_core::check_program(&ast, &resolved).expect("checks");
        let bare = Source::new(
            Box::leak(Box::new(ast)),
            Box::leak(Box::new(resolved)),
            Box::leak(Box::new(check)),
        );
        let stamp = blake3::hash(text.as_bytes()).to_hex();
        let keys = bare
            .functions()
            .into_iter()
            .map(|n| (n.clone(), format!("h-{n}-{}", &stamp[..16])))
            .collect();
        let program = bare.program;
        let resolved = bare.resolved;
        let check = bare.check;
        Some(Box::leak(Box::new(Source::keyed(
            program, resolved, check, keys,
        ))))
    }

    pub fn with_refusals(text: &str) -> Option<(&'static Source, Native, Vec<super::Refused>)> {
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
        let _config = super::CONFIG.read().unwrap_or_else(|e| e.into_inner());
        match crate::c::producer::reference_only(|| crate::c::build(source, &refs)) {
            Ok((native, refused)) => Some((source, native, refused)),
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
pub fn looped(n: Int) -> Int =
  iterate({i: 0, q: {a: 1u32, b: 2u32, c: 3u32, d: 4u32}}, n + 1, |s: {i: Int, q: Quad}|
    if s.i >= n { Stop(int_of_u32(s.q.a ^ s.q.b ^ s.q.c ^ s.q.d)) }
    else { Continue({i: s.i + 1, q: g(s.q, u32_of_int(s.i))}) })
"#;
    let Some((loaded, native)) = tests_support::unit(source) else {
        return;
    };
    let mut machine = tests_support::machine(loaded, source);
    let cases: &[(&str, Vec<ply_eval::Value>)] = &[
        ("m.mixed", vec![ply_eval::Value::Int(0xDEAD_BEEF)]),
        ("m.mixed", vec![ply_eval::Value::Int(0)]),
        ("m.counted", vec![ply_eval::Value::Int(40)]),
        // A record built at one site but *described* once per iteration. The tier holds such a
        // record back and builds it on demand, and the build has to describe a new one each time
        // round -- reusing the first iteration's object is a wrong answer, not a crash, and it
        // takes an input long enough to loop before anything notices.
        ("m.looped", vec![ply_eval::Value::Int(1)]),
        ("m.looped", vec![ply_eval::Value::Int(7)]),
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
        let entry: crate::rt::Entry = native
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

/// A `U64` past `2^62` is not an immediate: tagging one eats its top bit, so `carried` stops
/// below sixty-four and a value of the two widths past it is held as the machine's own value. An
/// operator over one reaches the machine's operator through the runtime rather than a register,
/// and the property is that the answer is the machine's -- a tier that emits a body it cannot get
/// right is worse than one that declines it, because the seam has an interpreter behind it and no
/// way to know it is needed.
#[test]
fn a_width_the_tier_cannot_carry_in_a_register_still_answers_what_the_machine_answers() {
    let source = r#"
pub fn wide(n: Int) -> Int = {
  let a = u64_of_int(n);
  let b = wrap_mul(wrap_add(a, a), 0x9E37_79B9_7F4A_7C15u64);
  int_of_u64(rotr(b, 7) & 0xFFFFu64)
}
pub fn narrow(n: Int) -> Int = int_of_u32(rotr(wrap_mul(u32_of_int(n), 2654435761u32), 7))
"#;
    let Some((loaded, native, _)) = tests_support::with_refusals(source) else {
        return;
    };
    let mut machine = tests_support::machine(loaded, source);
    for name in ["m.wide", "m.narrow"] {
        let entry: crate::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        // `-7` is the machine raising -- `u64_of_int` refuses a negative -- and the tier has to
        // raise with it rather than answer.
        for n in [0i64, 1, 12_345, 1 << 40, -7] {
            let want = machine.call(name, vec![ply_eval::Value::Int(n)], ply_span::Span::DUMMY);
            let mut ctx = native.context();
            ctx.fuel = 1_000;
            let layouts_ptr: *const crate::heap::Layouts = &native.tables().layouts;
            let word = ctx
                .heap
                .to_word(unsafe { &*layouts_ptr }, &ply_eval::Value::Int(n));
            let answer = unsafe { entry(&mut ctx, [word].as_ptr()) };
            match want {
                Ok(want) => {
                    assert_eq!(ctx.failed, 0, "`{name}({n})` raised in the C tier");
                    let got = crate::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
                    assert_eq!(
                        got, want,
                        "`{name}({n})`: the tier and the machine disagree"
                    );
                }
                Err(d) => assert_ne!(
                    ctx.failed, 0,
                    "`{name}({n})` answered where the machine raised: {}",
                    d.message
                ),
            }
        }
    }
}

/// Which shape loses a counted field. Each is one step of a parser's state handling.
#[test]
fn a_record_with_a_counted_field_survives_being_rebuilt() {
    for (which, body) in [
        (
            "plain",
            "pub fn probe(n: Int) -> P = {pos: 0, depth: n, diags: [n]}",
        ),
        (
            "rebuilt",
            "pub fn probe(n: Int) -> P = with_depth({pos: 0, depth: n, diags: [n]}, 9)",
        ),
        (
            "pushed",
            "pub fn probe(n: Int) -> P = noted({pos: 0, depth: n, diags: [n]}, 7)",
        ),
        (
            "let-bound",
            "pub fn probe(n: Int) -> P = { let p = {pos: 0, depth: n, diags: [n]}; with_depth(p, p.depth + 1) }",
        ),
        (
            "wrapped",
            "pub fn probe(n: Int) -> Option<P> = Some({pos: 0, depth: n, diags: [n]})",
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
        let Some((loaded, native)) = tests_support::unit(&source) else {
            return;
        };
        let mut machine = tests_support::machine(loaded, &source);
        let args = vec![ply_eval::Value::Int(4)];
        let want = machine
            .call("m.probe", args.clone(), ply_span::Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{which}` raised in the machine: {}", d.message));
        let entry: crate::rt::Entry = native.entry("m.probe").expect("compiled");
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts_ptr: *const crate::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts_ptr }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{which}` raised in the C tier");
        let got = crate::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(got, want, "`{which}`: the tiers disagree");
    }
}

/// Two definitions that say exactly the same thing are one content hash, and an emitted body
/// carries its own name -- so a cache keyed on the hash alone serves one body for both and the
/// unit holds two definitions of one symbol and none of the other.
///
/// `crates/ply-compiler/ply/lexer.ply` has that pair (`hex1` and `hex2`) and the C compiler is what
/// noticed. This is the five-line version, and it fails without the name in the key.
#[test]
fn two_definitions_that_say_the_same_thing_get_their_own_bodies() {
    let source = r#"
pub fn one(b: Bytes, i: Int) -> Int = bytes_at(b, i) + 1
pub fn two(b: Bytes, i: Int) -> Int = bytes_at(b, i) + 1
"#;
    let Some((_, native)) = tests_support::unit(source) else {
        return;
    };
    assert!(native.entry("m.one").is_some(), "`one` has no body");
    assert!(native.entry("m.two").is_some(), "`two` has no body");
}

/// A constructor a *program* declares, rather than one the prelude does.
///
/// The unit interns a user constructor under the program-wide name its module qualifies it with,
/// and a body names it bare, so nothing but the resolver stands between the two. Reading the
/// table with the bare symbol found only the prelude's, so every `type` a program declared was
/// refused -- and the fixpoint then refused each of that body's callers in turn, which is most of
/// why this tier took 295 of the front end's 1400 definitions and not why you would guess.
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
    let Some((loaded, native, refused)) = tests_support::with_refusals(source) else {
        return;
    };
    assert!(
        refused.is_empty(),
        "nothing here is outside the fragment: {refused:?}"
    );
    let mut machine = tests_support::machine(loaded, source);
    let cases: &[(&str, Vec<ply_eval::Value>)] = &[
        ("m.round", vec![ply_eval::Value::Int(7)]),
        ("m.eof", vec![]),
        ("m.named", vec![ply_eval::Value::bytes(b"abcd")]),
    ];
    for (name, args) in cases {
        let want = machine
            .call(name, args.clone(), ply_span::Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
        let entry: crate::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts: *const crate::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = crate::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(got, want, "`{name}{args:?}`: the tiers disagree");
    }
}

/// A list read after a fold over it, and a closure called through a value.
///
/// Two defects met here, and both were silent in every workload smaller than the self-hosted
/// front end.
///
/// `rt_dec` is `Heap::release_last`: it frees *unconditionally*, because it is the `rc == 1` case
/// its caller has already established. The in-process tier establishes it — it emits the count
/// test and calls the helper only on the branch where the count is one. This tier called it bare
/// at all five of its release sites, so a release freed the object whatever else was holding it.
/// The `debug_assert` that says so is compiled out of the profile the suite runs.
///
/// And `fused_fold` released a count it had never taken, so a caller that read the list again was
/// reading freed memory even once the release itself was guarded. `fold(xs, 0, add) + len(xs)` is
/// the whole reproduction; it had been wrong since this tier's first commit.
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
    let Some((loaded, native, refused)) = tests_support::with_refusals(source) else {
        return;
    };
    assert!(
        refused.is_empty(),
        "the callback family is inside the fragment now: {refused:?}"
    );
    let list =
        |xs: &[i64]| ply_eval::Value::list(xs.iter().map(|n| ply_eval::Value::Int(*n)).collect());
    let mut machine = tests_support::machine(loaded, source);
    let cases: &[(&str, Vec<ply_eval::Value>)] = &[
        ("m.twice", vec![list(&[1, 2, 3])]),
        ("m.and_len", vec![list(&[1, 2, 3])]),
        (
            "m.through_a_value",
            vec![list(&[1, 2, 3]), ply_eval::Value::Int(10)],
        ),
        ("m.mapped", vec![list(&[1, 2, 3]), ply_eval::Value::Int(3)]),
        ("m.by_name", vec![list(&[1, 2, 3, 4])]),
        (
            "m.used_twice",
            vec![ply_eval::Value::Int(5), ply_eval::Value::Int(2)],
        ),
    ];
    for (name, args) in cases {
        let want = machine
            .call(name, args.clone(), ply_span::Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
        let entry: crate::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts: *const crate::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = crate::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(got, want, "`{name}{args:?}`: the tiers disagree");
    }
}

/// A run that compiles fewer definitions must not poison the next one that compiles more.
///
/// A refusal is cached, because refusing costs the inliner in full and a definition this tier
/// will not take pays that on every run. It is keyed on the digest of what was *offered*, because
/// a body is refused when something it calls was not — so the two have to be the same set. They
/// were not: the instruments that narrow the offered set filtered it after the digest was taken,
/// so a narrowed run's refusals were served back to an unfiltered one. The unit that came out had
/// bodies compiled against a program that never existed, and it segfaulted rather than answering
/// wrongly, which is the only lucky thing about it.
#[test]
fn a_narrower_run_does_not_poison_a_wider_one() {
    let source = r#"
fn twice(n: Int) -> Int = n * 2
// Recursive, so the inliner leaves it a call: withholding it has to refuse `both`, and an
// inlined callee would be part of `both`'s body and refuse nothing.
fn thrice(n: Int) -> Int = if n <= 0 { 0 } else { 3 + thrice(n - 1) }
pub fn both(n: Int) -> Int = twice(n) + thrice(n)
pub fn alone(n: Int) -> Int = twice(n)
"#;
    let dir = std::env::temp_dir().join(format!("ply-c-poison-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The whole test holds `CONFIG` for writing: this run needs a cache of its own, and the
    // variable that gives it one is read by every build in the process.
    let _config = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let restore = std::env::var("PLY_C_CACHE").ok();
    unsafe { std::env::set_var("PLY_C_CACHE", &dir) };

    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let all: Vec<String> = loaded.functions();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();

    let answer = |native: &Native, name: &str, n: i64| -> Option<i64> {
        let entry: crate::rt::Entry = native.entry(name)?;
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let words = [crate::heap::imm(n)];
        let w = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        Some(crate::heap::imm_value(w))
    };

    let (wide, _) =
        crate::c::producer::reference_only(|| crate::c::build(loaded, &all)).expect("builds");
    assert_eq!(answer(&wide, "m.both", 5), Some(25));
    drop(wide);

    // The same offered set, narrowed by the instrument rather than by the caller. This is the
    // path the digest has to follow: `names` is unchanged, so a digest taken before the filter is
    // the same digest, and the refusals below land under the wider run's key.
    unsafe { std::env::set_var("PLY_C_SKIP", "m.thrice") };
    let (narrowed, refused) =
        crate::c::producer::reference_only(|| crate::c::build(loaded, &all)).expect("builds");
    assert!(
        refused.iter().any(|r| r.function == "m.both"),
        "`m.both` calls a definition this build was not offered: {refused:?}"
    );
    assert_eq!(answer(&narrowed, "m.alone", 5), Some(10));
    drop(narrowed);

    unsafe { std::env::remove_var("PLY_C_SKIP") };
    // The one that used to come back wrong.
    let (again, refused) =
        crate::c::producer::reference_only(|| crate::c::build(loaded, &all)).expect("builds");
    assert!(
        refused.is_empty(),
        "the wider build was served the narrower one's refusals: {refused:?}"
    );
    assert_eq!(answer(&again, "m.both", 5), Some(25));
    let _ = std::fs::remove_dir_all(&dir);
    // Put the shared cache back before the write lock goes, so the builds waiting on it read the
    // directory the rest of this binary uses.
    unsafe {
        match &restore {
            Some(had) => std::env::set_var("PLY_C_CACHE", had),
            None => std::env::remove_var("PLY_C_CACHE"),
        }
    }
}

/// A unit built once is put back together, not built again.
///
/// Every worker used to rebuild it: reading each cached body, substituting its placeholders and
/// assembling the whole translation unit, only to hand it to an object cache that already had the
/// answer. Sharing the built unit in process is not open to this tier -- `ply_eval::Value` holds
/// `Rc`, so nothing containing one crosses a rayon worker -- so it goes through the file system,
/// and this is the property that has to hold when it does: the second build answers what the
/// first one did.
///
/// The shapes are the part to distrust. Their ids are baked into the emitted C as numbers, so a
/// unit read back has to intern them in the order it recorded them or the C reads the wrong field
/// of the wrong record. `finish` refuses a unit whose ids come back different rather than guessing.
///
/// The nonce is what makes the first build a miss: the unit key is a function of every offered
/// definition's hash, so a body no previous run has seen has no entry waiting for it. Without it
/// this test would pass on its second-ever run without exercising the write path at all.
#[test]
fn a_unit_read_back_from_the_cache_answers_what_it_answered_when_built() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let source = format!(
        r#"
type Shape = {{ wide: Int, tall: Int }}
type Tag = TA | TB(Int)
fn area(s: Shape) -> Int = s.wide * s.tall
fn label(t: Tag) -> Int = match t {{ TA -> 0, TB(n) -> n }}
pub fn nonce() -> Int = {}
pub fn both(w: Int, h: Int) -> Int = area({{ wide: w, tall: h }}) + label(TB(w)) + nonce() - nonce()
pub fn tagged(n: Int) -> Int = label(if n > 0 {{ TB(n) }} else {{ TA }})
"#,
        nonce % 1_000_000
    );

    let Some(loaded) = tests_support::keyed(&source) else {
        return;
    };
    let names: Vec<String> = loaded.functions();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let ask = |native: &Native, name: &str, args: &[i64]| -> i64 {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was refused"));
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let words: Vec<crate::heap::Word> = args.iter().map(|a| crate::heap::imm(*a)).collect();
        let w = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        crate::heap::imm_value(w)
    };

    // Writing: `UNITS_REUSED` below counts every build in the process, not just these two.
    let _config = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let (built, _) = crate::c::producer::reference_only(|| crate::c::build(loaded, &names))
        .expect("the first build");
    let first = (
        ask(&built, "m.both", &[3, 4]),
        ask(&built, "m.tagged", &[7]),
        ask(&built, "m.tagged", &[0]),
    );
    assert_eq!(
        first,
        (15, 7, 0),
        "the built unit is wrong before the cache is even involved"
    );
    drop(built);

    let reused = super::cache::UNITS_REUSED.load(std::sync::atomic::Ordering::Relaxed);
    let (again, _) = crate::c::producer::reference_only(|| crate::c::build(loaded, &names))
        .expect("the second build");
    assert_eq!(
        super::cache::UNITS_REUSED.load(std::sync::atomic::Ordering::Relaxed),
        reused + 1,
        "the second build emitted a unit instead of reading back the one the first build wrote"
    );
    assert_eq!(
        (
            ask(&again, "m.both", &[3, 4]),
            ask(&again, "m.tagged", &[7]),
            ask(&again, "m.tagged", &[0])
        ),
        first,
        "the unit read back from the cache does not answer what the one that wrote it did"
    );
}

/// A pure nullary root that answers a handle is asked once, not once per call.
///
/// The in-process tier has always done this: `rt_constant` runs the root, remembers the word, and
/// answers it from then on. This tier declared the helper, bound it, and never emitted it -- so a
/// two-hundred-and-fifty-six-element list built inside a twenty-thousand-iteration fold cost 56ms
/// here against 0.1ms in process. Not worse code; the same code, run twenty thousand more times.
///
/// The memo is the observable, and it is the right one to assert on rather than a clock: without
/// the emitted `rt_constant` the compiled body calls the root directly and the slot stays empty
/// however long the run takes.
///
/// The slot is also a row of the unit's code table, which is what `rt_constant` calls through. Two
/// numberings here would be a value remembered by the seam that compiled code cannot see, and the
/// index this tier handed the seam was not a row of that table until the helper was emitted.
#[test]
fn a_pure_nullary_root_that_answers_a_handle_is_asked_once() {
    let source = r#"
pub fn table() -> List<Int> = map(range(0, 32), |i: Int| i * 7 + 1)
pub fn probe(n: Int) -> Int = fold(range(0, n), 0, |acc: Int, _x: Int| acc + len(table()))
"#;
    let Some((loaded, native)) = tests_support::unit(source) else {
        return;
    };
    let slot = native
        .constant_index("m.table")
        .expect("a pure nullary root is given a memo slot");
    assert!(
        slot < native.tables().functions.len(),
        "the memo slot is not a row of the code table `rt_constant` calls through"
    );
    assert!(
        native.tables().memoized(slot).is_none(),
        "something was remembered before the root ever ran"
    );

    let entry: crate::rt::Entry = native.entry("m.probe").expect("`probe` was refused");
    let mut ctx = native.context();
    ctx.fuel = 100_000;
    let args = [crate::heap::imm(64)];
    let answer = unsafe { entry(&mut ctx, args.as_ptr()) };
    assert_eq!(ctx.failed, 0, "`probe` raised");
    assert_eq!(crate::heap::imm_value(answer), 32 * 64);
    assert!(
        native.tables().memoized(slot).is_some(),
        "`probe` called the root directly instead of asking the runtime for its answer"
    );
    let _ = loaded;
}

/// Reading the accumulator more than once costs no allocations.
///
/// The rule that lets a record go is guarded on the base being read exactly once, because the
/// emitter does not visit reads in the order the lowering marked them and two attempts to relax
/// that guard by counting freed a record something later read. Every accumulator this tier
/// compiles reads its record more than once -- `{..s, count: s.count + 1}` is two -- so the guard
/// meant the record was never let go at all, and a fold allocated one per iteration and kept it.
///
/// The exception is position rather than counting: at the body's tail nothing is emitted after the
/// update, so the later read the guard protects cannot be there to protect. This is what says so.
/// `wide` and `narrow` differ only in how many times they read `s`, and on the old rule `wide`
/// allocated one five-word record per iteration that `narrow` did not.
#[test]
fn an_accumulator_read_more_than_once_is_still_let_go() {
    let source = r#"
type S = { i: Int, tag: Bytes }
fn narrow(s: S, x: Int) -> S = {..s, i: x}
fn wide(s: S, x: Int) -> S = {..s, i: s.i + x}
pub fn with_narrow(n: Int) -> Int = fold(range(0, n), {i: 0, tag: b"z"}, narrow).i
pub fn with_wide(n: Int) -> Int = fold(range(0, n), {i: 0, tag: b"z"}, wide).i
"#;
    let Some((loaded, native)) = tests_support::unit(source) else {
        return;
    };
    let rounds = 500i64;
    let allocations = |name: &str| -> usize {
        let entry: crate::rt::Entry = native.entry(name).expect("compiled");
        let mut ctx = native.context();
        ctx.fuel = 1_000_000;
        let before = ctx.heap.allocated();
        let args = [crate::heap::imm(rounds)];
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

/// A temporary a body makes and hands on is counted once, not twice.
///
/// Every runtime helper that answers a word answers an **owned** one: `rt_ctor` allocates,
/// `rt_field` increments what it reads out, `rt_concat` builds a new string, and each releases the
/// word arguments it was given -- which is what the duplicate on the way *in* is for. The emitter
/// took a second count at every use of such an answer, so `f(g(x))` counted `g`'s answer once for
/// `f` and once for nobody, and the second one was never released.
///
/// Asserted on the emitted text because that is where the property lives and nothing downstream
/// can see it: a doubled count is not a wrong answer, it is an object that cannot die. `wrap`
/// makes one word with a helper and hands it straight to another, so a correct emit takes no count
/// at all -- and the version this replaces took exactly one.
#[test]
fn a_helper_answer_handed_to_a_helper_is_not_counted_again() {
    let source = r#"
pub fn wrap(n: Int) -> List<Bytes> = [byte_of_int(n)]
"#;
    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let ctors = loaded.ctors();
    let digest = super::cache::ctors_digest(&ctors);
    let mut unit = super::emit::Unit::new(ctors, vec!["m.wrap".to_string()]);
    let inlining = crate::opt::Inlining::EMITTED;
    let (text, _) = super::build::emit_one(
        loaded,
        &mut unit,
        "m.wrap",
        &digest,
        (inlining.budget, inlining.depth),
        "",
    )
    .expect("`wrap` emits");
    assert!(
        text.contains("rt_byte_of_int_p") && text.contains("rt_list_p"),
        "the body no longer has the shape this test is about:\n{text}"
    );
    assert_eq!(
        text.matches("ply_inc(").count(),
        0,
        "a count was taken on a word the helper had already counted:\n{text}"
    );
}
