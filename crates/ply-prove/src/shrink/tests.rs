use super::*;
use crate::property::tests::{Fixture, Fn2, binder, ints, key};
use crate::property::{GenStream, TypeWorld, generate};
use crate::{Counterexample, Discharge, ProvePlan};
use ply_core::{LawBinder, Row};
use ply_span::{Span, Symbol};

const ADTS: &str = r#"
type Opt = None | Some(Int)
type Tree = Leaf | Node(Tree, Int, Tree)
"#;

const BOXES: &str = "type Box<a> = B(a)";

fn con(name: &str) -> Type {
    Type::Con(Symbol::new(name), Vec::new())
}

/// Saturating, because a single `i64::MIN` already saturates [`size`] and a
/// tuple containing one must still be measurable.
fn total_size(values: &[Value], world: &TypeWorld) -> u64 {
    values
        .iter()
        .fold(0u64, |acc, v| acc.saturating_add(size(v, world)))
}

fn plan() -> ProvePlan {
    ProvePlan {
        cases: 200,
        roots: vec![0],
        prove_budget: 10,
        shrink_budget: crate::DEFAULT_SHRINK_BUDGET,
        sim: Default::default(),
    }
}

/// A judge that watches the shrinker rather than only answering it: every tuple
/// the walk accepts is checked against the guard and the property from the
/// outside, so the two requirements are asserted rather than assumed.
struct Watchful<G, B> {
    guard: G,
    body: B,
    accepted: Vec<Vec<Value>>,
}

impl<G, B> Judge for Watchful<G, B>
where
    G: Fn(&[Value]) -> bool,
    B: Fn(&[Value]) -> bool,
{
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        Ok((self.guard)(values))
    }
    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        let held = (self.body)(values);
        if !held && (self.guard)(values) {
            self.accepted.push(values.to_vec());
        }
        Ok(held)
    }
}

fn refute<G, B>(
    binders: &[LawBinder],
    world: &TypeWorld,
    guard: G,
    body: B,
) -> (Counterexample, Vec<Vec<Value>>)
where
    G: Fn(&[Value]) -> bool + Copy,
    B: Fn(&[Value]) -> bool + Copy,
{
    let mut judge = Watchful {
        guard,
        body,
        accepted: Vec::new(),
    };
    let discharge =
        crate::property::run_property(key(5), binders, world, &plan(), Span::DUMMY, &mut judge);
    let Discharge::Refuted(counterexample) = discharge else {
        panic!("expected a refutation, got {discharge:?}");
    };
    (counterexample, judge.accepted)
}

// ------------------------------------------------------------------- the size

#[test]
fn a_negative_outweighs_its_own_magnitude() {
    let world = TypeWorld::default();
    assert_eq!(size(&Value::Int(0), &world), 0);
    assert!(size(&Value::Int(-5), &world) > size(&Value::Int(5), &world));
    assert!(size(&Value::Int(5), &world) > size(&Value::Int(2), &world));
    // Saturating, so the boundary the generator draws on every run does not
    // wrap the measure that terminates the walk.
    assert_eq!(size(&Value::Int(i64::MIN), &world), u64::MAX);
}

#[test]
fn an_empty_collection_is_smaller_than_a_populated_one() {
    let world = TypeWorld::default();
    assert!(
        size(&Value::list(vec![Value::Int(0)]), &world) > size(&Value::list(Vec::new()), &world)
    );
    assert!(size(&Value::str("a"), &world) > size(&Value::str(""), &world));
    assert!(size(&Value::str("b"), &world) > size(&Value::str("a"), &world));
}

#[test]
fn a_lower_constructor_is_smaller_than_a_higher_one() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let none = Value::ctor("None", Vec::new());
    let some = Value::ctor("Some", vec![Value::Int(0)]);
    assert!(size(&some, &world) > size(&none, &world));
}

// ------------------------------------------------------------------- minimal

