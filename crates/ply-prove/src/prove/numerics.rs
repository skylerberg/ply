//! What `Float` and `Decimal` may and may not do to the `proved` tier.
//!
//! A tier label is a truth claim, and this milestone is the one that can produce
//! a wrong answer wearing a certificate. The rule the whole file exists to hold:
//! **a verdict may only get stronger when the evidence does, and a type arriving
//! is not evidence.**
//!
//! - `Float` is excluded from `proved` entirely. `==` on it is not reflexive, so
//!   congruence closure over it is unsound and there is nothing to weaken; the
//!   refusal is structural, so no `Proof` is built to be discarded later.
//! - `Decimal` may appear only as an **uninterpreted term**. Its `==` *is* an
//!   equivalence relation, so `f(x) == f(x)` is a genuine proof; there is no
//!   theory of `+` or `<`, so `x + 0m == x` is `property`.

use super::tests::{Fixture, binders, fixture, not_proved, proof};
use super::{Blocker, Decision, Goal, Limits, decide_and_diagnose};
use ply_syntax::ast::Expr;

/// [`decide_and_diagnose`] over a law, so a test can assert *why* an attempt was
/// refused rather than only that it was.
fn attempt_for_test(f: &Fixture, label: &str) -> (Decision, Vec<Blocker>) {
    let ctx = f.context();
    let law = f.law(label);
    let binders = binders(law);
    let guards: Vec<&Expr> = law.guard.iter().collect();
    decide_and_diagnose(
        &ctx,
        &Goal {
            module: 0,
            binders: &binders,
            guards: &guards,
            result: None,
            body: &law.body,
        },
        &Limits::default(),
    )
}

// ------------------------------------------------------------------- `Float`

const FLOATS: &str = r#"
law "a float equals itself" forall (x: Float) { x == x }
law "a float sum commutes" forall (x: Float, y: Float) { x + y == y + x }
law "a float is at least itself" forall (x: Float) { x >= x }
law "a float zero is additive" forall (x: Float) { x + 0.0 == x }
law "one point five is one point five" forall (n: Int) { 1.5 == 1.5 }
law "a float in a list" forall (xs: List<Float>) { len(xs) >= 0 }
law "a float in a record" forall (r: {rate: Float}) { r == r }
fn half() -> Float = 0.5
law "a float from a call" forall (n: Int) { half() == half() }
"#;

/// Every one of these is either trivially true or true of every value that is
/// not a `NaN`, and **none** of them may be proved. Including the trivial ones:
/// `x == x` is exactly the sentence that is false at `NaN`, and a prover that
/// certified it would be wrong about the one value the type is defined by.
#[test]
fn no_law_mentioning_a_float_is_ever_proved() {
    let f = fixture(FLOATS);
    for label in [
        "a float equals itself",
        "a float sum commutes",
        "a float is at least itself",
        "a float zero is additive",
        "one point five is one point five",
        "a float in a list",
        "a float in a record",
        "a float from a call",
    ] {
        not_proved(&f, label);
    }
}

/// The refusal is reported as a blocker rather than being invisible, so "what
/// would extend this prover" stays a number somebody can read.
#[test]
fn a_refused_float_obligation_says_that_is_why() {
    let f = fixture(FLOATS);
    let (decision, blockers) = attempt_for_test(&f, "a float equals itself");
    assert!(matches!(decision, Decision::Unknown { .. }), "{decision:?}");
    assert!(
        blockers.contains(&Blocker::FloatTerm),
        "the blocker names the reason: {blockers:?}"
    );
}

/// A `Float` binder is refused even where the *body* never mentions it. The
/// question is whether the obligation mentions the type, not whether the
/// arithmetic happened to touch it.
#[test]
fn a_float_binder_alone_is_enough_to_refuse() {
    let f = fixture(
        r#"
        law "an int is itself, beside a float" forall (x: Int, unused: Float) { x == x }
        "#,
    );
    not_proved(&f, "an int is itself, beside a float");
}

