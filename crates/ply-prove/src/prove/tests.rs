//! Every rule gets a true instance that is proved and a false instance that is
//! **not**. The second matters more: reach is a nice-to-have and a wrong
//! `proved` is the one defect this milestone cannot ship.

use super::*;
use crate::Rule;
use ply_core::{CheckOutput, LawBinder, TyVar, Type};
use ply_span::{SourceId, Span, Symbol};
use ply_syntax::ast::{Expr, Item, LawDef, Program, TypeExpr};
use ply_syntax::resolve::Resolved;
use std::collections::BTreeMap;

const SRC: SourceId = SourceId(0);

struct Fixture {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn fixture(source: &str) -> Fixture {
    let module = match ply_syntax::parse(SRC, source) {
        Ok(module) => module,
        Err(diagnostics) => panic!("parse: {:?}", messages(&diagnostics)),
    };
    let program = Program::single(module);
    let resolved = match ply_syntax::resolve(&program) {
        Ok(resolved) => resolved,
        Err(diagnostics) => panic!("resolve: {:?}", messages(&diagnostics)),
    };
    let check = match ply_core::check_program(&program, &resolved) {
        Ok(check) => check,
        Err(diagnostics) => panic!("check: {:?}", messages(&diagnostics)),
    };
    Fixture {
        program,
        resolved,
        check,
    }
}

fn messages(diagnostics: &[ply_span::Diagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| d.message.clone()).collect()
}

impl Fixture {
    fn context(&self) -> Context<'_> {
        Context::new(&self.program, &self.resolved, &self.check)
    }

