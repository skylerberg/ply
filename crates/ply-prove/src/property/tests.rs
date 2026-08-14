use super::*;
use crate::{DEFAULT_SHRINK_BUDGET, MIN_PROPERTY_CASES, Tier};
use ply_core::{CheckOutput, EffectAtom, Resource, RowVar};
use ply_eval::Machine;
use ply_span::SourceId;
use ply_syntax::ast::{Mode, Program};
use ply_syntax::resolve::Resolved;

/// A compiled fixture, so the generator is exercised against the type
/// information the checker really produces rather than against a hand-built
/// approximation of it.
pub(crate) struct Fixture {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Fixture {
    pub(crate) fn compile(src: &str) -> Fixture {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let program = Program::single(module);
        let resolved = ply_syntax::resolve(&program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Fixture {
            program,
            resolved,
            check,
        }
    }

    pub(crate) fn world(&self) -> TypeWorld {
        TypeWorld::new(self.check.ctors.values())
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }
}

pub(crate) fn key(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

pub(crate) fn binder(name: &str, ty: Type) -> LawBinder {
    LawBinder {
        name: Symbol::new(name),
        ty,
        span: Span::DUMMY,
    }
}

fn con(name: &str) -> Type {
    Type::Con(Symbol::new(name), Vec::new())
}

fn draw(ty: &Type, world: &TypeWorld, cases: u32) -> Vec<Value> {
    let mut stream = GenStream::new(0, key(1));
    (0..cases)
        .map(|case| generate(ty, world, &mut stream, case).expect("must generate"))
        .collect()
}

/// Answers from two closures, and records every tuple it was asked about so a
/// test can assert on the *order* of the questions as well as the answers.
pub(crate) struct Fn2<G, B> {
    pub guard: G,
    pub body: B,
    pub asked: Vec<(&'static str, Vec<Value>)>,
}

impl<G, B> Fn2<G, B> {
    pub(crate) fn new(guard: G, body: B) -> Fn2<G, B> {
        Fn2 {
            guard,
            body,
            asked: Vec::new(),
        }
    }
}

impl<G, B> Judge for Fn2<G, B>
where
    G: FnMut(&[Value]) -> Result<bool, Diagnostic>,
    B: FnMut(&[Value]) -> Result<bool, Diagnostic>,
{
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        self.asked.push(("guard", values.to_vec()));
        (self.guard)(values)
    }
    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        self.asked.push(("body", values.to_vec()));
        (self.body)(values)
    }
}

pub(crate) fn ints(values: &[Value]) -> Vec<i64> {
    values
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            other => panic!("expected an Int, got {}", other.render()),
        })
        .collect()
}

const ADTS: &str = r#"
type Color = Red | Green | Blue
type Opt = None | Some(Int)
type Tree = Leaf | Node(Tree, Int, Tree)
type Pair<a> = MkPair(a, a)
type Never = Forever(Never)
type Rose = Rose(Int, List<Rose>)
fn apply1(f: (Int) -> Int, x: Int) -> Int = f(x)
fn apply_str(f: (String) -> Bool, x: String) -> Bool = f(x)
"#;

/// A type and what a value of it must look like.
type Shaped = fn(&Value) -> bool;

// ---------------------------------------------------------------- generation

#[test]
fn every_ply_type_generates() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let record = Type::Record(
        [
            (Symbol::new("balance"), Type::int()),
            (Symbol::new("open"), Type::bool()),
        ]
        .into_iter()
        .collect(),
    );
    let cases: Vec<(Type, Shaped)> = vec![
        (Type::int(), |v| matches!(v, Value::Int(_))),
        (Type::bool(), |v| matches!(v, Value::Bool(_))),
        (Type::string(), |v| matches!(v, Value::Str(_))),
        (Type::bytes(), |v| matches!(v, Value::Bytes(_))),
        (Type::unit(), |v| matches!(v, Value::Unit)),
        (Type::list(Type::int()), |v| matches!(v, Value::List(_))),
        (record, |v| matches!(v, Value::Record(_))),
        (con("Color"), |v| matches!(v, Value::Ctor { .. })),
        (con("Opt"), |v| matches!(v, Value::Ctor { .. })),
        (con("Tree"), |v| matches!(v, Value::Ctor { .. })),
        (Type::Con(Symbol::new("Pair"), vec![Type::bool()]), |v| {
            matches!(v, Value::Ctor { .. })
        }),
        (con("Rose"), |v| matches!(v, Value::Ctor { .. })),
        (
            Type::Fn {
                params: vec![Type::int()],
                ret: Box::new(Type::int()),
                effects: Row::empty(),
            },
            |v| matches!(v, Value::Closure(_)),
        ),
        // A type variable is monomorphised to `Int`, which is what
        // `CaseReport::instantiations` reports.
        (Type::Var(TyVar(0)), |v| matches!(v, Value::Int(_))),
    ];
    for (ty, shaped) in cases {
        for value in draw(&ty, &world, 40) {
            assert!(shaped(&value), "{ty} generated {}", value.render());
        }
    }
}

