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

fn shaped(x: Int) -> List<Int> = [x, x]
"#;

fn call(unit: &'static Cranelift, name: &str, args: &[Value]) -> Option<Value> {
    let backend = unit.attach(&ply_eval::BackendSpec::honest());
    backend.enter(&Symbol::new(name), args, 10_000)
}

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
    // `shaped` returns a `List<Int>`, so the seam could never carry its answer and it is not
    // registered — which is the gap `Mutation::Unoffered` lives in.
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
#[test]
fn a_definition_the_fragment_has_no_body_for_is_declined() {
    let (_, unit) = unit(ARITHMETIC);
    assert_eq!(call(unit, "m.shaped", &[Value::Int(1)]), None);
    assert_eq!(call(unit, "m.no_such_function", &[Value::Int(1)]), None);
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