#[test]
fn the_floor_of_every_type_is_its_smallest_value() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    assert_eq!(minimal(&Type::int(), &world).unwrap().render(), "0");
    assert_eq!(minimal(&Type::bool(), &world).unwrap().render(), "false");
    assert_eq!(minimal(&Type::string(), &world).unwrap().render(), "\"\"");
    assert_eq!(minimal(&Type::unit(), &world).unwrap().render(), "()");
    assert_eq!(
        minimal(&Type::list(Type::int()), &world).unwrap().render(),
        "[]"
    );
    assert_eq!(minimal(&con("Opt"), &world).unwrap().render(), "None");
    assert_eq!(minimal(&con("Tree"), &world).unwrap().render(), "Leaf");
}

/// A recursive type's floor is the constructor that terminates, not the one
/// declared first — otherwise the floor would not exist.
#[test]
fn a_recursive_types_floor_terminates() {
    let fixture = Fixture::compile("type Tree = Node(Tree, Int, Tree) | Leaf");
    let world = fixture.world();
    assert_eq!(minimal(&con("Tree"), &world).unwrap().render(), "Leaf");
}

/// A body, and what the minimal witness against it renders as.
type Property = fn(&[Value]) -> bool;

// ------------------------------------------------------------------ the walk

/// The two requirements, asserted from outside the shrinker: an accepted value
/// still falsifies and still satisfies the guard.
#[test]
fn every_accepted_value_still_falsifies_and_still_satisfies_the_guard() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let guard = |v: &[Value]| ints(v)[0] % 3 == 0;
    let body = |v: &[Value]| ints(v)[0].unsigned_abs() < 60;
    let (counterexample, accepted) = refute(&binders, &world, guard, body);

    let final_value: i64 = counterexample.bindings[0].rendered.parse().unwrap();
    assert_eq!(final_value % 3, 0, "the witness left the guard's domain");
    assert!(
        final_value.unsigned_abs() >= 60,
        "the witness stopped falsifying"
    );
    assert!(!accepted.is_empty());
    for tuple in accepted {
        assert!(guard(&tuple));
        assert!(!body(&tuple));
    }
}

/// A guard-violating candidate is not a smaller counterexample. It is a
/// counterexample to a different claim.
#[test]
fn a_candidate_that_leaves_the_guards_domain_is_rejected() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    // Only odd values are in the domain, so `0`, halving and `n - 1` are all
    // out of it half the time; a shrinker that skipped the guard would land on
    // an even witness.
    let (counterexample, _) = refute(
        &binders,
        &world,
        |v| ints(v)[0] % 2 != 0,
        |v| ints(v)[0].unsigned_abs() < 5,
    );
    let value: i64 = counterexample.bindings[0].rendered.parse().unwrap();
    assert!(value % 2 != 0, "{value} is outside the guard");
    assert!(value.unsigned_abs() >= 5);
    assert!(value.unsigned_abs() <= 7, "{value} is not minimal");
}

/// No monotonicity is assumed: this property holds at every proper sublist of
/// its counterexample, so a shrinker that trusted a smaller value to keep
/// failing would report something that does not fail.
#[test]
fn a_property_that_fails_only_at_a_pair_shrinks_to_a_pair() {
    let world = TypeWorld::default();
    let binders = vec![binder("xs", Type::list(Type::int()))];
    let fails_only_at_two = |v: &[Value]| {
        let Value::List(items) = &v[0] else {
            return true;
        };
        items.len() != 2
    };
    let (counterexample, accepted) = refute(&binders, &world, |_| true, fails_only_at_two);
    assert_eq!(counterexample.bindings[0].rendered, "[0, 0]");
    for tuple in accepted {
        assert!(!fails_only_at_two(&tuple));
    }
}