    fn law(&self, label: &str) -> &LawDef {
        self.program.modules[0]
            .items
            .iter()
            .find_map(|item| match item {
                Item::Law(def) if def.name == label => Some(&**def),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no law labelled `{label}`"))
    }
}

/// A law binder's declared type, resolved the way `ply-core` will resolve it.
/// The prover is written against `Type`, not against the surface syntax, so a
/// small converter here keeps these tests independent of the checker's own
/// spec support landing.
fn resolve_type(ty: &TypeExpr, vars: &mut BTreeMap<Symbol, TyVar>) -> Type {
    match ty {
        TypeExpr::Var(name) => {
            let next = vars.len() as u32;
            Type::Var(*vars.entry(name.name.clone()).or_insert(TyVar(next)))
        }
        TypeExpr::Con { name, args, .. } => Type::Con(
            name.symbol().clone(),
            args.iter().map(|a| resolve_type(a, vars)).collect(),
        ),
        TypeExpr::Fn { params, ret, .. } => Type::Fn {
            params: params.iter().map(|p| resolve_type(p, vars)).collect(),
            ret: Box::new(resolve_type(ret, vars)),
            effects: ply_core::Row::empty(),
        },
        TypeExpr::Record { fields, .. } => Type::Record(
            fields
                .iter()
                .map(|(n, t)| (n.name.clone(), resolve_type(t, vars)))
                .collect(),
        ),
        TypeExpr::Unit { .. } => Type::unit(),
    }
}

fn binders(law: &LawDef) -> Vec<LawBinder> {
    let mut vars = BTreeMap::new();
    law.binders
        .iter()
        .map(|b| LawBinder {
            name: b.name.name.clone(),
            ty: resolve_type(&b.ty, &mut vars),
            span: b.span,
        })
        .collect()
}

fn attempt(fixture: &Fixture, label: &str) -> Decision {
    attempt_with(fixture, label, &Limits::default())
}

fn attempt_with(fixture: &Fixture, label: &str, limits: &Limits) -> Decision {
    let ctx = fixture.context();
    let law = fixture.law(label);
    let binders = binders(law);
    let guards: Vec<&Expr> = law.guard.iter().collect();
    decide(
        &ctx,
        &Goal {
            module: 0,
            binders: &binders,
            guards: &guards,
            result: None,
            body: &law.body,
        },
        limits,
    )
}

#[track_caller]
fn proof(fixture: &Fixture, label: &str) -> Proof {
    match attempt(fixture, label) {
        Decision::Proved(proof) => proof,
        other => panic!("`{label}` was expected to be proved, got {other:?}"),
    }
}

#[track_caller]
fn not_proved(fixture: &Fixture, label: &str) {
    if let Decision::Proved(proof) = attempt(fixture, label) {
        panic!("`{label}` must not be proved, but got a certificate: {proof:?}");
    }
}

// ---------------------------------------------------------------- arithmetic

const ARITHMETIC: &str = r#"
law "a successor is larger" forall (x: Int) { x + 1 > x }
law "a bounded successor is larger" forall (x: Int) where x < 100 { x + 1 > x }
law "a successor is smaller" forall (x: Int) { x + 1 > x + 2 }
law "positives sum past one" forall (x: Int, y: Int) where x > 0 && y > 0 { x + y > 1 }
law "bounded positives sum past one" forall (x: Int, y: Int)
  where x > 0 && y > 0 && x < 1000 && y < 1000 { x + y > 1 }
law "positives sum past two" forall (x: Int, y: Int) where x > 0 && y > 0 { x + y > 2 }
law "bounded positives sum past two" forall (x: Int, y: Int)
  where x > 0 && y > 0 && x < 1000 && y < 1000 { x + y > 2 }
law "doubling is adding" forall (x: Int) { 2 * x == x + x }
law "bounded doubling is adding" forall (x: Int) where x > -1000 && x < 1000
  { 2 * x == x + x }
law "multiplication commutes" forall (x: Int, y: Int) { x * y == y * x }
law "subtraction inverts" forall (x: Int, y: Int) { x + y - y == x }
law "bounded subtraction inverts" forall (x: Int, y: Int)
  where x > -1000 && x < 1000 && y > -1000 && y < 1000 { x + y - y == x }
law "ordering is transitive" forall (x: Int, y: Int, z: Int) where x < y && y < z { x < z }
law "ordering skips one" forall (x: Int, y: Int, z: Int) where x < y && y < z { x < z - 1 }
law "ordering skips two" forall (x: Int, y: Int, z: Int) where x < y && y < z { x < z - 2 }
law "a strict bound is a tight bound" forall (x: Int, y: Int) where x < y { x + 1 <= y }
law "a strict bound is tighter still" forall (x: Int, y: Int) where x < y { x + 2 <= y }
law "an integer between" forall (x: Int) where x > 0 && x < 1 { x == 100 }
law "a domain of one point" forall (x: Int) where x > 0 && x < 2 { x == 1 }
law "a domain of one wrong point" forall (x: Int) where x > 0 && x < 2 { x == 2 }
law "halves are wholes" forall (x: Int) { x / 2 * 2 == x }
law "dividing by one" forall (x: Int) { x / 1 == x }
law "division is a function" forall (x: Int) { x / 2 == x / 2 }
law "remainder is a function" forall (x: Int, y: Int) where x == y { x % 3 == y % 3 }
law "a symbolic quotient is a function" forall (x: Int, y: Int) { x / y == x / y }
law "a nonzero quotient is a function" forall (x: Int, y: Int) where y > 0
  { x / y == x / y }
law "an int is not below the smallest" forall (x: Int)
  { x >= -9223372036854775807 - 1 }
law "dividing by minus one is a function" forall (x: Int) { x / -1 == x / -1 }
law "dividing a bounded value by minus one" forall (x: Int) where x > -100
  { x / -1 == x / -1 }
law "remainder by zero is a function" forall (x: Int) { x % 0 == x % 0 }
"#;

#[test]
fn linear_arithmetic_decides_both_directions() {
    let f = fixture(ARITHMETIC);
    let proved = proof(&f, "bounded positives sum past one");
    assert!(proved.rules.contains(&Rule::LinearArithmetic));
    not_proved(&f, "bounded positives sum past two");

    proof(&f, "bounded doubling is adding");
    proof(&f, "bounded subtraction inverts");
    proof(&f, "ordering is transitive");
    not_proved(&f, "ordering skips two");
}

/// Every `Int` is an `i64`, so its own width is a theorem rather than an
/// assumption — which is what lets a guard establish that an operator's result
/// fits and is the reason `x + 1` under `x < 100` is decided at all.
#[test]
fn an_int_is_bounded_by_its_own_width() {
    let f = fixture(ARITHMETIC);
    proof(&f, "an int is not below the smallest");
}

/// The claim is valid over ℤ and **raises** at `i64::MAX`, so there is no input
/// at which it holds and the prover may not report one covering every input.
/// ADR 0007 §5.1(a) disclosed this as a live unsoundness and named a mitigation
/// that could not fire; the fix is that a proof now carries the definedness of
/// every arithmetic term it reasoned about, and the bounded restatement beside
/// each entry is what says the reach was not simply thrown away.
#[test]
fn arithmetic_that_can_leave_int_is_not_proved() {
    let f = fixture(ARITHMETIC);
    for (unbounded, bounded) in [
        ("a successor is larger", "a bounded successor is larger"),
        ("positives sum past one", "bounded positives sum past one"),
        ("doubling is adding", "bounded doubling is adding"),
        ("subtraction inverts", "bounded subtraction inverts"),
    ] {
        not_proved(&f, unbounded);
        proof(&f, bounded);
    }
}

/// A zero divisor raises, so an uninterpreted `/` is a value only where the
/// guard says the divisor is not zero. Congruence over it is unchanged; what
/// changed is that it has to be reached.
#[test]
fn a_quotient_is_a_value_only_where_its_divisor_is_not_zero() {
    let f = fixture(ARITHMETIC);
    not_proved(&f, "a symbolic quotient is a function");
    proof(&f, "a nonzero quotient is a function");

    // A literal divisor is decided outright, in both directions: `0` never has
    // an answer, and `-1` has one everywhere except `i64::MIN`.
    not_proved(&f, "remainder by zero is a function");
    not_proved(&f, "dividing by minus one is a function");
    proof(&f, "dividing a bounded value by minus one");
}

/// `x < y` over `Int` is `x + 1 <= y`, and reasoning with that is what decides
/// `x < y && y < z ⟹ x < z - 1` — true over ℤ and not over ℚ. The rule has to
/// stop exactly one step further out, which the second claim pins.
#[test]
fn strict_inequalities_are_tightened_to_the_integers() {
    let f = fixture(ARITHMETIC);
    proof(&f, "a strict bound is a tight bound");
    not_proved(&f, "a strict bound is tighter still");
    proof(&f, "ordering skips one");
}

/// `x * y` with both factors symbolic is uninterpreted, so commutativity — true
/// of every actual `Int` — is not in the fragment and must not be proved.
#[test]
fn multiplication_by_a_symbolic_is_not_arithmetic() {
    let f = fixture(ARITHMETIC);
    not_proved(&f, "multiplication commutes");
}

/// Division is outside the fragment as a *value*, at all, including by a
/// literal. The cost is `x / 1 == x`; the benefit is that a wrong division rule
/// cannot exist.
#[test]
fn division_is_uninterpreted_in_both_directions() {
    let f = fixture(ARITHMETIC);
    not_proved(&f, "halves are wholes");
    not_proved(&f, "dividing by one");
    // Uninterpreted is not unknown: congruence still applies to it.
    proof(&f, "division is a function");
    proof(&f, "remainder is a function");
}

/// The guard admits nothing, so the obligation is trivially valid and says
/// nothing. Reporting it `proved` would turn a typo in a guard into a proof of
/// everything.
#[test]
fn an_unsatisfiable_guard_is_vacuous_and_never_proved() {
    let f = fixture(ARITHMETIC);
    assert!(matches!(
        attempt(&f, "an integer between"),
        Decision::GuardUnsatisfiable { .. }
    ));
}

/// The false instance of the vacuity rule, and the one that matters more:
/// `x > 0 && x < 2` admits exactly one integer, so a claim under it is a real
/// claim about a real domain and reporting it vacuous would be an error raised
/// against a correct spec. The domain being one point wide is deliberate — it is
/// the narrowest guard the tightening still has to call satisfiable.
#[test]
fn a_satisfiable_guard_is_never_vacuous_however_narrow() {
    let f = fixture(ARITHMETIC);
    for label in ["a domain of one point", "a domain of one wrong point"] {
        assert!(
            !matches!(attempt(&f, label), Decision::GuardUnsatisfiable { .. }),
            "`{label}` has a domain of exactly one integer"
        );
    }
    proof(&f, "a domain of one point");
    not_proved(&f, "a domain of one wrong point");
}

// ------------------------------------------------------------------ overflow

const BOUNDARIES: &str = r#"
law "adding one grows" forall (x: Int) { x + 1 > x }
law "the largest int" { 9223372036854775807 + 1 > 9223372036854775807 }
law "a round trip past the boundary" forall (x: Int)
  { x + 9223372036854775807 - 9223372036854775807 == x }
law "a bounded round trip" forall (x: Int) where x > -1000 && x < 1000
  { x + 1000 - 1000 == x }
law "scaling past the boundary" forall (x: Int)
  { x * 9223372036854775807 * 9223372036854775807 * 9223372036854775807 * 3 ==
    x * 9223372036854775807 * 9223372036854775807 * 9223372036854775807 * 3 }
law "the smallest int is smallest" forall (x: Int) { x >= -9223372036854775807 }
law "an overflowing sum is an overflowing product"
  { 9223372036854775807 + 9223372036854775807 ==
    9223372036854775807 * 9223372036854775807 }
law "an overflowing difference is an overflowing product"
  { -9223372036854775807 - 9223372036854775807 ==
    -9223372036854775807 * 9223372036854775807 }
law "an overflowing sum is itself" forall (x: Int)
  where x == 9223372036854775807 + 9223372036854775807
  { x == 9223372036854775807 + 9223372036854775807 }
"#;

/// Every operator whose result left `Int` becomes an uninterpreted term, and
/// **the operator has to be part of the term's identity.** A sum and a product
/// that both overflowed are not the same value, so an encoding that gave them
/// one symbol would prove `MAX + MAX == MAX * MAX` — a wrong `proved` built out
/// of two correct refusals to fold.
#[test]
fn two_operators_that_overflowed_are_not_one_term() {
    let f = fixture(BOUNDARIES);
    not_proved(&f, "an overflowing sum is an overflowing product");
    not_proved(&f, "an overflowing difference is an overflowing product");
    // Nor is either of them a value at all: `MAX + MAX` raises, so the
    // reflexivity that congruence would happily supply is a claim about
    // something the program never computes.
    not_proved(&f, "an overflowing sum is itself");
}

/// ADR 0007 §5.1(a) reasons over ℤ. That is a statement about **Ply** only
/// where ℤ and `i64` agree, so a proof carries the definedness of every
/// arithmetic term it used and a law true over ℤ and raising at `i64::MAX` is
/// not proved.
///
/// The mitigation the ADR named — the generator drawing the boundaries — could
/// never have caught this: `Prover::discharge_with` answers from the static
/// tier before any case is drawn, and Ply's arithmetic is checked, so the
/// divergence surfaces as a raise rather than as a refutation. Reach is
/// recovered by a guard rather than by a disclosure.
#[test]
fn arithmetic_is_proved_only_where_the_result_is_an_int() {
    let f = fixture(BOUNDARIES);
    not_proved(&f, "adding one grows");
    not_proved(&f, "a round trip past the boundary");
    proof(&f, "a bounded round trip");
}

/// What must never happen at the boundary is the prover's *own* arithmetic
/// wrapping. A constant no `Int` holds has no term to fold to, so the term is
/// opaque and nothing is claimed about it.
#[test]
fn a_constant_outside_int_is_opaque_rather_than_wrapped() {
    let f = fixture(BOUNDARIES);
    not_proved(&f, "the largest int");
    not_proved(&f, "the smallest int is smallest");
}

/// A coefficient that leaves `i128` makes the term uninterpreted — and the
/// product it stands for is one no `Int` holds, so it is not a value either and
/// congruence over it decides nothing.
#[test]
fn a_coefficient_that_overflows_stays_a_term() {
    let f = fixture(BOUNDARIES);
    not_proved(&f, "scaling past the boundary");
}

// -------------------------------------------------------------- propositional

const PROPOSITIONAL: &str = r#"
law "excluded middle" forall (b: Bool) { b || !b }
law "a bare disjunction" forall (b: Bool, c: Bool) { b || c }
law "disjunction commutes" forall (b: Bool, c: Bool) where b || c { c || b }
law "de morgan" forall (b: Bool, c: Bool) { !(b && c) == (!b || !c) }
law "de morgan mangled" forall (b: Bool, c: Bool) { !(b && c) == (!b && !c) }
law "a conditional picks a branch" forall (b: Bool, x: Int)
  { if b { x } else { x } == x }
law "a conditional is not constant" forall (b: Bool, x: Int, y: Int)
  { if b { x } else { y } == x }
"#;

#[test]
fn propositional_structure_decides_both_directions() {
    let f = fixture(PROPOSITIONAL);
    let proved = proof(&f, "excluded middle");
    assert!(proved.rules.contains(&Rule::Propositional));
    not_proved(&f, "a bare disjunction");

    proof(&f, "disjunction commutes");
    proof(&f, "de morgan");
    not_proved(&f, "de morgan mangled");

    proof(&f, "a conditional picks a branch");
    not_proved(&f, "a conditional is not constant");
}

// ------------------------------------------------------ congruence and sorts

const CONGRUENCE: &str = r#"
law "a function is a function" forall (f: (Int) -> Int, x: Int) { f(x) == f(x) }
law "equal arguments agree" forall (f: (Int) -> Int, x: Int, y: Int) where x == y
  { f(x) == f(y) }
law "any two arguments agree" forall (f: (Int) -> Int, x: Int, y: Int)
  { f(x) == f(y) }
law "congruence is polymorphic" forall (g: (a) -> b, u: a, v: a) where u == v
  { g(u) == g(v) }
law "a record is its fields" forall (n: Int) { { balance: n }.balance == n }
law "records with equal fields are equal" forall (n: Int, m: Int) where n == m
  { { balance: n } == { balance: m } }
law "records with any fields are equal" forall (n: Int, m: Int)
  { { balance: n } == { balance: m } }

fn moved(account: { name: String, balance: Int }, amount: Int)
  -> { name: String, balance: Int }
= { name: account.name, balance: account.balance + amount }

law "a record rebuilt from its own fields is the record"
  forall (a: { name: String, balance: Int }) {
    { name: a.name, balance: a.balance } == a
  }
law "moving an account and moving it back is the account"
  forall (a: { name: String, balance: Int }, n: Int)
  where n > -1000 && n < 1000 && a.balance > -1000 && a.balance < 1000 {
    moved(moved(a, n), 0 - n) == a
  }
law "moving an unbounded account and back is the account"
  forall (a: { name: String, balance: Int }, n: Int) {
    moved(moved(a, n), 0 - n) == a
  }
law "moving an account leaves it where it was"
  forall (a: { name: String, balance: Int }, n: Int) {
    moved(a, n) == a
  }
law "one matching field is enough"
  forall (a: { name: String, balance: Int }, b: { name: String, balance: Int })
  where a.name == b.name {
    a == b
  }
"#;

#[test]
fn congruence_over_an_uninterpreted_function_decides_both_directions() {
    let f = fixture(CONGRUENCE);
    let proved = proof(&f, "a function is a function");
    assert!(proved.rules.contains(&Rule::Congruence));
    proof(&f, "equal arguments agree");
    not_proved(&f, "any two arguments agree");
}

/// A proof over an uninterpreted sort is a proof for every instantiation, and
/// the certificate says which variables stayed uninterpreted.
#[test]
fn a_polymorphic_proof_records_its_sorts() {
    let f = fixture(CONGRUENCE);
    let proved = proof(&f, "congruence is polymorphic");
    assert_eq!(proved.sorts.len(), 2, "{:?}", proved.sorts);
}

#[test]
fn records_project_and_compare_structurally() {
    let f = fixture(CONGRUENCE);
    proof(&f, "a record is its fields");
    proof(&f, "records with equal fields are equal");
    not_proved(&f, "records with any fields are equal");
}

/// Extensionality in the *introduction* direction: a record literal is proved
/// equal to an opaque record when every field is. Without it, an `ensures` that
/// rebuilds a record — the commonest shape a postcondition has — could never be
/// proved, because one side is a literal and the other is a symbolic constant.
///
/// The two negative instances are the ones that matter. A rule that concluded
/// from *some* fields agreeing, or that assumed an uncompared field, would prove
/// two different accounts equal.
#[test]
fn a_record_equals_one_rebuilt_from_all_of_its_fields_and_no_fewer() {
    let f = fixture(CONGRUENCE);
    let proved = proof(&f, "a record rebuilt from its own fields is the record");
    assert!(proved.rules.contains(&Rule::Congruence));
    proof(&f, "moving an account and moving it back is the account");

    not_proved(&f, "moving an account leaves it where it was");
    not_proved(&f, "one matching field is enough");
    // The same claim without the guard moves a balance out of `Int`, so the
    // record equality it rests on is an equality between values one side of
    // which the program never produces.
    not_proved(&f, "moving an unbounded account and back is the account");
}

// -------------------------------------------------- constructors and splitting

const SHAPES: &str = r#"
type Shape = Circle(Int) | Square(Int)

law "constructors are injective" forall (a: Int, b: Int) where Circle(a) == Circle(b)
  { a == b }
law "injective by one" forall (a: Int, b: Int) where Circle(a) == Circle(b)
  { a == b + 1 }
law "constructors are distinct" forall (a: Int, b: Int) where Circle(a) == Square(b)
  { a == b + 1 }
law "a rebuilt value is the same" forall (a: Int) { Circle(a) == Circle(a) }
law "two constructors are not" forall (a: Int) { Circle(a) == Square(a) }
"#;

#[test]
fn constructor_injectivity_decides_both_directions() {
    let f = fixture(SHAPES);
    let proved = proof(&f, "constructors are injective");
    assert!(proved.rules.contains(&Rule::Injectivity));
    not_proved(&f, "injective by one");
    proof(&f, "a rebuilt value is the same");
    not_proved(&f, "two constructors are not");
}

/// Two distinct constructors cannot be equal, so the guard admits nothing. That
/// is a vacuity and an error, **not** a proof of the body beside it.
#[test]
fn distinct_constructors_make_a_guard_vacuous() {
    let f = fixture(SHAPES);
    assert!(matches!(
        attempt(&f, "constructors are distinct"),
        Decision::GuardUnsatisfiable { .. }
    ));
}

const RAINBOW: &str = r#"
type Colour = Red | Orange | Yellow | Green | Blue | Indigo | Violet | Black

fn rank(c: Colour) -> Int = match c {
  Red -> 0,
  Orange -> 1,
  Yellow -> 2,
  Green -> 3,
  Blue -> 4,
  Indigo -> 5,
  Violet -> 6,
  Black -> 7,
}

law "every rank is in range" forall (c: Colour) { rank(c) >= 0 && rank(c) <= 7 }
law "every rank is small" forall (c: Colour) { rank(c) <= 6 }
law "no rank is seven" forall (c: Colour) { rank(c) != 7 }
"#;

/// The split is over the constructor set, so eight arms are eight branches and
/// the proof covers the type rather than a sample of it.
#[test]
fn a_case_split_over_many_constructors_decides_both_directions() {
    let f = fixture(RAINBOW);
    let proved = proof(&f, "every rank is in range");
    assert!(
        proved.rules.contains(&Rule::CaseSplit {
            ty: Symbol::new("Colour"),
            arms: 8,
        }),
        "{:?}",
        proved.rules
    );
    not_proved(&f, "every rank is small");
    not_proved(&f, "no rank is seven");
}

// ------------------------------------------------------------------ unfolding

const CHAIN: &str = r#"
fn one(x: Int) -> Int = x + 1
fn two(x: Int) -> Int = one(x) + 1
fn three(x: Int) -> Int = two(x) + 1
fn four(x: Int) -> Int = three(x) + 1

fn countdown(n: Int) -> Int = if n <= 0 { 0 } else { countdown(n - 1) + 1 }

law "three unfoldings suffice" forall (x: Int) where x > 0 && x < 1000
  { three(x) == x + 3 }
law "four unfoldings do not" forall (x: Int) where x > 0 && x < 1000
  { four(x) == x + 4 }
law "a recursive call is a function" forall (n: Int) { countdown(n) == countdown(n) }
law "a recursive definition steps" forall (n: Int) where n > 0
  { countdown(n) == countdown(n - 1) + 1 }
"#;

#[test]
fn a_non_recursive_definition_unfolds_to_the_stated_depth() {
    let f = fixture(CHAIN);
    let proved = proof(&f, "three unfoldings suffice");
    let deepest = proved
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::Unfold { depth, .. } => Some(*depth),
            _ => None,
        })
        .max()
        .expect("an unfolding");
    assert!(deepest <= crate::UNFOLD_DEPTH, "{deepest}");
    not_proved(&f, "four unfoldings do not");
}

