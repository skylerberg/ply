//! An adversarial audit of what a `Value` *means* after ADR 0019 §1 and §2.

// A `Value::Record` holds `Arc<BTreeMap<Symbol, Value>>` and a `Value` is not `Send`; the same
// allow `secrets.rs` carries, for the same reason.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::{CheckOutput, check_program};
use ply_eval::{
    ARGUMENT_VECTOR_CLASSES, Decimal, Interp, Machine, SECRET_REDACTED, Value, first_difference,
    values_equal,
};
use ply_span::{Diagnostic, SourceId, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let inputs = [(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
    let resolved =
        resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
    let check = check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
    Compiled {
        program,
        resolved,
        check,
    }
}

impl Compiled {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    #[track_caller]
    fn run(&self, name: &str) {
        if let Err(d) = self.machine().eval_test(self.index_of(name)) {
            panic!("{name:?} was expected to pass:\n{d:#?}");
        }
    }

    #[track_caller]
    fn call(&self, name: &str, args: Vec<Value>) -> Result<Value, Diagnostic> {
        self.machine().call(name, args, Span::DUMMY)
    }
}

// --- 1. the argument vector under multi-shot resumption ---------------------

/// A continuation captured **inside an argument list**, resumed twice.
const RESUMED_ARGUMENTS: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

fn pick(t: Int, f: Int) -> Int = if amb.flip[coin]() { t } else { f }

fn one(a: Int) -> Int = a
fn two(a: Int, b: Int) -> Int = a * 10 + b
fn three(a: Int, b: Int, c: Int) -> Int = a * 100 + b * 10 + c
fn four(a: Int, b: Int, c: Int, d: Int) -> Int = a * 1000 + b * 100 + c * 10 + d
fn five(a: Int, b: Int, c: Int, d: Int, e: Int) -> Int =
  a * 10000 + b * 1000 + c * 100 + d * 10 + e

// Each of these captures the continuation part-way through an argument list and
// resumes it twice with different values. The clause combines the two answers so
// that both have to be right for the total to be.
test "arity 1" {
  let total = handle { one(pick(2, 7)) } with {
    amb.flip[coin]() resume k -> k(true) * 100 + k(false),
    return x -> x
  };
  assert_eq(total, 2 * 100 + 7)
}

test "arity 2" {
  let total = handle { two(1, pick(2, 7)) } with {
    amb.flip[coin]() resume k -> k(true) * 1000 + k(false),
    return x -> x
  };
  assert_eq(total, 12 * 1000 + 17)
}

test "arity 3" {
  let total = handle { three(1, pick(2, 7), 3) } with {
    amb.flip[coin]() resume k -> k(true) * 10000 + k(false),
    return x -> x
  };
  assert_eq(total, 123 * 10000 + 173)
}

test "arity 4" {
  let total = handle { four(1, pick(2, 7), 3, 4) } with {
    amb.flip[coin]() resume k -> k(true) * 100000 + k(false),
    return x -> x
  };
  assert_eq(total, 1234 * 100000 + 1734)
}

test "arity 5" {
  let total = handle { five(1, pick(2, 7), 3, 4, 5) } with {
    amb.flip[coin]() resume k -> k(true) * 1000000 + k(false),
    return x -> x
  };
  assert_eq(total, 12345 * 1000000 + 17345)
}

// Nested: the outer call's buffer is still half-filled while the inner call
// takes, fills and hands back a buffer of the same class, twice over.
test "nested calls of one class" {
  let total = handle { four(1, four(9, pick(2, 7), 9, 9), 3, 4) } with {
    amb.flip[coin]() resume k -> k(true) * 100000 + k(false),
    return x -> x
  };
  assert_eq(total, 930934 * 100000 + 980934)
}

// A `String` argument, so the buffer carries a value with a payload rather than
// five `i64`s: a residue would render.
test "a string argument survives two resumptions" {
  let joined = handle {
    string_concat(string_concat("a", if amb.flip[coin]() { "b" } else { "c" }), "d")
  } with {
    amb.flip[coin]() resume k -> string_concat(k(true), k(false)),
    return x -> x
  };
  assert_eq(joined, "abdacd")
}
"#;

#[test]
fn an_argument_vector_split_by_two_resumptions_carries_each_resumptions_own_arguments() {
    let compiled = compile(RESUMED_ARGUMENTS);
    for name in [
        "arity 1",
        "arity 2",
        "arity 3",
        "arity 4",
        "arity 5",
        "nested calls of one class",
        "a string argument survives two resumptions",
    ] {
        compiled.run(name);
    }
}

/// The engine that cannot run any of the above, stated rather than assumed.
#[test]
fn the_tree_walker_refuses_every_resumption_case_so_engine_both_compares_none_of_them() {
    let compiled = compile(RESUMED_ARGUMENTS);
    for name in ["arity 1", "arity 4", "nested calls of one class"] {
        let index = compiled.index_of(name);
        let refused = Interp::new(&compiled.program, &compiled.resolved, &compiled.check)
            .eval_test(index)
            .expect_err("the tree-walker cannot bind a continuation");
        assert_eq!(
            refused.code,
            ply_span::codes::MACHINE_ONLY_CLAUSE,
            "{name}: the tree-walker refused for an unexpected reason: {refused:#?}"
        );
    }
}

// --- 2. a credential in an argument vector ----------------------------------

const SECRET_ARGUMENTS: &str = r#"
fn keep1(s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep2(a: Int, s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep3(a: Int, b: Int, s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep4(a: Int, b: Int, c: Int, s: Secret<String>) -> Bool = secret_is_empty(s)
fn keep5(a: Int, b: Int, c: Int, d: Int, s: Secret<String>) -> Bool = secret_is_empty(s)

pub fn carry1(s: Secret<String>) -> Bool = keep1(s)
pub fn carry2(s: Secret<String>) -> Bool = keep2(1, s)
pub fn carry3(s: Secret<String>) -> Bool = keep3(1, 2, s)
pub fn carry4(s: Secret<String>) -> Bool = keep4(1, 2, 3, s)
pub fn carry5(s: Secret<String>) -> Bool = keep5(1, 2, 3, 4, s)

// Deeper than `argv::KEEP`, so the list fills, overflows and drains again while
// a credential is in every frame's argument vector.
fn descend(depth: Int, s: Secret<String>) -> Int =
  if depth <= 0 { 0 } else { descend(depth - 1, s) + 1 }

pub fn deep(s: Secret<String>) -> Int = descend(2000, s)

// The observation a dirty buffer would break: a call of the same arity made
// right after one that carried a credential must see exactly its own arguments.
pub fn after4(a: Int, b: Int, c: Int, d: Int) -> Int = a * 1000 + b * 100 + c * 10 + d
"#;

/// ADR 0015 §2 as a bound on the free list, measured through the machine.
#[test]
fn a_credential_passed_as_an_argument_is_unreachable_once_the_call_returns() {
    let compiled = compile(SECRET_ARGUMENTS);
    for arity in 1..=ARGUMENT_VECTOR_CLASSES + 1 {
        let payload = Arc::new(Value::str("hunter2"));
        let secret = Value::Secret(Arc::clone(&payload));
        let answered = compiled
            .call(&format!("m.carry{arity}"), vec![secret])
            .unwrap_or_else(|d| panic!("`carry{arity}` raised: {d:#?}"));
        assert_eq!(answered, Value::Bool(false), "arity {arity}");
        assert_eq!(
            Arc::strong_count(&payload),
            1,
            "after a call at arity {arity} returned, a credential was still reachable from \
             something the evaluator kept — a recycled argument buffer holds its contents"
        );
    }
}

/// The same, past the free list's bound and back.
#[test]
fn a_recursion_deeper_than_the_free_lists_bound_leaves_no_credential_behind() {
    let compiled = compile(SECRET_ARGUMENTS);
    let payload = Arc::new(Value::str("hunter2"));
    let secret = Value::Secret(Arc::clone(&payload));
    let answered = compiled
        .call("m.deep", vec![secret])
        .unwrap_or_else(|d| panic!("`deep` raised: {d:#?}"));
    assert_eq!(answered, Value::Int(2000));
    assert_eq!(
        Arc::strong_count(&payload),
        1,
        "a 2000-frame recursion carrying a credential left one reachable after it unwound"
    );
}

/// A buffer that carried a credential is handed to the next call of that arity.
#[test]
fn a_call_made_after_one_that_carried_a_credential_sees_only_its_own_arguments() {
    let compiled = compile(SECRET_ARGUMENTS);
    for _ in 0..64 {
        let secret = Value::secret(Value::str("hunter2"));
        let _ = compiled
            .call("m.carry4", vec![secret])
            .expect("`carry4` runs");
        let answered = compiled
            .call(
                "m.after4",
                vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)],
            )
            .expect("`after4` runs");
        assert_eq!(
            answered,
            Value::Int(1234),
            "the call after one that carried a credential was built from a buffer that was \
             not empty"
        );
    }
}

// --- 3. rendering paths other than `Value::write` ---------------------------

/// `first_difference` is a second structural walk over a value and it builds text that is
/// **stored** — `builtins::assert_failure` puts it in the note of a failing assertion and
/// `ply-store` caches that as `Outcome::Fail { message }`.
#[test]
fn the_assertion_differ_never_descends_into_a_credential() {
    let hidden = "hunter2";
    let other = "correct-horse-battery-staple";
    let secret = |s: &str| Value::secret(Value::str(s));
    let record = |s: Value| {
        Value::Record(Arc::new(
            [(ply_span::Symbol::new("password"), s)]
                .into_iter()
                .collect(),
        ))
    };

    let pairs: Vec<(&str, Value, Value)> = vec![
        ("bare", secret(hidden), secret(other)),
        (
            "in a list",
            Value::list(vec![Value::Int(1), secret(hidden)]),
            Value::list(vec![Value::Int(1), secret(other)]),
        ),
        ("in a record", record(secret(hidden)), record(secret(other))),
        (
            "in a variant",
            Value::ctor("Login", vec![secret(hidden)]),
            Value::ctor("Login", vec![secret(other)]),
        ),
        (
            "as a map value",
            Value::map([(Value::str("ada"), secret(hidden))]),
            Value::map([(Value::str("ada"), secret(other))]),
        ),
        (
            "wrapping a compound",
            Value::secret(Value::list(vec![Value::str(hidden)])),
            Value::secret(Value::list(vec![Value::str(other)])),
        ),
        (
            "a list against a shorter one",
            Value::list(vec![secret(hidden)]),
            Value::list(vec![secret(hidden), secret(other)]),
        ),
    ];

    for (label, actual, expected) in pairs {
        for text in [
            actual.render(),
            expected.render(),
            format!("{:?}", first_difference(&actual, &expected)),
            format!("{:?}", first_difference(&expected, &actual)),
        ] {
            assert!(
                !text.contains(hidden) && !text.contains(other),
                "{label}: a payload reached a rendered string: {text}"
            );
        }
        assert!(
            actual.render().contains(SECRET_REDACTED),
            "{label}: the redaction marker is missing, so something else rendered instead"
        );
    }
}

// --- 4. both engines read one constructor memo ------------------------------

const MENTIONS: &str = r#"
type Colour = Red | Green
type Boxed = Box(Int)

pub fn nullary(ignored: Int) -> Colour = Red
pub fn closure(ignored: Int) -> (Int) -> Boxed = Box
pub fn literal(ignored: Int) -> String = "abcd"
"#;

/// **An audit gap, asserted rather than described.**
#[test]
fn both_engines_answer_a_constructor_mention_from_one_memo_and_a_literal_from_two() {
    let c = compile(MENTIONS);
    let on_both = |name: &str| -> (Value, Value) {
        let walked = Interp::new(&c.program, &c.resolved, &c.check)
            .call(name, vec![Value::Int(0)], Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised on the tree-walker: {d:#?}"));
        let stepped = Machine::new(&c.program, &c.resolved, &c.check)
            .call(name, vec![Value::Int(0)], Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised on the machine: {d:#?}"));
        (walked, stepped)
    };

    match &on_both("m.nullary") {
        (Value::Ctor { args: a, .. }, Value::Ctor { args: b, .. }) => assert!(
            Arc::ptr_eq(a, b),
            "the two engines built a nullary constructor separately; if that is now true, \
             `--engine both` has become independent evidence for `ctor_value` and this test \
             should be replaced by one that says so"
        ),
        other => panic!("a mention of `Red` is not a `Ctor`: {other:?}"),
    }
    match &on_both("m.closure") {
        (Value::Closure(a), Value::Closure(b)) => assert!(
            Arc::ptr_eq(a, b),
            "the two engines built a constructor closure separately"
        ),
        other => panic!("a mention of `Box` is not a closure: {other:?}"),
    }

    // The control, and the reason the sentence in ADR 0019 §2 is half right: a literal really is
    // built twice, so a divergence in the literal half would be visible to the differential
    // harness.
    match &on_both("m.literal") {
        (Value::Str(a), Value::Str(b)) => {
            assert!(
                !Arc::ptr_eq(a, b),
                "the tree-walker stopped building a literal per evaluation, so the literal half \
                 of ADR 0019 §2 is no longer audited by `--engine both` either"
            );
            assert_eq!(a, b, "the two engines disagree about a literal's value");
        }
        other => panic!("a literal is not a `Str`: {other:?}"),
    }
}

// --- 5. equal values that do not render alike -------------------------------

/// Every value shape this evaluator can hold, in pairs, for the scan below.
fn probe_values() -> Vec<(&'static str, Value)> {
    let dec = |m: i128, s: u32| {
        Value::Decimal(Decimal::try_from_i128_with_scale(m, s).expect("in range"))
    };
    vec![
        ("unit", Value::Unit),
        ("bool false", Value::Bool(false)),
        ("bool true", Value::Bool(true)),
        ("int 0", Value::Int(0)),
        ("int 1", Value::Int(1)),
        ("float 0.0", Value::Float(0.0)),
        ("float -0.0", Value::Float(-0.0)),
        ("float 1.5", Value::Float(1.5)),
        ("float nan", Value::Float(f64::NAN)),
        ("decimal 1.5", dec(15, 1)),
        ("decimal 1.50", dec(150, 2)),
        ("decimal 1.500", dec(1500, 3)),
        ("decimal 2.5", dec(25, 1)),
        ("str empty", Value::str("")),
        ("str a", Value::str("a")),
        ("bytes empty", Value::bytes([])),
        ("bytes a", Value::bytes(b"a")),
        ("list empty", Value::list(vec![])),
        ("list [1]", Value::list(vec![Value::Int(1)])),
        ("list [1.5m]", Value::list(vec![dec(15, 1)])),
        ("list [1.50m]", Value::list(vec![dec(150, 2)])),
        ("map empty", Value::empty_map()),
        ("map {1: 1}", Value::map([(Value::Int(1), Value::Int(1))])),
        (
            "record {a: 1.5m}",
            Value::Record(Arc::new(
                [(ply_span::Symbol::new("a"), dec(15, 1))]
                    .into_iter()
                    .collect(),
            )),
        ),
        (
            "record {a: 1.50m}",
            Value::Record(Arc::new(
                [(ply_span::Symbol::new("a"), dec(150, 2))]
                    .into_iter()
                    .collect(),
            )),
        ),
        ("ctor Red", Value::ctor("Red", vec![])),
        ("ctor Box(1.5m)", Value::ctor("Box", vec![dec(15, 1)])),
        ("ctor Box(1.50m)", Value::ctor("Box", vec![dec(150, 2)])),
    ]
}

/// The language's `==` and the `Map`'s order, checked against each other over every pair of the
/// probe corpus.
#[test]
fn the_order_and_the_language_equality_part_at_a_nan_and_also_at_negative_zero() {
    let mut disagreements = Vec::new();
    for (an, a) in probe_values() {
        for (bn, b) in probe_values() {
            let ordered = a.cmp(&b) == std::cmp::Ordering::Equal;
            let Ok(equal) = values_equal(&a, &b, Span::DUMMY) else {
                continue;
            };
            if ordered != equal {
                disagreements.push(format!("{an} vs {bn}: cmp={ordered} ==={equal}"));
            }
        }
    }
    disagreements.sort();
    assert_eq!(
        disagreements,
        vec![
            "float -0.0 vs float 0.0: cmp=false ===true".to_string(),
            "float 0.0 vs float -0.0: cmp=false ===true".to_string(),
            "float nan vs float nan: cmp=true ===false".to_string(),
        ],
        "the `Map`'s order and the language's `==` disagree somewhere new; if the new pair is \
         an ordered key type, every `Map` built from those keys holds two entries where a \
         program wrote one"
    );
}

/// **The defect this test pinned is fixed; it now asserts the fix.**
#[test]
fn two_decimals_that_are_one_map_key_render_two_strings_and_build_one_map() {
    let short = Value::Decimal(Decimal::try_from_i128_with_scale(15, 1).expect("1.5"));
    let long = Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"));

    assert!(
        values_equal(&short, &long, Span::DUMMY).expect("two decimals compare"),
        "the language's `==` stopped treating `1.5m` and `1.50m` as one value"
    );
    assert_eq!(short.cmp(&long), std::cmp::Ordering::Equal);
    assert_eq!(short.render(), "1.5");
    assert_eq!(
        long.render(),
        "1.50",
        "two values that are one `Map` key render as two different strings"
    );

    // One key, canonical, whichever spelling was written last.
    let short_then_long = Value::map([
        (short.clone(), Value::Int(1)),
        (long.clone(), Value::Int(2)),
    ]);
    let long_then_short = Value::map([
        (long.clone(), Value::Int(1)),
        (short.clone(), Value::Int(2)),
    ]);
    assert!(
        values_equal(&short_then_long, &long_then_short, Span::DUMMY).expect("two maps compare"),
        "the two maps are not even equal, which is a larger defect than the one this pins"
    );
    assert_eq!(short_then_long.render(), "{1.5: 2}");
    assert_eq!(
        long_then_short.render(),
        "{1.5: 2}",
        "two `==`-equal maps render as two different strings, so `map_keys`, `map_entries`, \
         `map_fold` and every derived encoding over them are functions of insertion history"
    );
}

/// The same claim where a program meets it: `map_insert` with a key equal to one already present
/// replaces the value, and the key a program reads back is the canonical spelling either way.
#[test]
fn map_insert_over_an_equal_decimal_key_reads_back_one_canonical_spelling() {
    let compiled = compile(
        r#"
pub fn last_wins(ignored: Int) -> String =
  string_of_keys(map_insert(map_insert(map_new(), 1.50m, 1), 1.5m, 2))

pub fn first_spelling(ignored: Int) -> String =
  string_of_keys(map_insert(map_insert(map_new(), 1.5m, 1), 1.50m, 2))

fn string_of_keys(m: Map<Decimal, Int>) -> String =
  fold(map(map_keys(m), decimal_to_string), "", string_concat)

test "the two maps are equal" {
  assert_eq(
    map_insert(map_insert(map_new(), 1.50m, 1), 1.5m, 2),
    map_insert(map_insert(map_new(), 1.5m, 1), 1.50m, 2))
}
"#,
    );
    compiled.run("the two maps are equal");
    assert_eq!(
        compiled.call("m.last_wins", vec![Value::Int(0)]).unwrap(),
        Value::str("1.5")
    );
    assert_eq!(
        compiled
            .call("m.first_spelling", vec![Value::Int(0)])
            .unwrap(),
        Value::str("1.5"),
        "two maps that `assert_eq` as one value answer `map_keys` with two different lists"
    );
}

// --- 6. a shared constant inside a seeded simulation ------------------------

/// Two spawned tasks, each mentioning a nullary constructor, a constructor closure and a string
/// literal, writing what they built into a cell.
const SIMULATED_CONSTANTS: &str = r#"
type Colour = Red | Green
type Boxed = Box(Int)

fn label(c: Colour) -> String = match c { Red -> "red", Green -> "green" }

fn unbox(b: Boxed) -> Int = match b { Box(n) -> n }

test "two tasks build constants" {
  with_cell[trace]("") { seen -> {
    simulate {
      let a = task.spawn(|| {
        cell_set(seen, string_concat(cell_get(seen), label(Red)));
        cell_set(seen, string_concat(cell_get(seen), int_to_string(unbox(Box(1)))))
      });
      let b = task.spawn(|| {
        cell_set(seen, string_concat(cell_get(seen), label(Green)));
        cell_set(seen, string_concat(cell_get(seen), int_to_string(unbox(Box(2)))))
      });
      task.join(a);
      task.join(b)
    }
  } }
}
"#;

/// One seed, run as the very first thing this thread does and again after two hundred other
/// schedules have warmed both memos.
#[test]
fn a_seeded_schedule_is_the_same_cold_and_warm_over_the_constant_memos() {
    use ply_eval::{Plan, Seed, explore};

    let compiled = compile(SIMULATED_CONSTANTS);
    let observe = |seed: &Seed| -> (ply_eval::Interleaving, String) {
        let mut machine = compiled.machine();
        machine.cells_mut().journal();
        machine.set_seed(seed.clone(), 10_000);
        let outcome = machine.eval_test(compiled.index_of("two tasks build constants"));
        let verdict = match &outcome {
            Ok(()) => "ok".to_string(),
            Err(d) => format!("{} {}", d.code, d.message),
        };
        let world: Vec<String> = machine
            .cells()
            .journalled()
            .iter()
            .map(|(slot, value)| format!("#{}={}", slot.index(), value.render()))
            .collect();
        let record = machine
            .simulated()
            .expect("the fixture reaches a `simulate` region");
        (
            record.interleaving(&outcome),
            format!("{verdict} | {}", world.join(",")),
        )
    };

    // Cold: nothing on this thread has mentioned `Red`, `Box` or `"red"` yet.
    let seed = Seed::default();
    let cold = observe(&seed);

    // Two hundred other schedules, which is what warms both memos.
    let plan = Plan {
        budget: 200,
        ..Plan::default()
    };
    let report = explore(&plan, &mut |s: &Seed| observe(s).0);
    assert!(
        report.exploration.failure.is_none(),
        "the fixture must pass under every schedule: {:?}",
        report.exploration.failure
    );
    // Not vacuous: the fixture has to have real choices in it, or "the same schedule cold and warm"
    // is a statement about a program with one schedule.
    assert!(
        report.seeds.len() > 1,
        "the fixture only ever ran one schedule, so this test compares nothing"
    );
    assert!(
        cold.0.steps.len() > 2,
        "the fixture's interleaving is {} steps; a memo that moved a step could not show up \
         in something this short",
        cold.0.steps.len()
    );

    let warm = observe(&seed);
    assert_eq!(
        format!("{cold:?}"),
        format!("{warm:?}"),
        "one seed took two different schedules in one process: a memo that is cold on the \
         first mention and warm afterwards has reached the search"
    );
}

// --- 7. one memo, many programs ---------------------------------------------

/// `interp::ctor_value`'s cache is a **process-wide thread-local keyed by the constructor's name**,
/// and nothing clears it between programs.
#[test]
fn two_programs_on_one_thread_do_not_read_each_others_constructor_of_the_same_name() {
    let nullary = compile(
        r#"
type Tag = Marker | Other
pub fn mention(ignored: Int) -> Tag = Marker
"#,
    );
    let unary = compile(
        r#"
type Tag = Marker(Int) | Other
pub fn mention(ignored: Int) -> Tag = Marker(7)
pub fn func(ignored: Int) -> (Int) -> Tag = Marker
"#,
    );

    // Both orders, because a cache defect is usually asymmetric.
    for (first, second) in [(&nullary, &unary), (&unary, &nullary)] {
        let a = first.call("m.mention", vec![Value::Int(0)]).expect("runs");
        let b = second.call("m.mention", vec![Value::Int(0)]).expect("runs");
        let (nullary_answer, unary_answer) = if std::ptr::eq(first, &nullary) {
            (a, b)
        } else {
            (b, a)
        };
        assert_eq!(
            nullary_answer.render(),
            "m.Marker",
            "the nullary `Marker` answered with the other program's value"
        );
        assert_eq!(
            unary_answer.render(),
            "m.Marker(7)",
            "the applied `Marker` answered with the other program's value"
        );
        assert!(
            !values_equal(&nullary_answer, &unary_answer, Span::DUMMY).expect("two `Ctor`s"),
            "two constructors of one name and two arities compared equal"
        );
    }

    // And the closure half: a mention of the arity-1 `Marker` is a function whose arity is 1,
    // whatever the nullary program left in the cache.
    let _ = nullary
        .call("m.mention", vec![Value::Int(0)])
        .expect("runs");
    let f = unary.call("m.func", vec![Value::Int(0)]).expect("runs");
    match &f {
        Value::Closure(c) => assert_eq!(
            c.arity(),
            1,
            "the cache handed back a nullary constructor's value for a constructor of arity 1"
        ),
        other => panic!("a mention of `Marker` at arity 1 is not a closure: {other:?}"),
    }
}

// --- 8. the width ADR 0019 §4 rejected narrowing at -------------------------

/// ADR 0019 §4 refuses to narrow `Value` and §"What would make this ADR wrong" names *"if a build
/// agent has to widen `Value` past 32 bytes to land any of this"* as one of five conditions that
/// would sink the document.
#[test]
fn a_value_is_still_thirty_two_bytes_wide_and_an_optional_one_costs_nothing() {
    assert_eq!(
        size_of::<Value>(),
        32,
        "`Value` changed width; ADR 0019 §4's refusal to narrow it and §1's arithmetic over \
         885.6 Value-wide slots per request were both taken at 32"
    );
    assert_eq!(
        size_of::<Option<Value>>(),
        size_of::<Value>(),
        "`Option<Value>` stopped being niche-optimized, so every arena slot and every scope \
         binding grew"
    );
}

/// The blast radius the defect had, now the blast radius the fix has to cover: it was never only
/// `Map<Decimal, _>`.
#[test]
fn a_record_key_holding_a_decimal_is_canonical_in_the_compound_key_too() {
    let compiled = compile(
        r#"
type Line = {sku: String, price: Decimal}

fn prices(m: Map<Line, Int>) -> String =
  fold(map(map_keys(m), |l: Line| decimal_to_string(l.price)), "", string_concat)

pub fn wrote_rounded(ignored: Int) -> String =
  prices(map_insert(
    map_insert(map_new(), {sku: "bolt", price: 1.50m}, 1),
    {sku: "bolt", price: 1.5m}, 2))

pub fn wrote_exact(ignored: Int) -> String =
  prices(map_insert(
    map_insert(map_new(), {sku: "bolt", price: 1.5m}, 1),
    {sku: "bolt", price: 1.50m}, 2))

test "one line, either way" {
  assert_eq(map_len(map_insert(
    map_insert(map_new(), {sku: "bolt", price: 1.50m}, 1),
    {sku: "bolt", price: 1.5m}, 2)), 1)
}
"#,
    );
    compiled.run("one line, either way");
    assert_eq!(
        compiled
            .call("m.wrote_rounded", vec![Value::Int(0)])
            .unwrap(),
        Value::str("1.5")
    );
    assert_eq!(
        compiled.call("m.wrote_exact", vec![Value::Int(0)]).unwrap(),
        Value::str("1.5"),
        "a record key holding a `Decimal` reads back a price the program did not write last"
    );
}

/// A credential under a key the canonical form rebuilds is **not** descended into and **not**
/// rebuilt.
#[test]
fn canonicalizing_a_key_clones_a_credential_rather_than_rebuilding_it() {
    let payload = Arc::new(Value::str("hunter2"));
    let secret = Value::Secret(Arc::clone(&payload));
    let key = Value::Record(Arc::new(
        [
            (ply_span::Symbol::new("d"), {
                Value::Decimal(Decimal::try_from_i128_with_scale(150, 2).expect("1.50"))
            }),
            (ply_span::Symbol::new("p"), secret),
        ]
        .into_iter()
        .collect(),
    ));

    let m = Value::map([(key, Value::Int(1))]);
    let Value::Map(entries) = &m else {
        panic!("not a map")
    };
    let (stored, _) = entries.iter().next().expect("one entry");
    let Value::Record(fields) = stored else {
        panic!("the key is not a record")
    };

    assert_eq!(
        fields
            .get(&ply_span::Symbol::new("d"))
            .expect("the decimal field")
            .render(),
        "1.5",
        "the key was not canonicalized, so this test is not exercising the rebuild"
    );
    match fields.get(&ply_span::Symbol::new("p")) {
        Some(Value::Secret(held)) => assert!(
            Arc::ptr_eq(held, &payload),
            "canonicalization rebuilt a credential's payload instead of cloning the `Arc`"
        ),
        other => panic!("the credential stopped being a `Secret`: {other:?}"),
    }
    assert!(
        m.render().contains(SECRET_REDACTED),
        "a canonicalized key holding a credential renders it: {}",
        m.render()
    );
    assert!(
        !m.render().contains("hunter2"),
        "a canonicalized key printed a credential: {}",
        m.render()
    );
}