#[test]
fn shrinking_reaches_a_genuinely_minimal_witness() {
    let world = TypeWorld::default();
    let cases: Vec<(Type, Property, &str)> = vec![
        (Type::int(), |v| ints(v)[0] <= 0, "1"),
        (
            Type::list(Type::int()),
            |v| match &v[0] {
                Value::List(items) => items.is_empty(),
                _ => true,
            },
            "[0]",
        ),
        (
            Type::string(),
            |v| match &v[0] {
                Value::Str(s) => s.is_empty(),
                _ => true,
            },
            "\"a\"",
        ),
        // `false` is the only candidate `Bool` has, so a property that fails
        // only at `true` reports the value it failed at and shrinks no further.
        (Type::bool(), |v| !matches!(v[0], Value::Bool(true)), "true"),
    ];
    for (ty, body, expected) in cases {
        let binders = vec![binder("x", ty.clone())];
        let (counterexample, _) = refute(&binders, &world, |_| true, body);
        assert_eq!(
            counterexample.bindings[0].rendered, expected,
            "{ty} did not reach its minimal witness"
        );
    }
}

#[test]
fn an_adt_shrinks_toward_a_lower_constructor_and_a_recursive_field() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let binders = vec![binder("t", con("Tree"))];
    let (counterexample, _) = refute(&binders, &world, |_| true, |_| false);
    assert_eq!(
        counterexample.bindings[0].rendered, "Leaf",
        "an unconditional falsehood must shrink to the floor of the type"
    );

    // A property that needs a `Node` shrinks to the smallest `Node` rather than
    // past it.
    let needs_a_node =
        |v: &[Value]| !matches!(&v[0], Value::Ctor { name, .. } if name.as_str() == "Node");
    let (counterexample, _) = refute(&binders, &world, |_| true, needs_a_node);
    assert_eq!(counterexample.bindings[0].rendered, "Node(Leaf, 0, Leaf)");
}

#[test]
fn a_record_shrinks_field_by_field() {
    let world = TypeWorld::default();
    let ty = Type::Record(
        [
            (Symbol::new("a"), Type::int()),
            (Symbol::new("b"), Type::list(Type::int())),
        ]
        .into_iter()
        .collect(),
    );
    let binders = vec![binder("r", ty)];
    let (counterexample, _) = refute(&binders, &world, |_| true, |_| false);
    assert_eq!(counterexample.bindings[0].rendered, "{a: 0, b: []}");
}

#[test]
fn a_function_shrinks_toward_the_constant_of_the_minimal_return() {
    let world = TypeWorld::default();
    let ty = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    let binders = vec![binder("f", ty.clone())];
    let (counterexample, _) = refute(&binders, &world, |_| true, |_| false);
    assert_eq!(counterexample.bindings[0].rendered, "<fn |_| 0>");

    // And the floor is a fixed point: shrinking it again proposes itself, whose
    // size is not strictly smaller, so the walk stops.
    let floor = minimal(&ty, &world).unwrap();
    let again = candidates(&floor, &ty, &world);
    assert_eq!(again.len(), 1);
    assert_eq!(size(&again[0], &world), size(&floor, &world));
}

/// Structural, not budgetary: the measure has to fall at every accepted step or
/// the walk is only terminating by luck.
#[test]
fn the_size_measure_strictly_decreases_at_every_accepted_step() {
    let world = TypeWorld::default();
    let types = vec![Type::list(Type::int()), Type::int()];
    let binders = [
        binder("xs", types[0].clone()),
        binder("n", types[1].clone()),
    ];
    let mut stream = GenStream::new(0, key(2));
    let original: Vec<Value> = binders
        .iter()
        .map(|b| generate(&b.ty, &world, &mut stream, 40).unwrap())
        .collect();

    struct Descending<'a> {
        world: &'a TypeWorld,
        last: u64,
    }
    impl Judge for Descending<'_> {
        fn guard(&mut self, _: &[Value]) -> Result<bool, Diagnostic> {
            Ok(true)
        }
        fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
            let total = total_size(values, self.world);
            assert!(
                total < self.last,
                "a candidate at size {total} was offered against {}",
                self.last
            );
            self.last = self.last.min(total);
            Ok(false)
        }
    }

    let mut judge = Descending {
        world: &world,
        last: u64::MAX,
    };
    let shrunk = shrink(
        &original,
        &types,
        &world,
        &mut judge,
        Target::Falsifies,
        u32::MAX,
    );
    assert!(shrunk.steps > 0);
    assert_eq!(
        shrunk.values.iter().map(|v| v.render()).collect::<Vec<_>>(),
        vec!["[]".to_string(), "0".to_string()]
    );
}