/// A member of a recursive component is never unfolded: reaching a general
/// statement about one needs induction, which M8 does not have. Both claims
/// below report `Unknown`, and the first is the one that is easy to get wrong.
///
/// `countdown(n) == countdown(n)` is a theorem about a total uninterpreted
/// function symbol and is **not** a theorem about `countdown`, whose body this
/// module never read and which M8 has no termination checker for (ADR 0007
/// §12). Reporting it `proved` is how a definition that never returns ends up
/// counted as covered, telling a reviewer to stop reading a function that
/// cannot answer.
#[test]
fn a_recursive_definition_is_never_unfolded() {
    let f = fixture(CHAIN);
    not_proved(&f, "a recursive call is a function");
    not_proved(&f, "a recursive definition steps");

    let ctx = f.context();
    assert!(ctx.unfoldable(&Symbol::new("countdown")).is_none());
    assert!(ctx.unfoldable(&Symbol::new("three")).is_some());
}

const EFFECTFUL: &str = r#"
effect log { write note(n: Int) -> Unit }
effect counter { write next() -> Int }

fn shout(n: Int) -> Int { log.note(n); n }
fn bump() -> Int = counter.next()
fn difference() -> Int = bump() - bump()
fn once() -> Int { let n = counter.next(); n - n }
"#;