#[test]
fn a_record_generates_every_field_in_the_types_order() {
    let world = TypeWorld::default();
    let ty = Type::Record(
        [
            (Symbol::new("b"), Type::bool()),
            (Symbol::new("a"), Type::int()),
        ]
        .into_iter()
        .collect(),
    );
    for value in draw(&ty, &world, 20) {
        let Value::Record(fields) = &value else {
            panic!("expected a record");
        };
        assert_eq!(
            fields.keys().map(|k| k.to_string()).collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}

#[test]
fn an_adt_draws_every_constructor() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for value in draw(&con("Color"), &world, 60) {
        let Value::Ctor { name, .. } = &value else {
            panic!("expected a constructor");
        };
        seen.insert(name.to_string());
    }
    assert_eq!(seen.len(), 3, "every variant of `Color` should be drawn");
}

/// The disclosed unsoundness in ADR 0007 §5.1(a) is that the prover reasons over
/// ℤ while `Int` is an `i64`. The sampled tier is the mitigation, and a
/// mitigation that fires "with fixed probability" is one that misses.
#[test]
fn the_integer_boundary_is_drawn_on_every_run() {
    let world = TypeWorld::default();
    let drawn = ints(&draw(&Type::int(), &world, EDGE_CASES));
    for edge in EDGE_INTS {
        assert!(drawn.contains(&edge), "{edge} was never drawn: {drawn:?}");
    }
}

/// `Bytes` draws over the whole byte range and over the whole length range,
/// unlike `String`'s alphabet: a generator that never produces `0x00` or `0xff`
/// checks a law only over the cases that never break.
#[test]
fn bytes_generation_reaches_every_byte_and_the_empty_value() {
    let world = TypeWorld::default();
    let drawn = draw(&Type::bytes(), &world, 400);
    let mut seen_len: BTreeSet<usize> = BTreeSet::new();
    let mut low = false;
    let mut high = false;
    for value in &drawn {
        let Value::Bytes(b) = value else {
            panic!("expected Bytes, got {}", value.render());
        };
        seen_len.insert(b.len());
        low |= b.iter().any(|byte| *byte < 0x10);
        high |= b.iter().any(|byte| *byte > 0xf0);
        assert!(b.len() <= 32, "a drawn Bytes is longer than the bound");
    }
    assert!(seen_len.contains(&0), "`b\"\"` was never drawn");
    assert!(seen_len.len() > 4, "only {} lengths drawn", seen_len.len());
    assert!(low && high, "the byte range was not covered");
}

#[test]
fn a_recursive_type_terminates_and_stays_within_the_depth_bound() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    for ty in [con("Tree"), con("Rose")] {
        let mut deepest = 0;
        for value in draw(&ty, &world, 200) {
            deepest = deepest.max(value_depth(&value));
        }
        assert!(
            deepest > 1,
            "{ty} never nested, so the depth bound is untested"
        );
        assert!(
            deepest <= GEN_DEPTH as usize + 2,
            "{ty} nested {deepest} deep, past the bound"
        );
    }
}

fn value_depth(value: &Value) -> usize {
    match value {
        Value::Ctor { args, .. } => 1 + args.iter().map(value_depth).max().unwrap_or(0),
        Value::List(items) => items.iter().map(value_depth).max().unwrap_or(0),
        Value::Record(fields) => fields.values().map(value_depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[test]
fn a_type_no_finite_value_inhabits_is_not_generatable() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    assert_eq!(
        generatable(&con("Never"), &world),
        Err(Ungeneratable::Uninhabited(Symbol::new("Never")))
    );
}

#[test]
fn the_types_a_binder_may_not_have_are_named() {
    let world = TypeWorld::default();
    let cell = Type::Con(Symbol::new("Cell"), vec![Type::Var(TyVar(0)), Type::int()]);
    let task = Type::Con(Symbol::new(prelude::TASK_TYPE), vec![Type::int()]);
    assert_eq!(generatable(&cell, &world), Err(Ungeneratable::Cell));
    assert_eq!(generatable(&task, &world), Err(Ungeneratable::Task));
    assert_eq!(
        generatable(&Type::list(cell.clone()), &world),
        Err(Ungeneratable::Cell),
        "a type reaching a `Cell` is no more generatable than a `Cell`"
    );

    let atom = EffectAtom::new("db", Resource::Named("users".into()), Mode::Read);
    let effectful = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::singleton(atom.clone()),
    };
    assert_eq!(
        generatable(&effectful, &world),
        Err(Ungeneratable::Effectful(Row::singleton(atom)))
    );

    let polymorphic = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::open(RowVar(3)),
    };
    assert_eq!(
        generatable(&polymorphic, &world),
        Err(Ungeneratable::RowVariable)
    );
    assert_eq!(
        generatable(&con("Nowhere"), &world),
        Err(Ungeneratable::Unknown(Symbol::new("Nowhere")))
    );
}