/// The budget bounds wall clock. Termination does not depend on it, so an
/// unbounded walk still stops.
#[test]
fn shrinking_terminates_with_an_unbounded_budget() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let types = vec![con("Tree"), Type::string(), Type::list(Type::int())];
    let mut stream = GenStream::new(3, key(6));
    let original: Vec<Value> = types
        .iter()
        .map(|t| generate(t, &world, &mut stream, 60).unwrap())
        .collect();
    let mut judge = Fn2::new(|_: &[Value]| Ok(true), |_: &[Value]| Ok(false));
    let shrunk = shrink(
        &original,
        &types,
        &world,
        &mut judge,
        Target::Falsifies,
        u32::MAX,
    );
    assert_eq!(
        shrunk.values.iter().map(|v| v.render()).collect::<Vec<_>>(),
        vec!["Leaf".to_string(), "\"\"".to_string(), "[]".to_string()]
    );
}

/// `--shrink-budget` can only change how minimal a counterexample is, which is
/// why it is deliberately absent from the cache key.
#[test]
fn a_spent_budget_stops_the_walk_without_breaking_it() {
    let world = TypeWorld::default();
    let types = vec![Type::int()];
    let mut judge = Fn2::new(|_: &[Value]| Ok(true), |_: &[Value]| Ok(false));
    let shrunk = shrink(
        &[Value::Int(1_000_000)],
        &types,
        &world,
        &mut judge,
        Target::Falsifies,
        1,
    );
    assert_eq!(shrunk.evaluations, 1);
    assert_eq!(shrunk.steps, 1);
    assert_eq!(shrunk.values[0].render(), "0");
}

#[test]
fn two_walks_over_one_failure_agree_byte_for_byte() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let types = vec![Type::list(Type::int()), con("Tree")];
    let mut stream = GenStream::new(11, key(8));
    let original: Vec<Value> = types
        .iter()
        .map(|t| generate(t, &world, &mut stream, 30).unwrap())
        .collect();
    let walk = || {
        let mut judge = Fn2::new(
            |v: &[Value]| {
                Ok(match &v[0] {
                    Value::List(items) => items.len() % 2 == 0,
                    _ => true,
                })
            },
            |v: &[Value]| {
                Ok(match &v[0] {
                    Value::List(items) => items.len() < 2,
                    _ => true,
                })
            },
        );
        let shrunk = shrink(
            &original,
            &types,
            &world,
            &mut judge,
            Target::Falsifies,
            crate::DEFAULT_SHRINK_BUDGET,
        );
        (
            shrunk.values.iter().map(|v| v.render()).collect::<Vec<_>>(),
            shrunk.steps,
            shrunk.evaluations,
        )
    };
    assert_eq!(walk(), walk());
}

/// The original is kept because "shrank from a list of 400 to `[0, 1]` in 11
/// steps" is what tells a reader the space was searched; a minimal value alone
/// does not.
#[test]
fn the_original_and_the_step_count_are_both_reported() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let (counterexample, _) = refute(&binders, &world, |_| true, |v| ints(v)[0] < 4);
    assert_eq!(counterexample.bindings[0].rendered, "4");
    assert!(counterexample.shrinks > 0);
    let original: i64 = counterexample.original[0].rendered.parse().unwrap();
    assert!(original >= 4);
    assert_ne!(counterexample.original[0].rendered, "4");
}

/// A first hit that is already minimal reports zero steps, which is exactly the
/// distinction a reader needs the number for.
#[test]
fn an_already_minimal_first_hit_reports_no_steps() {
    let world = TypeWorld::default();
    let types = vec![Type::int()];
    let mut judge = Fn2::new(|_: &[Value]| Ok(true), |_: &[Value]| Ok(false));
    let shrunk = shrink(
        &[Value::Int(0)],
        &types,
        &world,
        &mut judge,
        Target::Falsifies,
        crate::DEFAULT_SHRINK_BUDGET,
    );
    assert_eq!(shrunk.steps, 0);
    assert_eq!(shrunk.evaluations, 0);
}

