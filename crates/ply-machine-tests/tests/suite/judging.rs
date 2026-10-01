//! The runtime's half of a discharge: a claim's points judged as the program asks, and a law over
//! interleavings searched at the points its guard kept. Which points those are, and what their
//! judgements come to, is `proof.property`'s, pinned by its own tests.

use crate::fixture::{loaded, project};
use ply_eval::{Seed, Span, Symbol, Value, codes};
use ply_machine::engine::{Binder, Judgement, Mode, Obligation, ObligationKind, Prover, Strategy};

const SOURCE: &str = r#"
law "halving a choice" forall (b: Bool) { (if b { 4 } else { 6 }) / 2 > 1 }

law "doubling is tripling" forall (n: Int) { n + n == n * 3 }

law "dividing by a choice" forall (b: Bool) { 12 / (if b { 3 } else { 0 }) > 1 }

law "past a hundred" forall (n: Int) where n > 100 { n > 0 }

law "a guarded quotient" forall (n: Int) where 12 / n > 0 { true }

law "a quotient past zero" forall (n: Int) where n != 0 { 12 / n != 99 }

law "a region joins what it spawned" forall (b: Bool) {
  simulate {
    let t = task.spawn(|| if b { 1 } else { 2 });
    task.join(t) > 0
  }
}

fn capped(n: Int) -> Int
  requires n < 1000
  ensures result <= 1000
  ensures result == n
= if n > 10 { 10 } else { n }
"#;

fn binder(name: &str, text: &str) -> Binder {
    Binder {
        name: Symbol::new(name),
        text: text.to_string(),
    }
}

fn claim(owner: &str, kind: ObligationKind, binders: Vec<Binder>, guards: usize) -> Obligation {
    Obligation {
        owner: Symbol::new(owner),
        kind,
        span: Span::DUMMY,
        binders,
        result: None,
        strategy: Strategy::Static,
        guards: vec![Span::DUMMY; guards],
    }
}

fn over_a_bool(owner: &str) -> Obligation {
    claim(owner, ObligationKind::Law, vec![binder("b", "Bool")], 0)
}

fn over_an_int(owner: &str, guards: usize) -> Obligation {
    claim(owner, ObligationKind::Law, vec![binder("n", "Int")], guards)
}

/// `capped`'s `ensures` clause `index`, under its one `requires`.
fn capped(index: usize) -> Obligation {
    Obligation {
        result: Some(binder("result", "Int")),
        ..claim(
            "m.capped",
            ObligationKind::Ensures { index },
            vec![binder("n", "Int")],
            1,
        )
    }
}

fn with_prover<R>(f: impl FnOnce(&Prover) -> R) -> R {
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
    with_prover(|prover| prover.judged(obligation, ply_eval::DEFAULT_STEP_BUDGET, points, mode))
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
        vec![binder("b", "Bool"), binder("spare", "Bool")],
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

/// A law that reaches no `simulate` region has no schedule: the run says so, and its body's verdict.
#[test]
fn a_run_that_reaches_no_region_is_unobserved_and_keeps_its_verdict() {
    let law = Obligation {
        strategy: Strategy::Interleave,
        ..over_a_bool("m.halving a choice")
    };
    let run = with_prover(|prover| {
        prover.interleaved(
            &law,
            ply_eval::DEFAULT_STEP_BUDGET,
            &[Value::Bool(true)],
            &Seed::at(0, Vec::new()),
            64,
        )
    });
    assert!(!run.observed);
    assert!(run.interleaving.steps.is_empty());
    assert!(run.verdict.is_none(), "{:?}", run.verdict);

    let dividing = Obligation {
        strategy: Strategy::Interleave,
        ..over_a_bool("m.dividing by a choice")
    };
    let run = with_prover(|prover| {
        prover.interleaved(
            &dividing,
            ply_eval::DEFAULT_STEP_BUDGET,
            &[Value::Bool(false)],
            &Seed::at(0, Vec::new()),
            64,
        )
    });
    assert_eq!(
        shown(&[run.verdict.expect("the body raised")]),
        [format!("raised {}", codes::RUNTIME_ERROR)]
    );
}

/// A law over a region runs one schedule per call: the seed's, recorded step by step.
#[test]
fn a_run_under_a_seed_records_the_schedule_it_took() {
    let law = Obligation {
        strategy: Strategy::Interleave,
        ..over_a_bool("m.a region joins what it spawned")
    };
    let run = with_prover(|prover| {
        prover.interleaved(
            &law,
            ply_eval::DEFAULT_STEP_BUDGET,
            &[Value::Bool(true)],
            &Seed::at(3, Vec::new()),
            64,
        )
    });
    assert!(run.observed);
    assert!(run.verdict.is_none(), "{:?}", run.verdict);
    assert!(!run.interleaving.steps.is_empty());
    for step in &run.interleaving.steps {
        assert_eq!(step.enabled.get(usize::from(step.choice)), Some(&step.task));
    }
    // The same seed takes the same schedule.
    let again = with_prover(|prover| {
        prover.interleaved(
            &law,
            ply_eval::DEFAULT_STEP_BUDGET,
            &[Value::Bool(true)],
            &Seed::at(3, Vec::new()),
            64,
        )
    });
    assert_eq!(again.interleaving.steps, run.interleaving.steps);
}