/// An `ensures` on an effectful definition is attempted statically before it is
/// reported as a gap (ADR 0007 §7.4), so the prover does see impure bodies — and
/// **a call that performs is not a function of its arguments.** Two calls to one
/// effectful definition may answer differently, so sharing a term between them
/// would prove `bump() - bump() == 0`, which is false of any `counter` that
/// counts.
#[test]
fn two_calls_to_an_effectful_definition_are_not_one_term() {
    let f = fixture(EFFECTFUL);
    assert!(!matches!(
        returns_zero(&f, "difference"),
        Decision::Proved(_)
    ));
    // The direction the rule must not take with it: one performance bound to a
    // local is evaluated once, so the two uses of `n` are one value and the
    // difference really is zero.
    assert!(matches!(returns_zero(&f, "once"), Decision::Proved(_)));
}

/// `ensures result == 0` on a nullary definition of `source`.
fn returns_zero(fixture: &Fixture, source: &str) -> Decision {
    let ctx = fixture.context();
    let def = fixture.program.modules[0]
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(d) if d.name.name.as_str() == source => Some(&**d),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no definition named `{source}`"));
    let binders = vec![LawBinder {
        name: Symbol::new("result"),
        ty: Type::int(),
        span: Span::DUMMY,
    }];
    let body = ply_syntax::parse_expr(SRC, "result == 0").expect("clause parses");
    decide(
        &ctx,
        &Goal {
            module: 0,
            binders: &binders,
            guards: &[],
            result: Some((Symbol::new("result"), &def.body)),
            body: &body,
        },
        &Limits::default(),
    )
}