/// The control: the same claim without the `Float` binder is proved, so the
/// refusal above is the `Float` and not a general loss of reach.
#[test]
fn the_same_claim_without_a_float_is_still_proved() {
    let f = fixture(r#"law "an int is itself" forall (x: Int) { x == x }"#);
    proof(&f, "an int is itself");
}

// ----------------------------------------------------------------- `Decimal`

const DECIMALS: &str = r#"
fn scaled(d: Decimal) -> Decimal = d
fn wrap(d: Decimal) -> { amount: Decimal } = { amount: d }

law "a decimal equals itself" forall (x: Decimal) { x == x }
law "congruence over a decimal" forall (x: Decimal) { scaled(x) == scaled(x) }
law "a decimal inside a record" forall (x: Decimal) { wrap(x) == wrap(x) }
law "a decimal literal is itself" forall (n: Int) { 1.5m == 1.5m }
law "two scales are one value" forall (n: Int) { 1.5m == 1.50m }
law "two values are two values" forall (n: Int) { 1.5m == 1.6m }

law "a decimal zero is additive" forall (x: Decimal) { x + 0m == x }
law "a decimal sum commutes" forall (x: Decimal, y: Decimal) { x + y == y + x }
law "a decimal is at least itself" forall (x: Decimal) { x >= x }
"#;

/// `==` on `Decimal` is an equivalence relation, so reflexivity and congruence
/// are sound over it — which is the whole of what the type is allowed to do
/// inside a certificate.
#[test]
fn a_decimal_is_provable_as_an_uninterpreted_term() {
    let f = fixture(DECIMALS);
    proof(&f, "a decimal equals itself");
    proof(&f, "congruence over a decimal");
    proof(&f, "a decimal inside a record");
    proof(&f, "a decimal literal is itself");
}

/// `1.5m` and `1.50m` are **equal in value** and differently written. The term
/// interner normalizes by value, so the prover agrees with the evaluator; a
/// representation that kept the scale would certify `1.5m != 1.50m`, which is
/// false.
#[test]
fn two_decimal_literals_of_one_value_are_one_term() {
    let f = fixture(DECIMALS);
    proof(&f, "two scales are one value");
    // And two literals that really are different values stay different, so the
    // normalization did not merge everything.
    not_proved(&f, "two values are two values");
}

/// No arithmetic and no ordering. `x + 0m == x` is true over the rationals and
/// **raises** at `Decimal::MAX`, so it is not true of every input the way a
/// statement about a total function would be — and there is no theory here to
/// decide it either way.
#[test]
fn decimal_arithmetic_and_ordering_are_property_rather_than_proved() {
    let f = fixture(DECIMALS);
    not_proved(&f, "a decimal zero is additive");
    not_proved(&f, "a decimal sum commutes");
    not_proved(&f, "a decimal is at least itself");
}

/// A `Decimal` obligation is refused for a *reason*, and the reason is not the
/// one `Float` gets: nothing about the type is outside the fragment, only its
/// arithmetic.
#[test]
fn a_decimal_never_reports_the_float_blocker() {
    let f = fixture(DECIMALS);
    for label in ["a decimal zero is additive", "a decimal equals itself"] {
        let (_, blockers) = attempt_for_test(&f, label);
        assert!(
            !blockers.contains(&Blocker::FloatTerm),
            "`{label}` is not a `Float` problem: {blockers:?}"
        );
    }
    let (_, blockers) = attempt_for_test(&f, "a decimal zero is additive");
    assert!(
        blockers.contains(&Blocker::DecimalArithmetic),
        "{blockers:?}"
    );
}

/// The linear-arithmetic fragment is over `Int` and does not extend by a type
/// arriving. This is the same claim as the two above, asked the other way: the
/// `Int` version of a law is proved and the `Decimal` version is not.
#[test]
fn the_arithmetic_fragment_did_not_grow() {
    let ints = fixture(
        r#"
        law "an int zero is additive" forall (x: Int) where x > 0 && x < 100 { x + 0 == x }
        "#,
    );
    proof(&ints, "an int zero is additive");

    let decimals = fixture(
        r#"
        law "a decimal zero is additive" forall (x: Decimal) { x + 0m == x }
        "#,
    );
    not_proved(&decimals, "a decimal zero is additive");
}

/// The prelude's ADTs are declared by the language rather than by a file, so
/// the fragment has to see their constructor lists in full — otherwise a
/// `match o { None -> .., Some(v) -> .. }` is a case analysis the prover
/// declines and an obligation it can decide comes back `Unknown`.
///
/// It sits in the numerics file because the prelude's ADTs arrived with
/// `Decimal`: `int_of_decimal` returns an `Option` and `decimal_div` takes a
/// `Rounding`.
#[test]
fn a_prelude_adt_is_a_case_analysis_like_any_other() {
    let f = fixture(
        "fn or_else(o: Option<Int>, d: Int) -> Int = match o { None -> d, Some(v) -> v }\n\
         fn direction(o: Ordering) -> Int = match o { Less -> -1, Equal -> 0, Greater -> 1 }\n\
         law \"or_else is a function\" forall (o: Option<Int>, d: Int)\n\
           { or_else(o, d) == or_else(o, d) }\n\
         law \"a direction is one of three\" forall (o: Ordering)\n\
           { direction(o) == -1 || direction(o) == 0 || direction(o) == 1 }",
    );
    proof(&f, "or_else is a function");
    proof(&f, "a direction is one of three");
}

/// The one way a `Float` can enter the graph carrying no sort of its own.
///
/// A destructuring bind is outside the fragment, so every name it introduces is
/// a fresh symbol with no sort — and an unsorted operand takes the `Int` path,
/// where `a + b` folds into a linear combination and `a + b == b + a` is a
/// theorem. It is not a theorem over binary64: `NaN + 1.0` is `NaN`, which is
/// not equal to itself. The scrutinee's sort is what closes it.
#[test]
fn a_float_pulled_out_of_a_destructuring_is_still_refused() {
    let f = fixture(
        r#"
        fn rates() -> { a: Float, b: Float } = { a: 1.0, b: 2.0 }
        fn ints() -> { a: Int, b: Int } = { a: 1, b: 2 }

        law "a destructured float sum commutes" forall (n: Int) {
          let { a: x, b: y } = rates();
          x + y == y + x
        }
        law "a destructured int sum commutes" forall (n: Int) {
          let { a: x, b: y } = ints();
          x + y == y + x
        }
        law "a matched float sum commutes" forall (n: Int) {
          match rates() { { a: x, b: y } -> x + y == y + x }
        }
        "#,
    );
    not_proved(&f, "a destructured float sum commutes");
    not_proved(&f, "a matched float sum commutes");
    // The `Int` control, so the refusal above is the `Float` rather than the
    // destructuring: this one is outside the fragment for its own reason and
    // must not be reported as a `Float` problem.
    let (_, blockers) = attempt_for_test(&f, "a destructured int sum commutes");
    assert!(
        !blockers.contains(&Blocker::FloatTerm),
        "an `Int` destructuring is not a `Float` refusal: {blockers:?}"
    );
}
