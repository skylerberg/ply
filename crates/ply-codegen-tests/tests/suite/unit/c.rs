mod cache;
mod sweep;
mod toolchain;
mod upgrade;

use ply_codegen::c::{HELPERS, Native, PRELUDE, compile_and_load, helper_addresses, runtime_decls};

/// A declaration with no address is a null call at run time: a crash rather than a decline.
#[test]
fn every_declared_helper_has_an_address() {
    let addrs = helper_addresses();
    assert_eq!(addrs.len(), HELPERS.len());
    for (h, a) in HELPERS.iter().zip(&addrs) {
        assert!(!a.is_null(), "`{}` has no address", h.name);
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
    assert!(PRELUDE.contains("#define PLY_HEADER 16"));
}

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
    let lib = match compile_and_load(&src, "probe") {
        Ok(l) => l,
        // A machine with no C compiler is not one this tier is for.
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
    let probe: ply_codegen::rt::Entry = unsafe { std::mem::transmute(probe) };
    let args = [ply_codegen::heap::imm(20), ply_codegen::heap::imm(22)];
    let answer = unsafe { probe(std::ptr::null_mut(), args.as_ptr()) };
    assert_eq!(ply_codegen::heap::imm_value(answer), 42);
}

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

/// `PLY_C_CACHE`, `PLY_C_SKIP` and `cache::UNITS_REUSED` are process-wide: a test that changes or counts them takes this for writing, every other build for reading.
static CONFIG: std::sync::RwLock<()> = std::sync::RwLock::new(());

pub mod tests_support {
    use ply_codegen::c::Native;
    use ply_codegen::source::Source;
    use ply_syntax::ast::ModuleName;
    use std::collections::HashMap;

    pub fn unit(text: &str) -> Option<(&'static Source, Native)> {
        with_refusals(text).map(|(s, n, _)| (s, n))
    }

    /// Keyed on the text, not the name: two tests defining `m.f` would otherwise share an emit-cache entry.
    pub fn keyed(text: &str) -> Option<&'static Source> {
        let mut sources = ply_span::SourceMap::new();
        let owned: &'static str = Box::leak(text.to_string().into_boxed_str());
        let id = sources.add("m.ply", owned.to_string());
        let mut ast =
            ply_syntax::parse_program([(id, ModuleName::from_dotted("m"), owned)]).expect("parses");
        let resolved = ply_syntax::resolve::resolve(&mut ast).expect("resolves");
        let check =
            ply_codegen::c::producer::checked_front(&[("m".to_string(), owned.to_string())], &[id])
                .expect("checks")
                .check;
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
        Some(Box::leak(Box::new(
            Source::keyed(program, resolved, check, keys).with_texts(texts(text)),
        )))
    }

    pub fn with_refusals(
        text: &str,
    ) -> Option<(&'static Source, Native, Vec<ply_codegen::c::Refused>)> {
        let mut sources = ply_span::SourceMap::new();
        let owned: &'static str = Box::leak(text.to_string().into_boxed_str());
        let id = sources.add("m.ply", owned.to_string());
        let mut ast =
            ply_syntax::parse_program([(id, ModuleName::from_dotted("m"), owned)]).expect("parses");
        let resolved = ply_syntax::resolve::resolve(&mut ast).expect("resolves");
        let check =
            ply_codegen::c::producer::checked_front(&[("m".to_string(), owned.to_string())], &[id])
                .expect("checks")
                .check;
        let source: &'static Source = Box::leak(Box::new(
            Source::new(
                Box::leak(Box::new(ast)),
                Box::leak(Box::new(resolved)),
                Box::leak(Box::new(check)),
            )
            .with_texts(texts(text)),
        ));
        let names = source.functions();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let _config = super::CONFIG.read().unwrap_or_else(|e| e.into_inner());
        match ply_codegen::c::build(source, &refs) {
            Ok((native, refused)) => Some((source, native, refused)),
            Err(e) if e.to_string().contains("could not run") => None,
            Err(e) => panic!("{e}"),
        }
    }

    pub fn interpreted(
        source: &'static Source,
        name: &str,
        args: &[ply_eval::Value],
    ) -> Result<ply_eval::Value, ply_span::Diagnostic> {
        ply_eval::interp::Pure::new(source.program, source.resolved).call(
            name,
            args.to_vec(),
            ply_span::Span::DUMMY,
            100_000,
        )
    }

    fn texts(text: &str) -> HashMap<String, String> {
        HashMap::from([("m".to_string(), text.to_string())])
    }
}