/// A definition that performs is not a value the fragment reasons about, so it
/// is never inlined however non-recursive it is.
#[test]
fn an_effectful_definition_is_never_unfolded() {
    let f = fixture(EFFECTFUL);
    let ctx = f.context();
    assert!(ctx.unfoldable(&Symbol::new("shout")).is_none());
}

// -------------------------------------------------------------------- matching

const MATCHING: &str = r#"
fn flip(b: Bool) -> Bool = match b { true -> false, false -> true }
fn absolute(x: Int) -> Int = if x < 0 { 0 - x } else { x }

type Pair = Pair(Int, Int)
fn left(p: Pair) -> Int = match p { Pair(a, _) -> a }
fn swap(p: Pair) -> Pair = match p { Pair(a, b) -> Pair(b, a) }

law "flipping twice is identity" forall (b: Bool) { flip(flip(b)) == b }
law "flipping once is identity" forall (b: Bool) { flip(b) == b }
law "absolute is non negative" forall (x: Int) { absolute(x) >= 0 }
law "a bounded absolute is non negative" forall (x: Int) where x > -1000
  { absolute(x) >= 0 }
law "absolute is positive" forall (x: Int) { absolute(x) > 0 }
law "swapping twice is identity" forall (p: Pair) { swap(swap(p)) == p }
law "swapping once is identity" forall (p: Pair) { swap(p) == p }
law "left of a swap" forall (p: Pair) { left(swap(swap(p))) == left(p) }
"#;

#[test]
fn a_literal_match_decides_both_directions() {
    let f = fixture(MATCHING);
    proof(&f, "flipping twice is identity");
    not_proved(&f, "flipping once is identity");
}

/// The branch a definedness requirement was reached under is part of it: `0 - x`
/// is only ever evaluated where `x < 0`, so the `else` arm costs nothing. What
/// the guard still owes is the low end — `0 - i64::MIN` raises — which is why
/// the unguarded law is not proved and the guarded one is.
#[test]
fn an_if_decides_both_directions() {
    let f = fixture(MATCHING);
    proof(&f, "a bounded absolute is non negative");
    not_proved(&f, "absolute is non negative");
    not_proved(&f, "absolute is positive");
}

#[test]
fn a_constructor_match_decides_both_directions() {
    let f = fixture(MATCHING);
    proof(&f, "swapping twice is identity");
    not_proved(&f, "swapping once is identity");
    proof(&f, "left of a swap");
}

// ------------------------------------------------------------------- ensures

const LEDGER: &str = r#"
type Account = Account(Int, Int)

fn identifier(a: Account) -> Int = match a { Account(i, _) -> i }
fn balance(a: Account) -> Int = match a { Account(_, b) -> b }

fn withdraw(acct: Account, amount: Int) -> Account =
  Account(identifier(acct), balance(acct) - amount)
"#;

