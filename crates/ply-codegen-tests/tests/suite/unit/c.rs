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
    use ply_span::SourceId;
    use std::collections::HashMap;

    pub fn unit(text: &str) -> Option<(&'static Source, Native)> {
        with_refusals(text).map(|(s, n, _)| (s, n))
    }

    fn front(text: &str) -> &'static ply_ty::Front {
        Box::leak(Box::new(
            ply_codegen::c::producer::checked_front(
                &[("m".to_string(), text.to_string())],
                &[SourceId(0)],
            )
            .expect("checks"),
        ))
    }

    /// Keyed on the text, not the name: two tests defining `m.f` would otherwise share an emit-cache entry.
    pub fn keyed(text: &str) -> Option<&'static Source> {
        let front = front(text);
        let stamp = blake3::hash(text.as_bytes()).to_hex();
        let keys = Source::from_front(front, HashMap::new())
            .functions()
            .into_iter()
            .map(|n| (n.clone(), format!("h-{n}-{}", &stamp[..16])))
            .collect();
        Some(Box::leak(Box::new(
            Source::from_front(front, keys).with_texts(texts(text)),
        )))
    }

    pub fn with_refusals(
        text: &str,
    ) -> Option<(&'static Source, Native, Vec<ply_codegen::c::Refused>)> {
        let source: &'static Source = Box::leak(Box::new(
            Source::from_front(front(text), HashMap::new()).with_texts(texts(text)),
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

    fn texts(text: &str) -> HashMap<String, String> {
        HashMap::from([("m".to_string(), text.to_string())])
    }
}

#[test]
fn the_tier_answers_what_the_program_means() {
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
    let Some((_, native)) = tests_support::unit(source) else {
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
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(got, want, "`{name}{args:?}`");
    }
}