#[test]
fn the_tier_agrees_with_the_interpreter() {
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
    let cases: &[(&str, Vec<ply_eval::Value>)] = &[
        ("m.mixed", vec![ply_eval::Value::Int(0xDEAD_BEEF)]),
        ("m.mixed", vec![ply_eval::Value::Int(0)]),
        ("m.counted", vec![ply_eval::Value::Int(40)]),
        // Built at one site but described once per iteration: reusing the first iteration's object is a wrong answer, not a crash.
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
        let want = tests_support::interpreted(loaded, name, args)
            .unwrap_or_else(|d| panic!("`{name}` raised in the interpreter: {}", d.message));
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
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the interpreter disagree"
        );
    }
}

/// A `U64` past `2^62` is not an immediate (tagging eats its top bit), so it is held as the machine's own value.
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
    for name in ["m.wide", "m.narrow"] {
        let entry: ply_codegen::rt::Entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        // `-7` makes the machine raise (`u64_of_int` refuses a negative), and the tier has to raise with it.
        for n in [0i64, 1, 12_345, 1 << 40, -7] {
            let want = tests_support::interpreted(loaded, name, &[ply_eval::Value::Int(n)]);
            let mut ctx = native.context();
            ctx.fuel = 1_000;
            let layouts_ptr: *const ply_codegen::heap::Layouts = &native.tables().layouts;
            let word = ctx
                .heap
                .to_word(unsafe { &*layouts_ptr }, &ply_eval::Value::Int(n));
            let answer = unsafe { entry(&mut ctx, [word].as_ptr()) };
            match want {
                Ok(want) => {
                    assert_eq!(ctx.failed, 0, "`{name}({n})` raised in the C tier");
                    let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
                    assert_eq!(
                        got, want,
                        "`{name}({n})`: the tier and the interpreter disagree"
                    );
                }
                Err(d) => assert_ne!(
                    ctx.failed, 0,
                    "`{name}({n})` answered where the interpreter raised: {}",
                    d.message
                ),
            }
        }
    }
}

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
        let args = vec![ply_eval::Value::Int(4)];
        let want = tests_support::interpreted(loaded, "m.probe", &args)
            .unwrap_or_else(|d| panic!("`{which}` raised in the interpreter: {}", d.message));
        let entry: ply_codegen::rt::Entry = native.entry("m.probe").expect("compiled");
        let mut ctx = native.context();
        ctx.fuel = 100_000;
        let layouts_ptr: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts_ptr }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{which}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(
            got, want,
            "`{which}`: the tier and the interpreter disagree"
        );
    }
}

/// An emitted body carries its own name, so a cache keyed on the content hash alone would serve one body for both.
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
    let Some((loaded, native, refused)) = tests_support::with_refusals(source) else {
        return;
    };
    assert!(
        refused.is_empty(),
        "nothing here is outside the fragment: {refused:?}"
    );
    let cases: &[(&str, Vec<ply_eval::Value>)] = &[
        ("m.round", vec![ply_eval::Value::Int(7)]),
        ("m.eof", vec![]),
        ("m.named", vec![ply_eval::Value::bytes(b"abcd")]),
    ];
    for (name, args) in cases {
        let want = tests_support::interpreted(loaded, name, args)
            .unwrap_or_else(|d| panic!("`{name}` raised in the interpreter: {}", d.message));
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
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the interpreter disagree"
        );
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
    let Some((loaded, native, refused)) = tests_support::with_refusals(source) else {
        return;
    };
    assert!(
        refused.is_empty(),
        "the callback family is inside the fragment now: {refused:?}"
    );
    let list =
        |xs: &[i64]| ply_eval::Value::list(xs.iter().map(|n| ply_eval::Value::Int(*n)).collect());
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
        let want = tests_support::interpreted(loaded, name, args)
            .unwrap_or_else(|d| panic!("`{name}` raised in the interpreter: {}", d.message));
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
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the interpreter disagree"
        );
    }
}