fn ensures_goal(source: &str, clause: &str) -> Decision {
    let f = fixture(source);
    let ctx = f.context();
    let def = f.program.modules[0]
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(d) if d.name.name.as_str() == "withdraw" => Some(&**d),
            _ => None,
        })
        .expect("withdraw");

    let mut vars = BTreeMap::new();
    let mut binders: Vec<LawBinder> = def
        .params
        .iter()
        .map(|p| LawBinder {
            name: p.name.name.clone(),
            ty: resolve_type(p.ty.as_ref().expect("annotated"), &mut vars),
            span: p.span,
        })
        .collect();
    binders.push(LawBinder {
        name: Symbol::new("result"),
        ty: resolve_type(def.ret.as_ref().expect("annotated"), &mut vars),
        span: Span::DUMMY,
    });

    let body = ply_syntax::parse_expr(SRC, clause).expect("clause parses");
    let guard = ply_syntax::parse_expr(SRC, "amount > 0").expect("guard parses");
    let guards = [&guard];
    decide(
        &ctx,
        &Goal {
            module: 0,
            binders: &binders,
            guards: &guards,
            result: Some((Symbol::new("result"), &def.body)),
            body: &body,
        },
        &Limits::default(),
    )
}

/// The postcondition of ADR 0007 §1.1, end to end: `result` is bound to the
/// definition's own body, the constructor is injective, the accessors unfold,
/// and the arithmetic closes.
#[test]
fn a_postcondition_over_a_definition_decides_both_directions() {
    assert!(matches!(
        ensures_goal(LEDGER, "balance(result) == balance(acct) - amount"),
        Decision::Proved(_)
    ));
    assert!(matches!(
        ensures_goal(LEDGER, "identifier(result) == identifier(acct)"),
        Decision::Proved(_)
    ));
    assert!(!matches!(
        ensures_goal(LEDGER, "balance(result) == balance(acct) + amount"),
        Decision::Proved(_)
    ));
}

/// Without the definitional equation `result` is an arbitrary value of its
/// type, and a postcondition mentioning it cannot be valid. The prover says
/// `Unknown` rather than inventing one.
#[test]
fn a_postcondition_without_the_definition_is_unknown() {
    let f = fixture(LEDGER);
    let ctx = f.context();
    let mut vars = BTreeMap::new();
    let account = resolve_type(
        &TypeExpr::Con {
            name: ply_syntax::ast::QName::bare(ply_syntax::ast::Ident::new("Account", Span::DUMMY)),
            args: Vec::new(),
            span: Span::DUMMY,
        },
        &mut vars,
    );
    let binders = vec![
        LawBinder {
            name: Symbol::new("acct"),
            ty: account.clone(),
            span: Span::DUMMY,
        },
        LawBinder {
            name: Symbol::new("amount"),
            ty: Type::int(),
            span: Span::DUMMY,
        },
        LawBinder {
            name: Symbol::new("result"),
            ty: account,
            span: Span::DUMMY,
        },
    ];
    let body = ply_syntax::parse_expr(SRC, "balance(result) == balance(acct) - amount").unwrap();
    let decision = decide(
        &ctx,
        &Goal {
            module: 0,
            binders: &binders,
            guards: &[],
            result: None,
            body: &body,
        },
        &Limits::default(),
    );
    assert!(!matches!(decision, Decision::Proved(_)), "{decision:?}");
}

// --------------------------------------------------------- budget and honesty

const HARD: &str = r#"
type Bit = Zero | One
fn value(b: Bit) -> Int = match b { Zero -> 0, One -> 1 }

law "four bits are bounded" forall (a: Bit, b: Bit, c: Bit, d: Bit)
  { value(a) + value(b) + value(c) + value(d) <= 4 }

law "five bits are bounded" forall (a: Bit, b: Bit, c: Bit, d: Bit, e: Bit)
  { value(a) + value(b) + value(c) + value(d) + value(e) <= 5 }
"#;

#[track_caller]
fn spends_its_budget(fixture: &Fixture, label: &str, limits: &Limits) {
    let decision = attempt_with(fixture, label, limits);
    assert!(
        matches!(
            decision,
            Decision::Unknown {
                reason: Reason::BudgetSpent,
                ..
            }
        ),
        "`{label}`: {decision:?}"
    );
}

/// A spent budget is inconclusive, and inconclusive reports the weaker tier.
/// It is never a proof and never a refutation.
#[test]
fn a_spent_budget_is_unknown_and_never_proved() {
    let f = fixture(HARD);
    spends_its_budget(
        &f,
        "four bits are bounded",
        &Limits {
            steps: 20,
            ..Limits::default()
        },
    );
    // The same obligation decides at the default budget, so the assertion above
    // is measuring the budget and not the fragment.
    proof(&f, "four bits are bounded");
}

/// The case analysis is exponential in the number of binders, and every sum in
/// the body carries a definedness requirement decided in the same analysis, so
/// the default budget is reached long before the fragment is. Five binders over
/// a two-constructor type does not fit in [`crate::DEFAULT_PROVE_BUDGET`] — and
/// the answer to that is `Unknown`, which the tier contract reads as
/// `property`. Widening the budget decides it, which is what makes this a
/// budget boundary and not a claim about the fragment.
///
/// ADR 0007 §5.1(f) is the reason this costs nothing in practice: `Bit⁵` is 32
/// points, well inside `ENUMERATION_BOUND`, so the obligation is proved by
/// exhaustive enumeration rather than by this module.
#[test]
fn an_obligation_larger_than_the_default_budget_is_unknown() {
    let f = fixture(HARD);
    spends_its_budget(&f, "five bits are bounded", &Limits::default());
    let wide = Limits {
        steps: 100_000,
        ..Limits::default()
    };
    assert!(matches!(
        attempt_with(&f, "five bits are bounded", &wide),
        Decision::Proved(_)
    ));
}

/// Two runs of the prover over one obligation produce one answer, including the
/// rule list, which is what makes today's artifact diffable against
/// yesterday's.
#[test]
fn a_decision_is_a_function_of_the_obligation() {
    let f = fixture(RAINBOW);
    assert_eq!(
        attempt(&f, "every rank is in range"),
        attempt(&f, "every rank is in range")
    );
}

const GUARDED: &str = r#"
law "a guard nothing establishes" forall (x: Int, y: Int) where x < y { x != y }
law "an unguarded claim" forall (x: Int) { x == x }
"#;