// ------------------------------------------------------------ reproducibility

#[test]
fn a_root_replays_exactly() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let binders = vec![
        binder("n", Type::int()),
        binder("xs", Type::list(Type::string())),
        binder("t", con("Tree")),
    ];
    let once = draw_cases(&binders, &world, key(9), 41, 50).expect("must generate");
    let again = draw_cases(&binders, &world, key(9), 41, 50).expect("must generate");
    assert_eq!(rendered(&once), rendered(&again));
    assert!(!rendered(&once).is_empty());
}

#[test]
fn another_root_draws_another_run() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let a = draw_cases(&binders, &world, key(9), 41, 60).expect("must generate");
    let b = draw_cases(&binders, &world, key(9), 42, 60).expect("must generate");
    assert_ne!(rendered(&a), rendered(&b));
}

/// Without the obligation in the stream's key, adding a law would shift every
/// later law's cases, so an unrelated edit would change which counterexample a
/// failing obligation reports.
#[test]
fn the_obligation_keys_the_stream() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let a = draw_cases(&binders, &world, key(1), 0, 60).expect("must generate");
    let b = draw_cases(&binders, &world, key(2), 0, 60).expect("must generate");
    assert_ne!(rendered(&a), rendered(&b));
}

#[test]
fn a_draw_is_a_function_of_root_key_and_counter_only() {
    let mut stream = GenStream::new(7, key(3));
    for counter in 0..16 {
        assert_eq!(stream.next_u64(), GenStream::draw(7, &key(3), counter));
    }
    assert_eq!(stream.drawn(), 16);
    assert_ne!(
        GenStream::draw(7, &key(3), 0),
        GenStream::draw(8, &key(3), 0)
    );
    assert_ne!(
        GenStream::draw(7, &key(3), 0),
        GenStream::draw(7, &key(4), 0)
    );
}

pub(crate) fn rendered(cases: &[Vec<Value>]) -> Vec<Vec<String>> {
    cases
        .iter()
        .map(|tuple| tuple.iter().map(|v| v.render()).collect())
        .collect()
}

// ------------------------------------------------------------ function values

/// Every member of the family has to be a function the evaluator can actually
/// apply — twice, to the same answer — or a counterexample naming one is a
/// counterexample naming something that does not exist.
#[test]
fn a_generated_function_is_total_pure_and_deterministic() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let ty = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    let mut applied = 0;
    for f in draw(&ty, &world, 40) {
        for x in [-3i64, 0, 1, 7, i64::MAX] {
            let mut machine = fixture.machine();
            let first = machine
                .call("apply1", vec![f.clone(), Value::Int(x)], Span::DUMMY)
                .unwrap_or_else(|d| panic!("a generated function must be total: {d:?}"));
            let mut machine = fixture.machine();
            let second = machine
                .call("apply1", vec![f.clone(), Value::Int(x)], Span::DUMMY)
                .expect("a generated function must be total");
            assert_eq!(first.render(), second.render());
            assert!(matches!(first, Value::Int(_)));
            applied += 1;
        }
    }
    assert!(applied > 0);
}

#[test]
fn a_generated_function_over_a_compound_argument_applies() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let ty = Type::Fn {
        params: vec![Type::string()],
        ret: Box::new(Type::bool()),
        effects: Row::empty(),
    };
    for f in draw(&ty, &world, 24) {
        for x in ["", "a", "hello"] {
            let mut machine = fixture.machine();
            let answer = machine
                .call("apply_str", vec![f.clone(), Value::str(x)], Span::DUMMY)
                .unwrap_or_else(|d| panic!("a generated function must be total: {d:?}"));
            assert!(matches!(answer, Value::Bool(_)));
        }
    }
}

/// A counterexample naming `<fn>` names nothing a reader can act on.
#[test]
fn a_generated_function_prints_what_it_does() {
    let world = TypeWorld::default();
    let ty = Type::Fn {
        params: vec![Type::int()],
        ret: Box::new(Type::int()),
        effects: Row::empty(),
    };
    for f in draw(&ty, &world, 32) {
        let text = f.render();
        assert!(text.contains("|"), "{text} is not a description");
        assert!(!text.contains("<fn>"), "{text} says nothing");
    }
}