/// Refusals are cached by the digest of the offered set, so the digest must be taken after any filter narrows it.
#[test]
fn a_narrower_run_does_not_poison_a_wider_one() {
    let source = r#"
fn twice(n: Int) -> Int = n * 2
fn thrice(n: Int) -> Int = if n <= 0 { 0 } else { 3 + thrice(n - 1) }
pub fn both(n: Int) -> Int = twice(n) + thrice(n)
pub fn alone(n: Int) -> Int = twice(n)
"#;
    let dir = std::env::temp_dir().join(format!("ply-c-poison-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // `CONFIG` for writing: this run needs its own cache, and every build in the process reads that variable.
    let _config = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let restore = std::env::var("PLY_C_CACHE").ok();
    unsafe { std::env::set_var("PLY_C_CACHE", &dir) };

    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let all: Vec<String> = loaded.functions();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();

    let answer = |native: &Native, name: &str, n: i64| -> Option<i64> {
        let entry: ply_codegen::rt::Entry = native.entry(name)?;
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let words = [ply_codegen::heap::imm(n)];
        let w = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        Some(ply_codegen::heap::imm_value(w))
    };

    let (wide, _) = ply_codegen::c::build(loaded, &all).expect("builds");
    assert_eq!(answer(&wide, "m.both", 5), Some(25));
    drop(wide);

    // Narrowed by the instrument: `names` is unchanged, so a digest taken before the filter would be the wider run's key.
    unsafe { std::env::set_var("PLY_C_SKIP", "m.thrice") };
    let (narrowed, refused) = ply_codegen::c::build(loaded, &all).expect("builds");
    assert!(
        refused.iter().any(|r| r.function == "m.both"),
        "`m.both` calls a definition this build was not offered: {refused:?}"
    );
    assert_eq!(answer(&narrowed, "m.alone", 5), Some(10));
    drop(narrowed);

    unsafe { std::env::remove_var("PLY_C_SKIP") };
    let (again, refused) = ply_codegen::c::build(loaded, &all).expect("builds");
    assert!(
        refused.is_empty(),
        "the wider build was served the narrower one's refusals: {refused:?}"
    );
    assert_eq!(answer(&again, "m.both", 5), Some(25));
    let _ = std::fs::remove_dir_all(&dir);
    // Restore the shared cache before the write lock goes, so the builds waiting on it read the usual directory.
    unsafe {
        match &restore {
            Some(had) => std::env::set_var("PLY_C_CACHE", had),
            None => std::env::remove_var("PLY_C_CACHE"),
        }
    }
}

/// Shape ids are baked into the C, so a unit read back must intern them in recorded order; the nonce makes the first build a miss.
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
        let words: Vec<ply_codegen::heap::Word> =
            args.iter().map(|a| ply_codegen::heap::imm(*a)).collect();
        let w = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised");
        ply_codegen::heap::imm_value(w)
    };

    // Writing: `UNITS_REUSED` below counts every build in the process, not just these two.
    let _config = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let (built, _) = ply_codegen::c::build(loaded, &names).expect("the first build");
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

    let reused = ply_codegen::c::cache::UNITS_REUSED.load(std::sync::atomic::Ordering::Relaxed);
    let (again, _) = ply_codegen::c::build(loaded, &names).expect("the second build");
    assert_eq!(
        ply_codegen::c::cache::UNITS_REUSED.load(std::sync::atomic::Ordering::Relaxed),
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

/// Keyed as a command keys it, on the front end's hashes; `spare`'s nonce makes the second build miss the unit cache and ask body by body.
#[test]
fn a_definition_that_only_moved_is_served_from_the_cache_and_placed_where_it_now_is() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        % 1_000_000_000_000_000;
    let main = "fn main() -> Int = 1 / 0\n";
    let moved = format!("fn spare() -> Int = {nonce}\n\n\n{main}");
    let hashed = |text: &str| -> &'static ply_codegen::Source {
        let owned: &'static str = Box::leak(text.to_string().into_boxed_str());
        let id = ply_span::SourceId(0);
        let mut ast =
            ply_syntax::parse_program([(id, ply_syntax::ast::ModuleName::from_dotted("m"), owned)])
                .expect("parses");
        let resolved = ply_syntax::resolve::resolve(&mut ast).expect("resolves");
        let front =
            ply_codegen::c::producer::checked_front(&[("m".to_string(), owned.to_string())], &[id])
                .expect("checks");
        let front: &'static ply_ty::Front = Box::leak(Box::new(front));
        let keys = ply_codegen::emit_keys(front);
        Box::leak(Box::new(
            ply_codegen::Source::from_front(
                Box::leak(Box::new(ast)),
                Box::leak(Box::new(resolved)),
                front,
                keys,
            )
            .with_texts(std::collections::HashMap::from([(
                "m".to_string(),
                owned.to_string(),
            )])),
        ))
    };
    let failure = |source: &'static ply_codegen::Source| -> Option<ply_span::Span> {
        let names = source.functions();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let native = match ply_codegen::c::build(source, &refs) {
            Ok((native, _)) => native,
            Err(e) if e.to_string().contains("could not run") => return None,
            Err(e) => panic!("{e}"),
        };
        let entry = native.entry("m.main").expect("`main` was refused");
        let mut ctx = native.context();
        ctx.fuel = 10_000;
        let _ = unsafe { entry(&mut ctx, std::ptr::null()) };
        assert_ne!(ctx.failed, 0, "`main` divided by zero and answered");
        let d = ctx.take_failure().expect("a failed entry has a diagnostic");
        let label = d
            .labels
            .iter()
            .find(|l| l.primary)
            .expect("a primary label");
        Some(label.span)
    };
    let asked = || ply_codegen::c::producer::with_current(|p| p.counts().0).unwrap_or(0);

    let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    let (first, second) = (hashed(main), hashed(&moved));
    let key = first.keys.get("m.main").expect("`main` is keyed");
    assert_eq!(
        second.keys.get("m.main"),
        Some(key),
        "moving `main` changed its key"
    );
    let Some(before) = failure(first) else {
        return;
    };
    let asked_before = asked();
    let Some(after) = failure(second) else {
        return;
    };
    assert_eq!(
        asked() - asked_before,
        1,
        "`main` was emitted again rather than served from the cache"
    );
    let shift = moved.find(main).expect("`main` is in the moved text") as u32;
    assert!(!before.is_dummy(), "the failure names no place");
    assert_eq!(
        (after.start, after.end),
        (before.start + shift, before.end + shift),
        "the failure is placed where `main` was, not where it is"
    );
    assert_eq!(moved[..after.start as usize].matches('\n').count() + 1, 4);
}