/// A certificate whose guard was never shown to admit a value has a domain it
/// cannot vouch for, so it cannot be built until something establishes one.
#[test]
fn a_certificate_needs_the_guard_to_admit_a_value() {
    let f = fixture(GUARDED);
    let guarded = proof(&f, "a guard nothing establishes");
    assert!(!guarded.guard_satisfiable);
    assert!(guarded.certify(false).is_none());
    let certificate = guarded.certify(true).expect("a kept case establishes it");
    assert!(certificate.guard_satisfiable);

    let unguarded = proof(&f, "an unguarded claim");
    assert!(unguarded.guard_satisfiable);
    assert!(unguarded.certify(false).is_some());
}

const UNINHABITED: &str = r#"
type Bottom = Wrap(Bottom)

law "anything about nothing" forall (b: Bottom) { b == b }
"#;

/// A type with no values makes every claim about it valid and empty. The proof
/// stands; the guard's satisfiability does not, so no certificate is built
/// without a kept case — and there will never be one.
#[test]
fn an_uninhabited_domain_does_not_establish_satisfiability() {
    let f = fixture(UNINHABITED);
    let proved = proof(&f, "anything about nothing");
    assert!(!proved.guard_satisfiable);
    assert!(proved.certify(false).is_none());
}

const LISTS: &str = r#"
law "list literals are injective" forall (a: Int, b: Int) where [a] == [b] { a == b }
law "list literals are injective by one" forall (a: Int, b: Int) where [a] == [b]
  { a == b + 1 }
law "lists of different lengths differ" forall (a: Int, b: Int) where [a] == [b, b]
  { a == b + 1 }
"#;

#[test]
fn list_literals_are_injective_and_length_distinct() {
    let f = fixture(LISTS);
    proof(&f, "list literals are injective");
    not_proved(&f, "list literals are injective by one");
    assert!(matches!(
        attempt(&f, "lists of different lengths differ"),
        Decision::GuardUnsatisfiable { .. }
    ));
}

const OPAQUE: &str = r#"
type Tree = Leaf | Node(Tree, Tree)

fn depth_one(t: Tree) -> Int = match t {
  Node(Leaf, _) -> 1,
  _ -> 0,
}

law "a nested pattern is not decided" forall (t: Tree) { depth_one(t) >= 0 }
law "a nested pattern is still a function" forall (t: Tree) { depth_one(t) == depth_one(t) }
"#;

/// A nested constructor pattern leaves its `match` uninterpreted rather than
/// guessed. The first law is true and reports `Unknown`, which is the right
/// answer; the second still holds by congruence.
#[test]
fn a_nested_pattern_leaves_the_match_uninterpreted() {
    let f = fixture(OPAQUE);
    not_proved(&f, "a nested pattern is not decided");
    proof(&f, "a nested pattern is still a function");
}

// ----------------------------------------------------------------- the audit

/// What is actually true of a law, established by hand rather than by the
/// component under test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Truth {
    /// Holds of every input its guard admits, and the guard admits one.
    Valid,
    /// Holds wherever it evaluates, and **raises** at some input its guard
    /// admits — an `Int` that left `i64`, a zero divisor, a definition that does
    /// not return. Valid over ℤ and over total functions, and therefore exactly
    /// the shape a prover reasoning over those would wrongly certify.
    Partial,
    /// Some input its guard admits falsifies it.
    Refutable,
    /// The guard admits nothing.
    Vacuous,
}

const CORPUS: &str = r#"
type Shade = Pale | Mid | Deep

fn weight(s: Shade) -> Int = match s { Pale -> 1, Mid -> 2, Deep -> 3 }
fn twice(x: Int) -> Int = x + x
fn quadruple(x: Int) -> Int = twice(x) + twice(x)

law "identity of addition" forall (x: Int) { x + 0 == x }
law "addition commutes" forall (x: Int, y: Int) { x + y == y + x }
law "bounded addition commutes" forall (x: Int, y: Int)
  where x > -1000 && x < 1000 && y > -1000 && y < 1000 { x + y == y + x }
law "addition associates" forall (x: Int, y: Int, z: Int)
  { (x + y) + z == x + (y + z) }
law "bounded addition associates" forall (x: Int, y: Int, z: Int)
  where x > -1000 && x < 1000 && y > -1000 && y < 1000 && z > -1000 && z < 1000
  { (x + y) + z == x + (y + z) }
law "self subtraction is zero" forall (x: Int) { x - x == 0 }
law "negation is subtraction" forall (x: Int) { 0 - x == -x }
law "bounded negation is subtraction" forall (x: Int) where x > -1000 && x < 1000
  { 0 - x == -x }
law "doubling scales" forall (x: Int) { twice(x) == 2 * x }
law "bounded doubling scales" forall (x: Int) where x > -1000 && x < 1000
  { twice(x) == 2 * x }
law "quadrupling scales" forall (x: Int) { quadruple(x) == 4 * x }
law "bounded quadrupling scales" forall (x: Int) where x > -1000 && x < 1000
  { quadruple(x) == 4 * x }
law "a positive is at least one" forall (x: Int) where x > 0 { x >= 1 }
law "at least one is positive" forall (x: Int) where x >= 1 { x > 0 }
law "antisymmetry" forall (x: Int, y: Int) where x <= y && y <= x { x == y }
law "strictness is asymmetric" forall (x: Int, y: Int) where x < y { y > x }
law "multiplication commutes" forall (x: Int, y: Int) { x * y == y * x }
law "a square is not negative" forall (x: Int) { x * x >= 0 }
law "de morgan for conjunction" forall (b: Bool, c: Bool)
  { !(b && c) == (!b || !c) }
law "de morgan for disjunction" forall (b: Bool, c: Bool)
  { !(b || c) == (!b && !c) }
law "double negation" forall (b: Bool) { !!b == b }
law "a disjunct is implied" forall (b: Bool, c: Bool) where b { b || c }
law "a branch is one of two" forall (b: Bool, x: Int, y: Int)
  { if b { x } else { y } == x || if b { x } else { y } == y }
law "a function is a function" forall (f: (Int) -> Int, x: Int) { f(x) == f(x) }
law "equal arguments agree" forall (f: (Int) -> Int, x: Int, y: Int) where x == y
  { f(x) == f(y) }
law "a field is what was put in it" forall (x: Int) { { amount: x }.amount == x }
law "every weight is positive" forall (s: Shade) { weight(s) >= 1 }
law "no weight is four" forall (s: Shade) { weight(s) != 4 }

