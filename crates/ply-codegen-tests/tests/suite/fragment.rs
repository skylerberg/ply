//! What the code generator compiles, what it refuses, and that what it answers is what the
//! interpreter answers.

use ply_codegen::Cranelift;
use ply_eval::{Provider, Value};
use ply_span::Symbol;
use ply_syntax::ast::{ModuleName, Program};

struct Loaded {
    program: &'static Program,
    resolved: &'static ply_syntax::resolve::Resolved,
    check: &'static ply_core::CheckOutput,
}

/// The shipped standard library plus `source` as a module named `m`.
fn load(source: &str) -> Loaded {
    let mut sources = ply_span::SourceMap::new();
    let mut owned: Vec<(ModuleName, &'static str)> = ply_std::sources()
        .map(|(module, text)| (ModuleName::from_dotted(module), text))
        .collect();
    owned.push((
        ModuleName::from_dotted("m"),
        &*Box::leak(source.to_string().into_boxed_str()),
    ));
    let mut inputs = Vec::new();
    for (module, text) in &owned {
        let id = sources.add(ply_std::pseudo_path(module), (*text).to_string());
        inputs.push((id, module.clone(), *text));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("the corpus parses");
    let expanded = ply_derive::expand_program(&mut ast);
    assert!(expanded.is_empty(), "{expanded:?}");
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the corpus resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("the corpus checks");
    Loaded {
        program: Box::leak(Box::new(ast)),
        resolved: Box::leak(Box::new(resolved)),
        check: Box::leak(Box::new(check)),
    }
}

fn unit(source: &str) -> (&'static Loaded, &'static Cranelift) {
    let loaded: &'static Loaded = Box::leak(Box::new(load(source)));
    let unit = Cranelift::over(loaded.program, loaded.resolved, loaded.check)
        .expect("this host has a cranelift backend");
    (loaded, unit)
}

/// Arithmetic, comparison, `if`, `let`, a `match` on literals, recursion, and a call between two
/// members — the fragment the spike's fragment pins, in one module.
const ARITHMETIC: &str = r#"
fn double(x: Int) -> Int = x * 2

fn even(x: Int) -> Bool = x % 2 == 0

fn clamp(x: Int, lo: Int, hi: Int) -> Int =
  if x < lo { lo } else { if x > hi { hi } else { x } }

fn collatz(n: Int) -> Int =
  if n <= 1 { 0 } else { if even(n) { 1 + collatz(n / 2) } else { 1 + collatz(3 * n + 1) } }

fn sign(n: Int) -> Int = match n { 0 -> 0, _ -> if n < 0 { 0 - 1 } else { 1 } }

fn busy(n: Int) -> Int = {
  let a = double(n);
  let b = clamp(a, 0, 100);
  b + sign(a)
}

fn ladder(n: Int) -> Int = if n <= 0 { 0 } else { 1 + ladder(n - 1) }

fn limit() -> Int = 7

fn odd(n: Int) -> Bool = n % 2 == 1

fn capped(n: Int) -> Int = if odd(n) && n > limit() { limit() } else { n }

fn shaped(x: Int) -> List<Int> = [x, x]
"#;

/// `++` and the pattern shapes the fixpoint gained: a record pattern, one with `..`, one nested
/// inside a constructor, and a constructor nested inside a list.
const SHAPES: &str = r#"
type Step = { value: Int, next: Int }

fn step(n: Int) -> Step = {value: n, next: n + 1}

fn taken(n: Int) -> Int = match step(n) { {value, next} -> value + next }

fn ignored(n: Int) -> Int = match step(n) { {value, ..} -> value }

fn wrapped(n: Int) -> Result<Step, Int> = if n < 0 { Err(0 - n) } else { Ok(step(n)) }

fn nested(n: Int) -> Int = match wrapped(n) { Ok({value, next}) -> value + next, Err(e) -> e }

fn listed(n: Int) -> Int = match [Ok(n), Err(2)] { [Ok(a), Err(b)] -> a + b, _ -> 0 }

fn joined(n: Int) -> Int = if "ab" ++ "cd" == "abcd" { n } else { 0 - n }

fn let_taken(n: Int) -> Int = { let {value, next} = step(n); value + next }

fn let_rest(n: Int) -> Int = { let {value: v, ..} = step(n); v }

fn let_tuple(n: Int) -> Int = { let (a, b) = (n, n + 1); a * b }

fn let_nested(n: Int) -> Int = { let {left: {value, ..}, right: (r, _)} = {left: step(n), right: (n, n)}; value + r }

fn projected(n: Int) -> Int = { let r = { v: { let s = step(n); s.value + 1 }, w: n }; match r { {v, w} -> v + w } }

fn aliased(n: Int) -> Int = {
  let s = step(n);
  let t: Step = if n > 0 { s } else { step(0) };
  let u = {..t, value: 99};
  s.value + u.value
}

type Pair = { x: Int, y: Int }

fn spin(p: Pair) -> Pair = { let a = p.x; let b = p.y; {x: b + 1, y: a} }

fn spun(n: Int) -> Int = { let p = iterate({x: n, y: 0}, 100, |q: Pair| if q.x > 40 { Stop(q) } else { Continue(spin(q)) }); p.x * 100 + p.y }

fn shared(n: Int) -> Int = { let p = {x: n, y: 1}; let q = spin(p); p.x * 1000 + q.x * 10 + q.y }

fn swapped(n: Int) -> Int = { let p = {x: n, y: 7}; let q = {x: p.y, y: p.x}; let r = {x: q.y, y: q.x}; p.x + q.x * 10 + r.y * 100 }

fn leftover(p: Pair, b: Bool) -> Int = { let s = p.x + p.y; if b { let q = {x: s, y: 0}; q.x } else { s } }

fn boxed_field(n: Int) -> Int = { let h = {left: step(n), right: n}; let v = h.left.value; let k = {left: step(v), right: h.right}; k.left.value + k.right }

fn counted(n: Int) -> Map<Int, Int> = fold(range(0, n), map_new(), |m: Map<Int, Int>, i: Int| map_insert(m, i % 3, i))

fn looked_up(n: Int) -> Int = match map_get(counted(n), 1) { Some(v) -> v * 10, None -> 0 - 1 }

fn looked_up_any(n: Int) -> Int = match map_get(counted(n), 1) { None -> 0 - 1, _ -> 7 }

fn looked_up_nested(n: Int) -> Int = match map_get(counted(n), 2) { Some(5) -> 55, Some(v) -> v, None -> 0 - 1 }

fn looked_up_twice(n: Int) -> Int = { let m = counted(n); match map_get(m, 0) { Some(a) -> match map_get(m, 1) { Some(b) -> a + b, None -> a }, None -> 0 } }

fn looked_up_by_list(n: Int) -> Int = { let m = map_insert(map_new(), [n, n + 1], n); match map_get(m, [n, n + 1]) { Some(v) -> v, None -> 0 - 1 } }

fn indexed(n: Int) -> Int = { let xs = map(range(0, n), |x: Int| x + 1); match list_at(xs, 2) { Some(v) -> v * 10, None -> 0 - 1 } }

fn indexed_past(n: Int) -> Int = match list_at(map(range(0, n), |x: Int| x + 1), n + 5) { Some(_) -> 1, None -> 0 - 1 }

fn ordered(a: Int, b: Int) -> Int = match compare(a, b) { Less -> 0 - 1, Equal -> 0, Greater -> 1 }

fn grown_apart(n: Int) -> Int = { let a = push([], n); let b = push([], n + 1); let c = push(push([], n + 2), n + 3); len(a) * 100 + len(b) * 10 + len(c) + fold(a, 0, |acc: Int, x: Int| acc + x) + fold(b, 0, |acc: Int, x: Int| acc + x) }

fn maps_apart(n: Int) -> Int = { let a = map_insert(map_new(), n, 1); let b = map_insert(map_new(), n + 1, 2); let e = map_new(); map_len(a) * 100 + map_len(b) * 10 + map_len(e) + (match map_get(a, n + 1) { Some(_) -> 1000, None -> 0 }) }

fn turned(x: Int, n: Int) -> Int = rotr32(x, n)

fn wrapping(a: Int, b: Int) -> Int = wrap_add(a, b) + wrap_sub(a, b) + wrap_mul(a, b)

fn turned_by_name(x: Int) -> Int = { let n = 4; let k = 32 - n; rotr32(x, n) + (x << k) }

fn width_of(p: Pair) -> Int = p.x * 10 + p.y

fn takes(p: Pair) -> Int = match p { {x, y} -> x - y }

fn lent_then_read(n: Int) -> Int = { let p = {x: n, y: 1}; width_of(p) + p.x }

fn lent_beside_a_move(n: Int) -> Int = { let p = {x: n, y: 2}; width_of(p) + takes(p) }

fn lent_and_moved_in_order(n: Int) -> Int = { let p = {x: n, y: 3}; summed(width_of(p), takes(p)) }

fn summed(a: Int, b: Int) -> Int = a + b

fn lent_per_step(n: Int) -> Int = fold(range(0, n), 0, |acc: Int, i: Int| acc + width_of({x: i, y: i}))

fn read_by_step(acc: Pair, i: Int) -> Int = acc.x + i

fn lent_to_a_step(n: Int) -> Int = { let p = {x: n, y: 0}; fold(range(0, 3), 0, |acc: Int, i: Int| acc + read_by_step(p, i)) + p.y }
"#;

fn call(unit: &'static Cranelift, name: &str, args: &[Value]) -> Option<Value> {
    let backend = unit.attach(&ply_eval::BackendSpec::honest());
    backend.enter(&Symbol::new(name), args, 10_000)
}

/// Closures and the callbacks that take them: a lambda capturing a parameter, nested lambdas, a
/// named function and a constructor and a builtin used as values, a call through a parameter and
/// through a `let`, `iterate` stopping and running out, `map_fold` over a map built in place, and
/// the bitwise operators.
const CLOSURES: &str = r#"
fn sum_to(n: Int) -> Int = fold(range(0, n), 0, |acc, x| acc + x)

fn keyed_sum(n: Int) -> Int = map_fold(fold(range(0, n), map_new(), |m, i| map_insert(m, n - i, i)), 0, |acc, k, v| acc * 10 + k + v)

fn scaled_sum(n: Int, k: Int) -> Int = fold(map(range(0, n), |x| x * k), 0, |a, b| a + b)

fn even_count(n: Int) -> Int = len(filter(range(0, n), |x| x % 2 == 0))

fn countdown(n: Int) -> Int = iterate(n, 1000, |s| if s <= 0 { Stop(s) } else { Continue(s - 1) })

fn stuck(n: Int) -> Int = iterate(n, 3, |s| Continue(s + 1))

fn twice(f: (Int) -> Int, x: Int) -> Int = f(f(x))

fn inc(x: Int) -> Int = x + 1

fn plus_two(x: Int) -> Int = twice(inc, x)

fn tripled(x: Int) -> Int = { let g = |y| y * 3; g(x) }

fn nested(k: Int, n: Int) -> Int = fold(range(0, n), 0, |acc, x| acc + fold([x, k], 0, |a, b| a + b))

fn bits(n: Int) -> Int = ((n << 2) | (n >> 1)) ^ (n & 3)

fn bad_shift(n: Int) -> Int = 1 << n

fn flipped(n: Int) -> Int = ~n

fn wrapped_count(n: Int) -> Int = len(map(range(0, n), Some))

fn named_count(n: Int) -> Int = len(map(range(0, n), int_to_string))

fn adder(k: Int) -> (Int) -> Int = |x| x + k

fn added(k: Int, x: Int) -> Int = adder(k)(x)

fn add(a: Int, b: Int) -> Int = a + b

fn stepped(n: Int) -> Int = fold(range(0, n), 0, add)

type Acc = { total: Int, count: Int }

fn bump(a: Acc, x: Int) -> Acc = {..a, total: a.total + x, count: a.count + 1}

fn totals(n: Int) -> Int = { let a = fold(range(0, n), {total: 0, count: 0}, bump); a.total + a.count }

fn walked(n: Int, k: Int) -> Int = iterate({total: 0, count: 0}, n + 1, |a: Acc| if a.count >= n { Stop(a.total) } else { Continue({..a, total: a.total + k, count: a.count + 1}) })

fn huge(n: Int) -> Int = fold(range(0, 20000000 + n), 0, add)

fn listed_sum(n: Int) -> Int = fold(map(range(0, n), inc), 0, add)

fn kept(n: Int) -> Int = len(filter(map(range(0, n), inc), |x: Int| x % 2 == 1))

fn longer(n: Int) -> Int = fold(map(range(0, n), inc), 0, |acc: Int, x: Int| acc + x * 2)
"#;

/// The control every other test here is read against: the fragment is not empty, and it is not
/// everything.
#[test]
fn the_fragment_is_neither_empty_nor_everything() {
    let (loaded, unit) = unit(ARITHMETIC);
    let total = loaded.program.modules.iter().flat_map(|m| &m.items).count();
    assert!(unit.len() >= 6, "the fragment holds {}", unit.len());
    assert!(
        !unit.refusals().is_empty(),
        "nothing was refused over the whole standard library, so the fixpoint is not deciding \
         anything: {} items, {} compiled",
        total,
        unit.compiled().len()
    );
    // `adder` answers a function, which the seam never carries, so a call of it declines however
    // wide the registry — the gap `Mutation::Unoffered` lives in.
    assert!(!unit.compiled().is_empty());
    let members: Vec<&str> = unit.compiled().iter().map(String::as_str).collect();
    assert!(members.contains(&"m.double"), "{members:?}");
}

/// The whole point: a call the seam admits is answered by native code, and the answer is the
/// interpreter's.
#[test]
fn a_compiled_body_answers_what_the_interpreter_answers() {
    let (_, unit) = unit(ARITHMETIC);
    let cases: &[(&str, Vec<Value>, Value)] = &[
        ("m.double", vec![Value::Int(21)], Value::Int(42)),
        ("m.even", vec![Value::Int(4)], Value::Bool(true)),
        ("m.even", vec![Value::Int(7)], Value::Bool(false)),
        (
            "m.clamp",
            vec![Value::Int(150), Value::Int(0), Value::Int(100)],
            Value::Int(100),
        ),
        ("m.collatz", vec![Value::Int(27)], Value::Int(111)),
        ("m.sign", vec![Value::Int(-9)], Value::Int(-1)),
        ("m.sign", vec![Value::Int(0)], Value::Int(0)),
        ("m.busy", vec![Value::Int(9)], Value::Int(19)),
        ("m.capped", vec![Value::Int(9)], Value::Int(7)),
        ("m.capped", vec![Value::Int(8)], Value::Int(8)),
        ("m.capped", vec![Value::Int(3)], Value::Int(3)),
    ];
    for (name, args, want) in cases {
        let got = call(unit, name, args);
        assert_eq!(
            got.as_ref(),
            Some(want),
            "`{name}{args:?}` answered {got:?}, not {want:?}"
        );
    }
}

/// A body outside the fragment is a registry miss, not a wrong answer.
/// Compiled code answers what the interpreter answers over `++` and the nested patterns.
///
/// These paths are compiled by every census over the parser but entered by no workload measured
/// so far, so without this the fixpoint's own count is the only thing vouching for them.
#[test]
#[allow(clippy::arc_with_non_send_sync)]
fn a_compiled_body_answers_over_concat_and_nested_patterns() {
    let (_, unit) = unit(SHAPES);
    let cases: &[(&str, Vec<Value>, Value)] = &[
        ("m.taken", vec![Value::Int(4)], Value::Int(9)),
        ("m.ignored", vec![Value::Int(7)], Value::Int(7)),
        ("m.nested", vec![Value::Int(4)], Value::Int(9)),
        ("m.nested", vec![Value::Int(-3)], Value::Int(3)),
        ("m.let_taken", vec![Value::Int(4)], Value::Int(9)),
        ("m.let_rest", vec![Value::Int(7)], Value::Int(7)),
        ("m.let_tuple", vec![Value::Int(4)], Value::Int(20)),
        ("m.let_nested", vec![Value::Int(4)], Value::Int(8)),
        // A written field's own block projects a record it binds; that record is not the
        // literal's update base.
        ("m.projected", vec![Value::Int(4)], Value::Int(9)),
        // A branch answering a local must not alias it at one count: the update through the
        // alias would otherwise write into the original.
        ("m.aliased", vec![Value::Int(4)], Value::Int(103)),
        ("m.listed", vec![Value::Int(5)], Value::Int(7)),
        ("m.joined", vec![Value::Int(11)], Value::Int(11)),
        // A record rebuilt from a dying one of its width takes the dying one's memory
        // (ADR 0036, Decision 8); one still held elsewhere is left where it is, and a literal
        // written from a base of another shape, or from one read later, is built as written.
        ("m.spun", vec![Value::Int(1)], Value::Int(4_140)),
        ("m.shared", vec![Value::Int(3)], Value::Int(3_023)),
        ("m.swapped", vec![Value::Int(2)], Value::Int(772)),
        (
            "m.leftover",
            vec![
                Value::Record(std::sync::Arc::new(
                    [("x", Value::Int(4)), ("y", Value::Int(5))]
                        .into_iter()
                        .map(|(k, v)| (Symbol::new(k), v))
                        .collect(),
                )),
                Value::Bool(true),
            ],
            Value::Int(9),
        ),
        (
            "m.leftover",
            vec![
                Value::Record(std::sync::Arc::new(
                    [("x", Value::Int(4)), ("y", Value::Int(5))]
                        .into_iter()
                        .map(|(k, v)| (Symbol::new(k), v))
                        .collect(),
                )),
                Value::Bool(false),
            ],
            Value::Int(9),
        ),
        ("m.boxed_field", vec![Value::Int(4)], Value::Int(8)),
        // A `match` over `map_get` whose arms only ask whether the key was found builds no
        // constructor: the value found, or nothing, is tested directly.
        ("m.looked_up", vec![Value::Int(6)], Value::Int(40)),
        ("m.looked_up", vec![Value::Int(1)], Value::Int(-1)),
        ("m.looked_up_any", vec![Value::Int(2)], Value::Int(7)),
        ("m.looked_up_any", vec![Value::Int(1)], Value::Int(-1)),
        ("m.looked_up_nested", vec![Value::Int(6)], Value::Int(55)),
        ("m.looked_up_nested", vec![Value::Int(3)], Value::Int(2)),
        ("m.looked_up_nested", vec![Value::Int(2)], Value::Int(-1)),
        ("m.looked_up_twice", vec![Value::Int(6)], Value::Int(7)),
        ("m.looked_up_by_list", vec![Value::Int(4)], Value::Int(4)),
        // The same shape over `list_at`; a three-way compare answering singletons; and the empty
        // list and the empty map made once, grown apart.
        ("m.indexed", vec![Value::Int(5)], Value::Int(30)),
        ("m.indexed", vec![Value::Int(2)], Value::Int(-1)),
        ("m.indexed_past", vec![Value::Int(3)], Value::Int(-1)),
        (
            "m.ordered",
            vec![Value::Int(1), Value::Int(2)],
            Value::Int(-1),
        ),
        (
            "m.ordered",
            vec![Value::Int(2), Value::Int(2)],
            Value::Int(0),
        ),
        (
            "m.ordered",
            vec![Value::Int(3), Value::Int(2)],
            Value::Int(1),
        ),
        ("m.grown_apart", vec![Value::Int(4)], Value::Int(121)),
        ("m.maps_apart", vec![Value::Int(7)], Value::Int(110)),
        // The rotate of the low word and the wrapping arithmetic, each one instruction in a
        // compiled body, answering what the interpreter answers at the edges.
        (
            "m.turned",
            vec![Value::Int(1), Value::Int(1)],
            Value::Int(0x8000_0000),
        ),
        (
            "m.turned",
            vec![Value::Int(0x1_0000_0001), Value::Int(1)],
            Value::Int(0x8000_0000),
        ),
        (
            "m.turned",
            vec![Value::Int(-1), Value::Int(7)],
            Value::Int(0xFFFF_FFFF),
        ),
        (
            "m.turned",
            vec![Value::Int(2), Value::Int(-1)],
            Value::Int(4),
        ),
        (
            "m.wrapping",
            vec![Value::Int(i64::MAX), Value::Int(1)],
            Value::Int(i64::MIN + (i64::MAX - 1) + i64::MAX),
        ),
        (
            "m.wrapping",
            vec![Value::Int(6), Value::Int(7)],
            Value::Int(13 - 1 + 42),
        ),
        (
            "m.turned_by_name",
            vec![Value::Int(1)],
            Value::Int(0x1000_0000 + (1 << 28)),
        ),
        // A callee that only reads a record borrows it: the caller's hold outlives the call,
        // beside a later read, beside a call that takes it, and inside a step called per
        // iteration.
        ("m.lent_then_read", vec![Value::Int(4)], Value::Int(41 + 4)),
        (
            "m.lent_beside_a_move",
            vec![Value::Int(5)],
            Value::Int(52 + 3),
        ),
        (
            "m.lent_and_moved_in_order",
            vec![Value::Int(6)],
            Value::Int(63 + 3),
        ),
        ("m.lent_per_step", vec![Value::Int(3)], Value::Int(11 + 22)),
        (
            "m.lent_to_a_step",
            vec![Value::Int(7)],
            Value::Int(7 * 3 + 3),
        ),
    ];
    for (name, args, want) in cases {
        assert_eq!(
            call(unit, name, args),
            Some(want.clone()),
            "{name} answered differently through compiled code"
        );
    }
}

/// Compiled code answers what the interpreter answers over closures, the callback builtins and
/// the bitwise operators — the constructs the parser census ranked first.
#[test]
fn a_compiled_body_answers_over_closures_and_callbacks() {
    let (_, unit) = unit(CLOSURES);
    let refused: Vec<String> = unit
        .refusals()
        .iter()
        .filter(|(f, _)| f.starts_with("m."))
        .map(|(f, c)| format!("{f}: {c}"))
        .collect();
    assert!(refused.is_empty(), "{refused:#?}");
    let cases: &[(&str, Vec<Value>, Value)] = &[
        ("m.sum_to", vec![Value::Int(10)], Value::Int(45)),
        (
            "m.scaled_sum",
            vec![Value::Int(4), Value::Int(3)],
            Value::Int(18),
        ),
        ("m.even_count", vec![Value::Int(7)], Value::Int(4)),
        ("m.keyed_sum", vec![Value::Int(3)], Value::Int(333)),
        ("m.countdown", vec![Value::Int(5)], Value::Int(0)),
        ("m.plus_two", vec![Value::Int(5)], Value::Int(7)),
        ("m.tripled", vec![Value::Int(4)], Value::Int(12)),
        (
            "m.nested",
            vec![Value::Int(10), Value::Int(3)],
            Value::Int(33),
        ),
        ("m.bits", vec![Value::Int(5)], Value::Int(23)),
        ("m.flipped", vec![Value::Int(5)], Value::Int(-6)),
        ("m.wrapped_count", vec![Value::Int(3)], Value::Int(3)),
        ("m.named_count", vec![Value::Int(3)], Value::Int(3)),
        (
            "m.added",
            vec![Value::Int(10), Value::Int(5)],
            Value::Int(15),
        ),
        // Fused loops: a `fold` over a range with a compiled step, over an `Int` and over a
        // record, and an `iterate` with a lambda that captures.
        ("m.stepped", vec![Value::Int(10)], Value::Int(45)),
        ("m.totals", vec![Value::Int(4)], Value::Int(10)),
        // Fused loops over a list built by a fused `map`: a typed step, a filter, a lambda step
        // over a list of a hundred so the walk crosses a leaf.
        ("m.listed_sum", vec![Value::Int(10)], Value::Int(55)),
        ("m.kept", vec![Value::Int(5)], Value::Int(3)),
        ("m.longer", vec![Value::Int(100)], Value::Int(10100)),
        (
            "m.walked",
            vec![Value::Int(5), Value::Int(3)],
            Value::Int(15),
        ),
    ];
    for (name, args, want) in cases {
        let got = call(unit, name, args);
        assert_eq!(
            got.as_ref(),
            Some(want),
            "`{name}{args:?}` answered {got:?}, not {want:?}"
        );
    }
}

/// A closure never crosses the seam: a definition answering a function is registered like any
/// other, runs, and has its answer refused by the backend itself, while the compiled body that
/// calls through the same closure answers.
#[test]
fn a_native_closure_stays_inside_the_entry_that_made_it() {
    let (_, unit) = unit(CLOSURES);
    assert!(unit.compiled().iter().any(|f| f == "m.adder"));
    assert_eq!(call(unit, "m.adder", &[Value::Int(1)]), None);
    assert_eq!(
        call(unit, "m.added", &[Value::Int(1), Value::Int(2)]),
        Some(Value::Int(3))
    );
}

/// What raises in the interpreter declines here: an `iterate` past its budget, and a shift by a
/// count outside `0..64`.
#[test]
fn a_callback_that_raises_declines_rather_than_answering() {
    let (_, unit) = unit(CLOSURES);
    assert_eq!(call(unit, "m.stuck", &[Value::Int(0)]), None);
    // A fused loop declines where the runtime's would: a range past the interpreter's limit.
    assert_eq!(call(unit, "m.huge", &[Value::Int(1)]), None);
    assert_eq!(call(unit, "m.bad_shift", &[Value::Int(70)]), None);
    assert_eq!(
        call(unit, "m.bad_shift", &[Value::Int(3)]),
        Some(Value::Int(8))
    );
}

#[test]
fn a_definition_the_fragment_has_no_body_for_is_declined() {
    let (_, unit) = unit(ARITHMETIC);
    assert_eq!(call(unit, "m.no_such_function", &[Value::Int(1)]), None);
    // A carried list answer crosses now that every compiled function is registered.
    assert_eq!(
        call(unit, "m.shaped", &[Value::Int(1)]),
        Some(Value::list(vec![Value::Int(1), Value::Int(1)]))
    );
}

/// A call with the wrong number of arguments declines rather than reading past the argument array
/// it was handed.
#[test]
fn a_call_of_the_wrong_arity_is_declined() {
    let (_, unit) = unit(ARITHMETIC);
    assert_eq!(call(unit, "m.clamp", &[Value::Int(1)]), None);
    assert_eq!(
        call(
            unit,
            "m.double",
            &[Value::Int(1), Value::Int(2), Value::Int(3)]
        ),
        None
    );
}

/// `budget` is the machine's remaining nested calls and not a hint.
#[test]
fn a_recursion_past_the_budget_declines_rather_than_running_it() {
    let (_, unit) = unit(ARITHMETIC);
    let backend = unit.attach(&ply_eval::BackendSpec::honest());
    let ladder = Symbol::new("m.ladder");
    assert_eq!(
        backend.enter(&ladder, &[Value::Int(100)], 8),
        None,
        "a hundred-deep recursion answered on a budget of eight"
    );
    assert_eq!(
        backend.enter(&ladder, &[Value::Int(100)], 10_000),
        Some(Value::Int(100)),
        "the same call declined on a budget that fits it, so the decline above says nothing"
    );
}

/// Arithmetic that raises in the interpreter declines here rather than answering a wrapped value.
#[test]
fn an_overflow_declines_rather_than_wrapping() {
    let (_, unit) = unit(ARITHMETIC);
    assert_eq!(call(unit, "m.double", &[Value::Int(i64::MAX)]), None);
}

/// Pointer identity, as `code::Lowering::describes` is and for the same reason: a bisection builds
/// programs whose definitions carry the names of the ones they replace, and a registry keyed on a
/// bare name would answer for the wrong body.
#[test]
fn a_backend_declines_to_describe_a_program_it_was_not_built_from() {
    let (loaded, unit) = unit(ARITHMETIC);
    let other = load(ARITHMETIC);
    let backend = unit.attach(&ply_eval::BackendSpec::honest());
    assert!(backend.describes(loaded.program));
    assert!(!backend.describes(other.program));
}

/// The set is closed under calls, which is the property that makes the promise
/// `ply_codegen::backend` gives the machine true by construction: from inside a member there is no
/// reachable call that leaves compiled code.
#[test]
fn the_compiled_set_is_closed_under_calls() {
    let (loaded, unit) = unit(ARITHMETIC);
    let source = ply_codegen::Source::new(loaded.program, loaded.resolved, loaded.check);
    let source: &'static ply_codegen::Source = Box::leak(Box::new(source));
    let names: Vec<&str> = unit.compiled().iter().map(String::as_str).collect();
    let refusals = ply_codegen::Jit::refusals(source, &names, ply_codegen::Opts::default())
        .expect("the set compiles");
    assert!(
        refusals.is_empty(),
        "the fixpoint returned a set that still refuses: {:?}",
        refusals
            .iter()
            .map(|r| (r.function.as_str(), r.construct.as_str()))
            .collect::<Vec<_>>()
    );
}

/// The census this crate exists to move, printed so that a run of the suite says what the fragment
/// reached rather than only that it did not crash.
#[test]
fn the_census_over_the_standard_library() {
    let (loaded, unit) = unit(ARITHMETIC);
    let functions = ply_codegen::Source::new(loaded.program, loaded.resolved, loaded.check)
        .functions()
        .len();
    let mut by_construct: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, construct) in unit.refusals() {
        *by_construct.entry(construct.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = by_construct.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    println!(
        "{functions} functions, {} compiled as one closed unit, {} of those enterable",
        unit.compiled().len(),
        unit.len()
    );
    println!("refused, by construct, most first:");
    for (construct, count) in ranked.iter().take(20) {
        println!("  {count:5}  {construct}");
    }
    let c = unit.compilation();
    println!(
        "analysis {:.1}ms, codegen {:.1}ms over {} unit(s)",
        c.analysis_nanos as f64 / 1e6,
        c.codegen_nanos as f64 / 1e6,
        c.units
    );
    assert!(
        unit.len() >= 6,
        "the enterable fragment fell to {} definitions",
        unit.len()
    );
    assert!(functions > 100, "only {functions} functions were offered");
}