// ---------------------------------------------------------------- the run

fn plan(cases: u32) -> ProvePlan {
    ProvePlan {
        cases,
        roots: vec![0],
        prove_budget: 10,
        shrink_budget: DEFAULT_SHRINK_BUDGET,
        sim: Default::default(),
    }
}

fn run<G, B>(binders: &[LawBinder], world: &TypeWorld, cases: u32, guard: G, body: B) -> Discharge
where
    G: FnMut(&[Value]) -> Result<bool, Diagnostic>,
    B: FnMut(&[Value]) -> Result<bool, Diagnostic>,
{
    let mut judge = Fn2::new(guard, body);
    run_property(
        key(5),
        binders,
        world,
        &plan(cases),
        Span::DUMMY,
        &mut judge,
    )
}

/// The case the milestone exists to not get wrong: a guard nothing satisfies
/// makes `guard ⟹ body` valid, and a system that called that a pass would
/// reward a typo with a green tick.
#[test]
fn a_guard_that_admits_nothing_is_reported_rather_than_passed() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let discharge = run(&binders, &world, 200, |_| Ok(false), |_| Ok(true));
    assert_eq!(discharge.tier(), None, "a vacuity has no tier to report");
    match discharge {
        Discharge::Vacuous(v) => {
            assert_eq!(v.kind, VacuityKind::NoCaseKept { generated: 200 });
        }
        other => panic!("expected a vacuity, got {other:?}"),
    }
}

/// `example` is not a thing a user asks for. It is what the system reports when
/// the guard was tight enough that a coverage claim would be a lie.
#[test]
fn a_tight_guard_reports_example_and_a_loose_one_property() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];

    let tight = run(
        &binders,
        &world,
        200,
        |v| Ok(ints(v)[0] % 97 == 0),
        |_| Ok(true),
    );
    let Discharge::Held(Evidence::Cases(report)) = &tight else {
        panic!("expected a hold, got {tight:?}");
    };
    assert!(
        report.kept > 0 && report.kept < MIN_PROPERTY_CASES,
        "{report:?}"
    );
    assert_eq!(report.rejected, report.generated - report.kept);
    assert_eq!(tight.tier(), Some(Tier::Example));

    let loose = run(&binders, &world, 200, |_| Ok(true), |_| Ok(true));
    let Discharge::Held(Evidence::Cases(report)) = &loose else {
        panic!("expected a hold, got {loose:?}");
    };
    assert_eq!(report.kept, 200);
    assert_eq!(report.rejected, 0);
    assert_eq!(loose.tier(), Some(Tier::Property));
}

/// A body evaluated at a tuple the guard rejects is a claim about a value the
/// obligation never spoke about.
#[test]
fn the_guard_decides_before_the_body_is_ever_evaluated() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let mut judge = Fn2::new(|v: &[Value]| Ok(ints(v)[0] > 0), |_: &[Value]| Ok(true));
    run_property(key(5), &binders, &world, &plan(50), Span::DUMMY, &mut judge);
    for pair in judge.asked.windows(2) {
        if pair[1].0 == "body" {
            assert_eq!(pair[0].0, "guard");
            assert_eq!(rendered_one(&pair[0].1), rendered_one(&pair[1].1));
        }
    }
    let bodies: Vec<_> = judge
        .asked
        .iter()
        .filter(|(kind, _)| *kind == "body")
        .collect();
    assert!(bodies.iter().all(|(_, v)| ints(v)[0] > 0));
}

fn rendered_one(values: &[Value]) -> Vec<String> {
    values.iter().map(|v| v.render()).collect()
}

#[test]
fn a_refutation_names_its_root_its_case_and_what_it_started_from() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let discharge = run(
        &binders,
        &world,
        200,
        |_| Ok(true),
        |v| Ok(ints(v)[0].unsigned_abs() < 1000),
    );
    let Discharge::Refuted(counterexample) = discharge else {
        panic!("expected a refutation");
    };
    assert_eq!(counterexample.root, 0);
    assert_eq!(counterexample.bindings.len(), 1);
    assert_eq!(counterexample.original.len(), 1);
    assert!(
        counterexample.shrinks > 0,
        "the first hit was already minimal, which this fixture rules out"
    );
    assert_eq!(counterexample.bindings[0].name.as_str(), "n");
    assert_eq!(counterexample.bindings[0].ty, Type::int());
}

