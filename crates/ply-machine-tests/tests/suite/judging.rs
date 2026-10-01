//! The runtime's half of a discharge: a claim's points judged as the program asks, and a law over
//! interleavings searched at the points its guard kept. Which points those are, and what their
//! judgements come to, is `proof.property`'s, pinned by its own tests.

use crate::fixture::{loaded, project};
use ply_eval::{DefHash, Plain, Span, Symbol, Value, codes};
use ply_machine::engine::{Judgement, Mode, Prover};
use ply_prove::concurrency::ValueDomain;
use ply_prove::{
    Binder, Discharge, Evidence, Gap, Obligation, ObligationKind, ProvePlan, Sort, Strategy,
};

const SOURCE: &str = r#"
law "halving a choice" forall (b: Bool) { (if b { 4 } else { 6 }) / 2 > 1 }

law "doubling is tripling" forall (n: Int) { n + n == n * 3 }

law "dividing by a choice" forall (b: Bool) { 12 / (if b { 3 } else { 0 }) > 1 }

law "past a hundred" forall (n: Int) where n > 100 { n > 0 }

law "a guarded quotient" forall (n: Int) where 12 / n > 0 { true }

law "a quotient past zero" forall (n: Int) where n != 0 { 12 / n != 99 }

fn capped(n: Int) -> Int
  requires n < 1000
  ensures result <= 1000
  ensures result == n
= if n > 10 { 10 } else { n }
"#;

fn binder(name: &str, sort: Sort, text: &str) -> Binder {
    Binder {
        name: Symbol::new(name),
        sort,
        text: text.to_string(),
    }
}

fn claim(owner: &str, kind: ObligationKind, binders: Vec<Binder>, guards: usize) -> Obligation {
    Obligation {
        key: DefHash([7; 32]),
        owner: Symbol::new(owner),
        kind,
        span: Span::DUMMY,
        binders,
        result: None,
        variables: Vec::new(),
        footprint: Some("{m.db.read[users]}".to_string()),
        strategy: Strategy::Static,
        guards: vec![Span::DUMMY; guards],
    }
}

fn over_a_bool(owner: &str) -> Obligation {
    claim(
        owner,
        ObligationKind::Law,
        vec![binder("b", Sort::bool(), "Bool")],
        0,
    )
}

fn over_an_int(owner: &str, guards: usize) -> Obligation {
    claim(
        owner,
        ObligationKind::Law,
        vec![binder("n", Sort::int(), "Int")],
        guards,
    )
}

/// `capped`'s `ensures` clause `index`, under its one `requires`.
fn capped(index: usize) -> Obligation {
    Obligation {
        result: Some(binder("result", Sort::int(), "Int")),
        ..claim(
            "m.capped",
            ObligationKind::Ensures { index },
            vec![binder("n", Sort::int(), "Int")],
            1,
        )
    }
}