// ------------------------------------------------- adversarial: the two rules

/// A law that is false only far from zero. Every candidate the walk offers is
/// nearer zero, so a shrinker that assumed a smaller value keeps failing would
/// report an input that falsifies nothing at all.
#[test]
fn a_witness_that_exists_only_far_from_zero_is_never_shrunk_away() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let body = |v: &[Value]| ints(v)[0] < 1_000_000;
    let (counterexample, accepted) = refute(&binders, &world, |_| true, body);

    let value: i64 = counterexample.bindings[0].rendered.parse().unwrap();
    assert!(value >= 1_000_000, "{value} does not falsify anything");
    assert!(counterexample.shrinks > 0, "the walk never moved");
    for tuple in accepted {
        assert!(!body(&tuple));
    }
}

/// A guard that couples two binders, against a walk that moves one at a time:
/// lowering `lo` past `hi` leaves the domain the law spoke about, and the only
/// thing stopping that is the guard being asked again about every candidate.
#[test]
fn a_guard_coupling_two_binders_holds_at_every_accepted_step() {
    let world = TypeWorld::default();
    let binders = vec![binder("lo", Type::int()), binder("hi", Type::int())];
    let guard = |v: &[Value]| {
        let n = ints(v);
        n[0] < n[1]
    };
    let body = |v: &[Value]| {
        let n = ints(v);
        n[1].saturating_sub(n[0]) < 8
    };
    let (counterexample, accepted) = refute(&binders, &world, guard, body);

    let lo: i64 = counterexample.bindings[0].rendered.parse().unwrap();
    let hi: i64 = counterexample.bindings[1].rendered.parse().unwrap();
    assert!(lo < hi, "{lo} < {hi} is the domain the law spoke about");
    assert!(hi.saturating_sub(lo) >= 8, "the witness stopped falsifying");
    assert!(!accepted.is_empty());
    for tuple in accepted {
        assert!(guard(&tuple) && !body(&tuple));
    }
}

fn left_is_a_node(value: &Value) -> bool {
    let Value::Ctor { name, args } = value else {
        return false;
    };
    name.as_str().ends_with("Node")
        && matches!(args.first(), Some(Value::Ctor { name, .. }) if name.as_str().ends_with("Node"))
}

/// The recursive-field candidate replaces a `Node` with one of its subtrees,
/// which is the step that collapses a deep tree in one move — and the step that
/// would destroy a witness that needs the nesting.
#[test]
fn a_nested_adt_witness_survives_the_recursive_field_candidate() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let binders = vec![binder("t", con("Tree"))];
    let body = |v: &[Value]| !left_is_a_node(&v[0]);
    let (counterexample, accepted) = refute(&binders, &world, |_| true, body);

    assert_eq!(
        counterexample.bindings[0].rendered, "Node(Node(Leaf, 0, Leaf), 0, Leaf)",
        "the smallest tree whose left child is a `Node`"
    );
    assert!(!accepted.is_empty());
    for tuple in accepted {
        assert!(left_is_a_node(&tuple[0]));
    }
}

/// A constructor's field types are written in the owning type's parameters, so
/// the walk has to substitute the `Type::Con`'s arguments before it can shrink a
/// field. Without that, `B([9, 9])` shrinks its payload as if it were an `Int`
/// and produces nothing.
#[test]
fn a_polymorphic_adt_shrinks_through_its_substituted_field_type() {
    let fixture = Fixture::compile(BOXES);
    let world = fixture.world();
    let ty = Type::Con(Symbol::new("Box"), vec![Type::list(Type::int())]);
    let binders = vec![binder("b", ty)];
    let has_a_payload = |v: &[Value]| match &v[0] {
        Value::Ctor { args, .. } => match args.first() {
            Some(Value::List(items)) => items.is_empty(),
            _ => true,
        },
        _ => true,
    };
    let (counterexample, _) = refute(&binders, &world, |_| true, has_a_payload);
    assert_eq!(counterexample.bindings[0].rendered, "B([0])");
}