#[test]
fn a_binder_the_generator_cannot_inhabit_is_a_gap_rather_than_a_verdict() {
    let world = TypeWorld::default();
    let cell = Type::Con(Symbol::new("Cell"), vec![Type::Var(TyVar(0)), Type::int()]);
    let binders = vec![binder("c", cell.clone())];
    let discharge = run(&binders, &world, 200, |_| Ok(true), |_| Ok(false));
    match discharge {
        Discharge::Unattempted(Gap::Ungeneratable { param, ty }) => {
            assert_eq!(param.as_str(), "c");
            assert_eq!(ty, cell);
        }
        other => panic!("expected a gap, got {other:?}"),
    }
}

/// A spec that raises is not false, so it is neither a refutation nor a hold —
/// and the raising input is still worth minimizing.
#[test]
fn a_raising_case_is_a_gap_with_a_shrunk_input() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let boom = |v: &[Value]| {
        if ints(v)[0].unsigned_abs() > 100 {
            Err(Diagnostic::error(
                ply_span::codes::RUNTIME_ERROR,
                "divided by zero",
            ))
        } else {
            Ok(true)
        }
    };
    let discharge = run(&binders, &world, 200, |_| Ok(true), boom);
    match discharge {
        Discharge::Unattempted(Gap::Raised {
            bindings,
            diagnostic,
        }) => {
            assert_eq!(diagnostic.message, "divided by zero");
            let value: i64 = bindings[0].rendered.parse().expect("an Int renders as one");
            assert!(
                value.unsigned_abs() > 100,
                "the shrunk input must still raise"
            );
            assert!(
                value.unsigned_abs() <= 128,
                "{value} is nowhere near minimal for `|n| > 100`"
            );
        }
        other => panic!("expected a raised gap, got {other:?}"),
    }
}

/// The property tier cannot generate a value of an unknown type, so `property`
/// on a polymorphic law is a claim about `Int` and has to say so.
#[test]
fn a_polymorphic_binder_is_monomorphised_and_recorded() {
    let world = TypeWorld::default();
    let binders = vec![
        binder("x", Type::Var(TyVar(4))),
        binder("xs", Type::list(Type::Var(TyVar(4)))),
        binder("y", Type::Var(TyVar(9))),
    ];
    let discharge = run(&binders, &world, 40, |_| Ok(true), |_| Ok(true));
    let Discharge::Held(Evidence::Cases(report)) = discharge else {
        panic!("expected a hold");
    };
    assert_eq!(
        report.instantiations,
        vec![
            (Symbol::new("t4"), Type::int()),
            (Symbol::new("t9"), Type::int()),
        ]
    );
}

#[test]
fn two_runs_over_one_refutation_agree_byte_for_byte() {
    let fixture = Fixture::compile(ADTS);
    let world = fixture.world();
    let binders = vec![
        binder("xs", Type::list(Type::int())),
        binder("t", con("Tree")),
    ];
    let falsify = |v: &[Value]| {
        let Value::List(items) = &v[0] else {
            return Ok(true);
        };
        Ok(items.len() < 2)
    };
    let first = run(&binders, &world, 200, |_| Ok(true), falsify);
    let second = run(&binders, &world, 200, |_| Ok(true), falsify);
    let (Discharge::Refuted(a), Discharge::Refuted(b)) = (&first, &second) else {
        panic!("expected two refutations");
    };
    assert_eq!(a.shrinks, b.shrinks);
    assert_eq!(a.case, b.case);
    assert_eq!(
        a.bindings.iter().map(|x| &x.rendered).collect::<Vec<_>>(),
        b.bindings.iter().map(|x| &x.rendered).collect::<Vec<_>>()
    );
    assert_eq!(
        a.original.iter().map(|x| &x.rendered).collect::<Vec<_>>(),
        b.original.iter().map(|x| &x.rendered).collect::<Vec<_>>()
    );
}

#[test]
fn every_root_in_the_plan_is_drawn_and_reported() {
    let world = TypeWorld::default();
    let binders = vec![binder("n", Type::int())];
    let mut judge = Fn2::new(|_: &[Value]| Ok(true), |_: &[Value]| Ok(true));
    let plan = ProvePlan {
        cases: 30,
        roots: vec![3, 1, 1],
        ..plan(30)
    };
    let discharge = run_property(key(5), &binders, &world, &plan, Span::DUMMY, &mut judge);
    let Discharge::Held(Evidence::Cases(report)) = discharge else {
        panic!("expected a hold");
    };
    assert_eq!(report.roots, vec![1, 3], "roots are normalized before use");
    assert_eq!(report.generated, 60);
    assert_eq!(report.kept, 60);
}