/// A `U64` past `2^62` is not an immediate (tagging eats its top bit), so it is held as the machine's own value.
#[test]
fn a_width_the_tier_cannot_carry_in_a_register_still_answers() {
    let source = r#"
pub fn wide(n: Int) -> Int = {
  let a = u64_of_int(n);
  let b = wrap_mul(wrap_add(a, a), 0x9E37_79B9_7F4A_7C15u64);
  int_of_u64(rotr(b, 7) & 0xFFFFu64)
}
pub fn narrow(n: Int) -> Int = int_of_u32(rotr(wrap_mul(u32_of_int(n), 2654435761u32), 7))
"#;
    let Some((_, native, _)) = tests_support::with_refusals(source) else {
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
                    assert_eq!(ctx.failed, 0, "`{name}({n})` raised in the C tier");
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
    for (which, body, want) in [
        (
            "plain",
            "pub fn probe(n: Int) -> P = {pos: 0, depth: n, diags: [n]}",
            "{depth: 4, diags: [4], pos: 0}",
        ),
        (
            "rebuilt",
            "pub fn probe(n: Int) -> P = with_depth({pos: 0, depth: n, diags: [n]}, 9)",
            "{depth: 9, diags: [4], pos: 0}",
        ),
        (
            "pushed",
            "pub fn probe(n: Int) -> P = noted({pos: 0, depth: n, diags: [n]}, 7)",
            "{depth: 4, diags: [4, 7], pos: 0}",
        ),
        (
            "let-bound",
            "pub fn probe(n: Int) -> P = { let p = {pos: 0, depth: n, diags: [n]}; with_depth(p, p.depth + 1) }",
            "{depth: 5, diags: [4], pos: 0}",
        ),
        (
            "wrapped",
            "pub fn probe(n: Int) -> Option<P> = Some({pos: 0, depth: n, diags: [n]})",
            "Some({depth: 4, diags: [4], pos: 0})",
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
        let Some((_, native)) = tests_support::unit(&source) else {
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
        assert_eq!(ctx.failed, 0, "`{which}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts_ptr }, answer);
        assert_eq!(got.render(), want, "`{which}`");
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
    let Some((_, native, refused)) = tests_support::with_refusals(source) else {
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
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
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
    let Some((_, native, refused)) = tests_support::with_refusals(source) else {
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
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        assert_eq!(&got, want, "`{name}{args:?}`");
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

/// Nanoseconds now: a definition whose text carries it is in no earlier process's cache.
fn nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        % 1_000_000_000_000_000
}

/// Module `m` keyed as a command keys it, on the front end's hashes; `suffix` turns every key, so a
/// source keyed apart from the others is built cold.
fn keyed_by_hash(text: &str, suffix: &str) -> &'static ply_codegen::Source {
    let owned: &'static str = Box::leak(text.to_string().into_boxed_str());
    let id = ply_span::SourceId(0);
    let front =
        ply_codegen::c::producer::checked_front(&[("m".to_string(), owned.to_string())], &[id])
            .expect("checks");
    let front: &'static ply_ty::Front = Box::leak(Box::new(front));
    let keys = ply_codegen::emit_keys(front)
        .into_iter()
        .map(|(name, key)| (name, format!("{key}{suffix}")))
        .collect();
    Box::leak(Box::new(
        ply_codegen::Source::from_front(front, keys).with_texts(std::collections::HashMap::from([
            ("m".to_string(), owned.to_string()),
        ])),
    ))
}

/// The unit over every root of `source`, body by body: `produce` never reads a whole unit back.
fn produced(source: &'static ply_codegen::Source) -> ply_codegen::c::Produced {
    let names = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let produced = ply_codegen::c::produce(source, &refs).expect("the program emits");
    assert!(produced.refused.is_empty(), "{:?}", produced.refused);
    produced
}

/// `spare`'s nonce makes the second build miss the unit cache and ask body by body.
#[test]
fn a_definition_that_only_moved_is_served_from_the_cache_and_placed_where_it_now_is() {
    let nonce = nonce();
    let main = "fn main() -> Int = 1 / 0\n";
    let moved = format!("fn spare() -> Int = {nonce}\n\n\n{main}");
    let hashed = |text: &str| keyed_by_hash(text, "");
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

/// The nonce keeps both definitions out of any earlier process's cache.
#[test]
fn an_edited_definition_alone_is_asked_of_the_emitter_and_the_unit_is_the_one_a_cold_build_emits() {
    let nonce = nonce();
    let program = |k: u128| {
        let steady = format!("pub fn steady(x: Int) -> Int = x + {nonce}\n");
        format!("{steady}pub fn changed(x: Int) -> Int = x * {k}\n")
    };

    let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    let first = produced(keyed_by_hash(&program(nonce + 1), ""));
    let edited_text = program(nonce + 2);
    ply_codegen::c::producer::reset_census();
    let edited = produced(keyed_by_hash(&edited_text, ""));
    assert_eq!(
        ply_codegen::c::producer::census().wanted,
        vec![vec!["m.changed".to_string()]],
        "the emitter was not entered once, for the edited definition alone"
    );
    assert_ne!(first.text, edited.text, "the edit did not reach the unit");
    let cold = produced(keyed_by_hash(&edited_text, "-cold"));
    assert_eq!(
        edited.text, cold.text,
        "a unit with one body served from the cache is not the one a cold build emits"
    );
}

/// `caller`'s C reads only `leaf`'s signature, which a body edit keeps, so its key holds and its
/// body is served from the cache; the nonce in `caller` keeps it out of an earlier process's.
#[test]
fn editing_a_body_asks_the_emitter_for_that_definition_and_not_its_callers() {
    let nonce = nonce();
    let program = |k: u128| {
        format!(
            "pub fn leaf(x: Int) -> Int = x + {k}\n\
             pub fn caller(x: Int) -> Int = leaf(x) * 2 + {nonce}\n"
        )
    };

    let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    let before = keyed_by_hash(&program(nonce + 1), "");
    let after = keyed_by_hash(&program(nonce + 2), "");
    assert_ne!(
        before.keys.get("m.leaf"),
        after.keys.get("m.leaf"),
        "editing `leaf`'s body kept its key"
    );
    assert_eq!(
        before.keys.get("m.caller"),
        after.keys.get("m.caller"),
        "editing `leaf`'s body turned `caller`'s key"
    );
    let first = produced(before);
    ply_codegen::c::producer::reset_census();
    let edited = produced(after);
    assert_eq!(
        ply_codegen::c::producer::census().wanted,
        vec![vec!["m.leaf".to_string()]],
        "the emitter was not entered once, for `leaf` alone"
    );
    assert_ne!(first.text, edited.text, "the edit did not reach the unit");
    let cold = produced(keyed_by_hash(&program(nonce + 2), "-cold"));
    assert_eq!(
        edited.text, cold.text,
        "a unit with `caller` served from the cache is not the one a cold build emits"
    );
}

/// `caller` opens `leaf`'s answer by the kind its signature declares, so a new answer type turns
/// `caller`'s key too; `len` takes a list of either, so `caller` reads the same both times.
#[test]
fn changing_a_signature_asks_the_emitter_for_the_definition_and_its_callers() {
    let nonce = nonce();
    let program =
        |leaf: &str| format!("{leaf}\npub fn caller(x: Int) -> Int = len([leaf(x)]) + {nonce}\n");
    let as_int = program(&format!("pub fn leaf(x: Int) -> Int = x + {nonce}"));
    let as_bool = program(&format!("pub fn leaf(x: Int) -> Bool = x > {nonce}"));

    let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    let before = keyed_by_hash(&as_int, "");
    let after = keyed_by_hash(&as_bool, "");
    assert_ne!(
        before.keys.get("m.caller"),
        after.keys.get("m.caller"),
        "retyping `leaf` kept `caller`'s key"
    );
    let _ = produced(before);
    ply_codegen::c::producer::reset_census();
    let retyped = produced(after);
    let wanted: Vec<Vec<String>> = ply_codegen::c::producer::census()
        .wanted
        .into_iter()
        .map(|mut entry| {
            entry.sort();
            entry
        })
        .collect();
    assert_eq!(
        wanted,
        vec![vec!["m.caller".to_string(), "m.leaf".to_string()]],
        "the emitter was not entered once, for `leaf` and its caller"
    );
    let cold = produced(keyed_by_hash(&as_bool, "-cold"));
    assert_eq!(
        retyped.text, cold.text,
        "the unit is not the one a cold build emits"
    );
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

    pub fn produce(
        source: &'static ply_codegen::Source,
        names: &[String],
    ) -> ply_codegen::c::Produced {
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let _config = super::CONFIG.read().unwrap_or_else(|e| e.into_inner());
        let produced = ply_codegen::c::produce(source, &refs).expect("the program emits");
        assert!(produced.refused.is_empty(), "{:?}", produced.refused);
        produced
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
    let (Some(small), Some(large)) = (tests_support::keyed(base), tests_support::keyed(&grown))
    else {
        return;
    };
    let small = numbering_support::produce(small, &small.functions());
    let large = numbering_support::produce(large, &large.functions());
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
    let _ = numbering_support::body(&large.text, "ply_m_zz");
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
    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let produced = numbering_support::produce(loaded, &loaded.functions());
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
    for symbol in ["ply_m_one", "ply_m_two"] {
        let body = numbering_support::body(&produced.text, symbol);
        assert!(
            body.contains(&format!("rt_lit_p(ctx, {at})")),
            "`{symbol}` does not read the shared constant from its one entry:\n{body}"
        );
    }
}

#[test]
fn a_unit_is_the_same_bytes_however_its_definitions_are_offered() {
    let source = r#"
type Pair = { left: Int, right: Int }
fn key(x: Int) -> Int = x + 1
pub fn sum(xs: List<Int>) -> Int = fold(map(xs, key), 0, |a: Int, x: Int| a + x)
pub fn pick(p: Pair) -> Bytes = if p.left > p.right { b"left" } else { b"right" }
pub fn steady() -> Int = 7
"#;
    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let forward = loaded.functions();
    let mut backward = forward.clone();
    backward.reverse();
    assert!(forward.len() > 1 && forward != backward);
    let first = numbering_support::produce(loaded, &forward);
    let second = numbering_support::produce(loaded, &backward);
    assert_eq!(
        first.text, second.text,
        "the order the definitions were offered in reached the unit"
    );
}

/// The members of a recursive group share one C function, so a tail call between them is a jump:
/// the fuel is far below the calls made, and a call that nested would spend it first.
#[test]
fn a_tail_call_between_members_of_a_recursive_group_is_a_jump() {
    let source = r#"
fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }
pub fn parity(n: Int) -> Bool = even(n)
"#;
    let Some((loaded, native)) = tests_support::unit(source) else {
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

/// A `handle` lands failures on its own label, so a cycle holding one is emitted definition by definition.
#[test]
fn a_recursive_group_holding_a_handle_is_emitted_per_definition() {
    let source = r#"
fn a(n: Int) -> Int = if n == 0 { 0 } else if n == 1 { handle { b(0) } with { clock.now() -> 7, } } else { b(n - 1) }
fn b(n: Int) -> Int = if n == 0 { clock.now() } else { a(n - 1) }
"#;
    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let names = loaded.functions();
    let produced = numbering_support::produce(loaded, &names);
    let code = numbering_support::code(&produced.text);
    assert!(
        !code.contains("ply__group_"),
        "a member holding a `handle` was grouped:\n{code}"
    );
    for symbol in ["ply_m_a", "ply_m_b"] {
        let _ = numbering_support::body(code, symbol);
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = {
        let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
        ply_codegen::c::build(loaded, &refs).expect("builds")
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

/// A group's body serves every member and is placed once; the second build reads it from the
/// cache under each member's key.
#[test]
fn a_unit_holding_a_group_is_the_same_bytes_however_its_definitions_are_offered() {
    let source = r#"
fn ping(n: Int, acc: Int) -> Int = if n == 0 { acc } else { pong(n - 1, acc + 1) }
fn pong(n: Int, acc: Int) -> Int = if n == 0 { acc } else { ping(n - 1, acc + 2) }
pub fn volley(n: Int) -> Int = ping(n, 0)
pub fn steady() -> Int = 7
"#;
    let Some(loaded) = tests_support::keyed(source) else {
        return;
    };
    let forward = loaded.functions();
    let mut backward = forward.clone();
    backward.reverse();
    let first = numbering_support::produce(loaded, &forward);
    let second = numbering_support::produce(loaded, &backward);
    assert_eq!(
        first.text, second.text,
        "the order the definitions were offered in reached the unit"
    );
    let code = numbering_support::code(&first.text);
    assert_eq!(
        code.matches("static Word ply__group_").count(),
        1,
        "the group's body is placed once:\n{code}"
    );
    assert_eq!(
        code.matches("goto ply_loop;").count(),
        2,
        "each member's tail call into the group jumps:\n{code}"
    );
    for symbol in ["ply_m_ping", "ply_m_pong", "ply_m_volley"] {
        let _ = numbering_support::body(code, symbol);
    }
}