/// A long list is where a shrinker that only removes one element at a time takes
/// forever and a shrinker that halves blindly overshoots. The halves are offered
/// first, and the witness — a length nothing below 100 has — is kept.
#[test]
fn a_long_list_shrinks_to_the_shortest_length_the_witness_needs() {
    let world = TypeWorld::default();
    let types = vec![Type::list(Type::int())];
    let original = Value::list(vec![Value::Int(0); 400]);
    let mut judge = Fn2::new(
        |_: &[Value]| Ok(true),
        |v: &[Value]| {
            Ok(match &v[0] {
                Value::List(items) => items.len() < 100,
                _ => true,
            })
        },
    );
    let shrunk = shrink(
        std::slice::from_ref(&original),
        &types,
        &world,
        &mut judge,
        Target::Falsifies,
        u32::MAX,
    );
    let Value::List(items) = &shrunk.values[0] else {
        panic!("a list must shrink to a list");
    };
    assert_eq!(items.len(), 100);
    assert!(shrunk.steps > 0);
    assert!(size(&shrunk.values[0], &world) < size(&original, &world));
}

/// `i64::MIN` is drawn on every property run, and its [`size`] saturates. The
/// walk must neither loop on a candidate whose size ties nor accept one that
/// stops raising the witness's own flag.
#[test]
fn the_integer_boundary_neither_loops_nor_loses_its_witness() {
    let world = TypeWorld::default();
    let types = vec![Type::int()];
    let mut judge = Fn2::new(
        |_: &[Value]| Ok(true),
        |v: &[Value]| Ok(ints(v)[0] != i64::MIN),
    );
    let shrunk = shrink(
        &[Value::Int(i64::MIN)],
        &types,
        &world,
        &mut judge,
        Target::Falsifies,
        u32::MAX,
    );
    assert_eq!(shrunk.values[0].render(), i64::MIN.to_string());
    assert_eq!(shrunk.steps, 0);
    assert!(
        shrunk.evaluations > 0,
        "candidates were offered and refused"
    );

    // `i64::MIN + 1` ties the saturated measure rather than falling below it, so
    // the walk refuses it without evaluating — which is the only reason a
    // measure that cannot separate the two ends of the range still terminates.
    let floor = size(&Value::Int(i64::MIN), &world);
    assert_eq!(floor, u64::MAX);
    assert_eq!(size(&Value::Int(i64::MIN + 1), &world), floor);
    assert!(
        candidates(&Value::Int(i64::MIN), &Type::int(), &world)
            .iter()
            .any(|c| matches!(c, Value::Int(n) if *n == i64::MIN + 1)),
        "the tying candidate is offered, and the size test is what refuses it"
    );
}

/// A raising input is shrunk with "still raises" as the predicate, so every
/// accepted candidate has to raise — not merely the last one.
#[test]
fn every_accepted_raising_candidate_still_raises() {
    struct Watch {
        raised_at: Vec<i64>,
    }
    impl Judge for Watch {
        fn guard(&mut self, _: &[Value]) -> Result<bool, Diagnostic> {
            Ok(true)
        }
        fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
            let n = ints(values)[0];
            if n.unsigned_abs() > 100 {
                self.raised_at.push(n);
                return Err(Diagnostic::error(
                    ply_span::codes::RUNTIME_ERROR,
                    "divided by zero",
                ));
            }
            Ok(true)
        }
    }

    let world = TypeWorld::default();
    let types = vec![Type::int()];
    let mut judge = Watch {
        raised_at: Vec::new(),
    };
    let shrunk = shrink(
        &[Value::Int(1_000_000)],
        &types,
        &world,
        &mut judge,
        Target::Raises,
        crate::DEFAULT_SHRINK_BUDGET,
    );
    let value: i64 = shrunk.values[0].render().parse().unwrap();
    assert!(value.unsigned_abs() > 100, "{value} stopped raising");
    assert!(shrunk.diagnostic.is_some(), "the raise has to be carried");
    assert!(judge.raised_at.contains(&value));
}