law "successor is identity" forall (x: Int) { x + 1 == x }
law "everything is ordered one way" forall (x: Int, y: Int) { x > y }
law "a positive is at least two" forall (x: Int) where x > 0 { x > 1 }
law "doubling is squaring" forall (x: Int) { twice(x) == x * x }
law "quadrupling is tripling" forall (x: Int) { quadruple(x) == 3 * x }
law "de morgan mangled" forall (b: Bool, c: Bool) { !(b && c) == (!b && !c) }
law "a disjunction always holds" forall (b: Bool, c: Bool) { b || c }
law "a branch is always the first" forall (b: Bool, x: Int, y: Int)
  { if b { x } else { y } == x }
law "any two arguments agree" forall (f: (Int) -> Int, x: Int, y: Int)
  { f(x) == f(y) }
law "a field is one more" forall (x: Int) { { amount: x }.amount == x + 1 }
law "every weight is one" forall (s: Shade) { weight(s) == 1 }
law "weights stop at two" forall (s: Shade) { weight(s) <= 2 }
law "ordering skips two" forall (x: Int, y: Int, z: Int) where x < y && y < z
  { x < z - 2 }
law "halving round trips" forall (x: Int) { x / 2 * 2 == x }

law "between zero and one" forall (x: Int) where x > 0 && x < 1 { x == x }
law "its own successor" forall (x: Int) where x == x + 1 { x != x }
law "its own bounded successor" forall (x: Int) where x < 100 && x == x + 1
  { x != x }
law "true and false at once" forall (b: Bool) where b && !b { !b }
law "one is two" forall (x: Int) where 1 == 2 { x != x }
law "two weights at once" forall (s: Shade) where weight(s) == 1 && weight(s) == 3
  { s == s }
"#;

fn corpus() -> Vec<(&'static str, Truth)> {
    let valid = [
        "identity of addition",
        "bounded addition commutes",
        "bounded addition associates",
        "self subtraction is zero",
        "bounded negation is subtraction",
        "bounded doubling scales",
        "bounded quadrupling scales",
        "a positive is at least one",
        "at least one is positive",
        "antisymmetry",
        "strictness is asymmetric",
        "multiplication commutes",
        "a square is not negative",
        "de morgan for conjunction",
        "de morgan for disjunction",
        "double negation",
        "a disjunct is implied",
        "a branch is one of two",
        "a function is a function",
        "equal arguments agree",
        "a field is what was put in it",
        "every weight is positive",
        "no weight is four",
    ];
    let partial = [
        "addition commutes",
        "addition associates",
        "negation is subtraction",
        "doubling scales",
        "quadrupling scales",
        // The guard itself raises at `i64::MAX`, so its domain is not empty —
        // it is undecided, which is a different thing and a weaker claim.
        "its own successor",
    ];
    let refutable = [
        "successor is identity",
        "everything is ordered one way",
        "a positive is at least two",
        "doubling is squaring",
        "quadrupling is tripling",
        "de morgan mangled",
        "a disjunction always holds",
        "a branch is always the first",
        "any two arguments agree",
        "a field is one more",
        "every weight is one",
        "weights stop at two",
        "ordering skips two",
        "halving round trips",
    ];
    let vacuous = [
        "between zero and one",
        "its own bounded successor",
        "true and false at once",
        "one is two",
        "two weights at once",
    ];
    valid
        .into_iter()
        .map(|l| (l, Truth::Valid))
        .chain(partial.into_iter().map(|l| (l, Truth::Partial)))
        .chain(refutable.into_iter().map(|l| (l, Truth::Refutable)))
        .chain(vacuous.into_iter().map(|l| (l, Truth::Vacuous)))
        .collect()
}

/// The audit, and the most important test in this module: over a corpus whose
/// truth was established by hand, **nothing false is ever proved and nothing
/// with a real domain is ever called vacuous.**
///
/// It says nothing about reach. A `Valid` law is allowed to come back
/// `Unknown` — `x * y == y * x` and `x * x >= 0` are in the corpus precisely
/// because they are true and outside the fragment — and every entry that does
/// not come back `Proved` costs a weaker tier and nothing else.
#[test]
fn nothing_false_is_ever_proved_and_nothing_real_is_ever_vacuous() {
    let f = fixture(CORPUS);
    for (label, truth) in corpus() {
        let decision = attempt(&f, label);
        match truth {
            Truth::Valid => assert!(
                !matches!(decision, Decision::GuardUnsatisfiable { .. }),
                "`{label}` has a domain: {decision:?}"
            ),
            // The entries this milestone's worst defect is made of: valid over
            // ℤ and over total function symbols, and raising at some input the
            // guard admits. A prover that reported one `proved` would be
            // certifying an obligation with no input at which it holds.
            Truth::Partial => assert!(
                matches!(decision, Decision::Unknown { .. }),
                "`{label}` raises at some input its guard admits: {decision:?}"
            ),
            Truth::Refutable => assert!(
                matches!(decision, Decision::Unknown { .. }),
                "`{label}` is false and has a domain: {decision:?}"
            ),
            Truth::Vacuous => assert!(
                !matches!(decision, Decision::Proved(_)),
                "`{label}` admits nothing, so a proof of its body is a proof of \
                 everything: {decision:?}"
            ),
        }
    }
}

/// The corpus is worth exactly what its reach is: an audit every entry of which
/// came back `Unknown` would pass while proving nothing at all. So the two
/// halves are pinned by name rather than by a count.
///
/// The only valid entries the fragment leaves undecided are the two whose truth
/// depends on `x * y` with both factors symbolic — the boundary ADR 0007 §5.1(a)
/// draws, restated from the far side.
#[test]
fn the_audit_corpus_exercises_the_fragment() {
    let f = fixture(CORPUS);
    let undecided: Vec<&str> = corpus()
        .into_iter()
        .filter(|(label, truth)| {
            *truth == Truth::Valid && !matches!(attempt(&f, label), Decision::Proved(_))
        })
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        undecided,
        ["multiplication commutes", "a square is not negative"]
    );

    let missed: Vec<&str> = corpus()
        .into_iter()
        .filter(|(label, truth)| {
            *truth == Truth::Vacuous
                && !matches!(attempt(&f, label), Decision::GuardUnsatisfiable { .. })
        })
        .map(|(label, _)| label)
        .collect();
    assert!(missed.is_empty(), "{missed:?}");
}