/// Asserted on the memo, not a clock: without the emitted `rt_constant` the slot stays empty however long the run takes.
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

    let entry: ply_codegen::rt::Entry = native.entry("m.probe").expect("`probe` was refused");
    let mut ctx = native.context();
    ctx.fuel = 100_000;
    let args = [ply_codegen::heap::imm(64)];
    let answer = unsafe { entry(&mut ctx, args.as_ptr()) };
    assert_eq!(ctx.failed, 0, "`probe` raised");
    assert_eq!(ply_codegen::heap::imm_value(answer), 32 * 64);
    assert!(
        native.tables().memoized(slot).is_some(),
        "`probe` called the root directly instead of asking the runtime for its answer"
    );
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
    let Some((loaded, native)) = tests_support::unit(source) else {
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
pub fn wrap(n: Int) -> List<Bytes> = [byte_of_int(n)]
"#;
    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let produced = {
        let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
        ply_codegen::c::produce(loaded, &["m.wrap"]).expect("`wrap` emits")
    };
    let body = produced
        .text
        .find("Word ply_m_wrap(PlyCtx *ctx")
        .map(|at| &produced.text[at..])
        .expect("the unit has a body for `wrap`");
    let text = &body[..body.find("\n}\n").map_or(body.len(), |end| end + 3)];
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