/// The budget bounds wall clock and nothing else: a walk cut short still reports
/// a value that falsifies and that the guard admits.
#[test]
fn every_shrink_budget_reports_a_legal_witness() {
    let world = TypeWorld::default();
    let types = vec![Type::int()];
    for budget in [0u32, 1, 3, 17, crate::DEFAULT_SHRINK_BUDGET] {
        let mut judge = Fn2::new(
            |v: &[Value]| Ok(ints(v)[0] % 7 == 0),
            |v: &[Value]| Ok(ints(v)[0].unsigned_abs() < 70),
        );
        let shrunk = shrink(
            &[Value::Int(7 * 99_991)],
            &types,
            &world,
            &mut judge,
            Target::Falsifies,
            budget,
        );
        let value: i64 = shrunk.values[0].render().parse().unwrap();
        assert_eq!(value % 7, 0, "budget {budget} left the guard's domain");
        assert!(
            value.unsigned_abs() >= 70,
            "budget {budget} stopped falsifying at {value}"
        );
        assert!(shrunk.evaluations <= budget);
    }
}

/// The only candidate a function value has is the constant of its minimal
/// return, so a witness that needs a function telling two inputs apart must
/// survive that candidate being refused — and the other binders must still
/// shrink around it.
#[test]
fn a_witness_needing_a_non_constant_function_keeps_it() {
    let world = TypeWorld::default();
    let ty = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    let binders = vec![binder("f", ty), binder("n", Type::int())];
    // Holds for every function that answers the same thing everywhere, which is
    // exactly the constant the shrinker walks toward — so the one candidate `f`
    // has is refused at every step.
    let constant = |v: &[Value]| !v[0].render().contains("if ");
    let (counterexample, accepted) = refute(&binders, &world, |_| true, constant);

    assert_eq!(
        counterexample.bindings[1].rendered, "0",
        "the binder that can shrink still does"
    );
    let rendered = &counterexample.bindings[0].rendered;
    assert!(
        rendered.contains("if "),
        "{rendered} is a constant function"
    );
    assert!(!accepted.is_empty());
    for tuple in accepted {
        assert!(!constant(&tuple));
    }
}

/// The walk reads its types from the obligation's binders, and a value that does
/// not match one is a defect elsewhere. It has to come back with no candidates
/// rather than with a candidate of the wrong shape.
#[test]
fn a_value_the_type_does_not_describe_offers_nothing() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let tree = Value::ctor(
        "Node",
        vec![
            Value::ctor("Leaf", vec![]),
            Value::Int(1),
            Value::ctor("Leaf", vec![]),
        ],
    );
    assert!(candidates(&tree, &Type::int(), &world).is_empty());
    assert!(candidates(&tree, &Type::bool(), &world).is_empty());
    assert!(candidates(&Value::Unit, &con("Tree"), &world).is_empty());
    assert!(
        candidates(&Value::ctor("Nowhere", vec![]), &con("Tree"), &world).is_empty(),
        "a constructor the type does not declare is not a member of it"
    );
}

#[test]
fn a_candidate_order_is_fixed() {
    let world = TypeWorld::default();
    let rendered = |v: &Value, t: &Type| {
        candidates(v, t, &world)
            .iter()
            .map(|c| c.render())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rendered(&Value::Int(9), &Type::int()),
        vec!["0", "4", "2", "1", "8"]
    );
    assert_eq!(
        rendered(&Value::Int(-4), &Type::int()),
        vec!["0", "-2", "-1", "-3", "4"]
    );
    assert_eq!(
        rendered(
            &Value::list(vec![Value::Int(1), Value::Int(2)]),
            &Type::list(Type::int())
        ),
        vec![
            "[]", "[1]", "[2]", "[2]", "[1]", "[0, 2]", "[1, 0]", "[1, 1]"
        ]
    );
    assert_eq!(
        rendered(&Value::str("bc"), &Type::string())[..3],
        ["\"\"".to_string(), "\"b\"".to_string(), "\"c\"".to_string()]
    );
}