fn with_prover<R>(f: impl FnOnce(&Prover<'_>) -> R) -> R {
    let dir = project(SOURCE);
    let loaded = loaded(dir.path());
    let backend =
        ply_machine::support::prover_backend(&loaded).expect("the program compiles to a tier");
    f(&Prover::new(&loaded, backend))
}

fn bools(bs: &[bool]) -> Vec<Vec<Value>> {
    bs.iter().map(|&b| vec![Value::Bool(b)]).collect()
}

fn ints(ns: &[i64]) -> Vec<Vec<Value>> {
    ns.iter().map(|&n| vec![Value::Int(n)]).collect()
}

fn judged(obligation: &Obligation, points: &[Vec<Value>], mode: Mode) -> Vec<Judgement> {
    with_prover(|prover| prover.judged(obligation, &ProvePlan::default(), points, mode))
}

/// Each judgement's name, with the code a stopped one carries.
fn shown(judgements: &[Judgement]) -> Vec<String> {
    judgements
        .iter()
        .map(|j| match j {
            Judgement::Held => "held".to_string(),
            Judgement::Failed => "failed".to_string(),
            Judgement::Rejected => "rejected".to_string(),
            Judgement::Raised(d) => format!("raised {}", d.code),
            Judgement::Faulted(d) => format!("faulted {}", d.code),
        })
        .collect()
}

#[test]
fn a_claim_is_judged_until_a_point_falsifies_it() {
    let judgements = judged(
        &over_an_int("m.doubling is tripling", 0),
        &ints(&[0, 1, 2]),
        Mode::Whole,
    );
    assert_eq!(shown(&judgements), ["held", "failed"]);
}

#[test]
fn the_programs_raise_ends_a_batch_and_is_not_a_fault() {
    let judgements = judged(
        &over_a_bool("m.dividing by a choice"),
        &bools(&[true, false, true]),
        Mode::Whole,
    );
    assert_eq!(
        shown(&judgements),
        [
            "held".to_string(),
            format!("raised {}", codes::RUNTIME_ERROR)
        ]
    );
}

/// The body divides by `n`; the guard keeps it from ever seeing zero.
#[test]
fn a_guard_rejects_a_point_before_the_body_runs() {
    let judgements = judged(
        &over_an_int("m.a quotient past zero", 1),
        &ints(&[0, 3]),
        Mode::Whole,
    );
    assert_eq!(shown(&judgements), ["rejected", "held"]);
}

#[test]
fn a_witness_search_runs_the_guard_alone_until_it_admits_a_point() {
    let judgements = judged(
        &over_an_int("m.past a hundred", 1),
        &ints(&[1, 101, 102]),
        Mode::Witness,
    );
    assert_eq!(shown(&judgements), ["rejected", "held"]);
}

#[test]
fn a_domain_is_the_guard_at_every_point_until_it_raises() {
    let judgements = judged(
        &over_an_int("m.past a hundred", 1),
        &ints(&[1, 101, 2]),
        Mode::Domain,
    );
    assert_eq!(shown(&judgements), ["rejected", "held", "rejected"]);
    let judgements = judged(
        &over_an_int("m.a guarded quotient", 1),
        &ints(&[1, 0, 2]),
        Mode::Domain,
    );
    assert_eq!(
        shown(&judgements),
        [
            "held".to_string(),
            format!("raised {}", codes::RUNTIME_ERROR)
        ]
    );
}

#[test]
fn an_ensures_calls_its_owner_for_the_result_it_states() {
    assert_eq!(
        shown(&judged(&capped(0), &ints(&[5, 2000, 11]), Mode::Whole)),
        ["held", "rejected", "held"]
    );
    assert_eq!(
        shown(&judged(&capped(1), &ints(&[5, 11, 3]), Mode::Whole)),
        ["held", "failed"]
    );
}

/// A binder the law does not take makes the tier decline the entry: Ply's failure, never the
/// program's raise.
#[test]
fn an_entry_the_tier_declines_is_plys_failure() {
    let mismatched = claim(
        "m.halving a choice",
        ObligationKind::Law,
        vec![
            binder("b", Sort::bool(), "Bool"),
            binder("spare", Sort::bool(), "Bool"),
        ],
        0,
    );
    let judgements = judged(
        &mismatched,
        &[
            vec![Value::Bool(true), Value::Bool(false)],
            vec![Value::Bool(false), Value::Bool(false)],
        ],
        Mode::Whole,
    );
    let [Judgement::Faulted(diagnostic)] = judgements.as_slice() else {
        panic!("a declined entry ends the batch as Ply's fault: {judgements:?}");
    };
    assert_eq!(diagnostic.code, codes::INTERNAL_ERROR, "{diagnostic:?}");
    assert!(
        diagnostic.message.contains("declined"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn a_claim_the_program_does_not_state_is_plys_failure() {
    let judgements = judged(&over_a_bool("m.no such law"), &bools(&[true]), Mode::Whole);
    assert_eq!(
        shown(&judgements),
        [format!("faulted {}", codes::INTERNAL_ERROR)]
    );
}

/// The program judges a `law/host` only when the run bound a host.
#[test]
fn a_hosted_claim_judged_with_no_host_bound_is_plys_failure() {
    let hosted = Obligation {
        strategy: Strategy::Hosted,
        ..over_an_int("m.doubling is tripling", 0)
    };
    assert_eq!(
        shown(&judged(&hosted, &ints(&[0]), Mode::Whole)),
        [format!("faulted {}", codes::INTERNAL_ERROR)]
    );
}

/// The claim reaches no `simulate` region, so each point is one evaluation, and nothing it
/// scheduled is a proof.
#[test]
fn a_law_over_interleavings_is_searched_at_each_point_it_is_handed() {
    let searched = Obligation {
        strategy: Strategy::Interleave,
        ..over_a_bool("m.halving a choice")
    };
    let domain = ValueDomain::Enumerated {
        domain: Symbol::new("Bool"),
        points: 2,
        kept: 2,
    };
    let discharge = with_prover(|prover| {
        prover.searched(
            &searched,
            &ProvePlan::default(),
            bools(&[true, false]),
            domain,
        )
    });
    let Discharge::Held(Evidence::Cases(report)) = &discharge else {
        panic!("a search that scheduled nothing proves nothing: {discharge:?}");
    };
    assert_eq!((report.generated, report.kept, report.rejected), (2, 2, 0));
}

#[test]
fn a_raise_in_an_interleaving_search_is_a_gap_at_its_point() {
    let searched = Obligation {
        strategy: Strategy::Interleave,
        ..over_a_bool("m.dividing by a choice")
    };
    let domain = ValueDomain::Enumerated {
        domain: Symbol::new("Bool"),
        points: 2,
        kept: 2,
    };
    let discharge = with_prover(|prover| {
        prover.searched(
            &searched,
            &ProvePlan::default(),
            bools(&[true, false]),
            domain,
        )
    });
    let Discharge::Unattempted(Gap::Raised { bindings, .. }) = &discharge else {
        panic!("the program's raise was reported as {discharge:?}");
    };
    assert_eq!(bindings[0].value, Plain::Bool(false), "{bindings:?}");
}
