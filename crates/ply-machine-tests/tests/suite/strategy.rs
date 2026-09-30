//! The engine carries out the strategy an obligation is handed. Which search a claim goes to is
//! `proof.world`'s decision, pinned by its own tests; here one claim is handed each strategy and
//! discharged each way, whatever the claim itself would have been decided to be.

use crate::fixture::{loaded, project};
use ply_eval::{DefHash, SourceId, Span, Symbol, Value, codes};
use ply_machine::engine::{Point, Prover};
use ply_prove::domain::{Finite, Shape};
use ply_prove::property::Outcome;
use ply_prove::{
    Binder, Certificate, Discharge, Evidence, Gap, Obligation, ObligationKind, Points, ProvePlan,
    Rule, Sort, Static, Strategy, Unsettled, Vacuity, VacuityKind, World,
};

const SOURCE: &str = r#"
law "halving a choice" forall (b: Bool) { (if b { 4 } else { 6 }) / 2 > 1 }

law "excluded middle" forall (b: Bool) { b || !b }

law "doubling is tripling" forall (n: Int) { n + n == n * 3 }

law "concatenation preserves length" forall (a: Bytes, b: Bytes) {
  bytes_len(bytes_concat(a, b)) == bytes_len(a) + bytes_len(b)
}

law "dividing by a choice" forall (b: Bool) { 12 / (if b { 3 } else { 0 }) > 1 }
"#;

/// The claim the static prover leaves open: an uninterpreted `/` over a `Bool`, true at both points.
const HALVING: &str = "m.halving a choice";

/// Left open the same way, and the program divides by zero at `false`.
const DIVIDING: &str = "m.dividing by a choice";

fn binder(name: &str, sort: Sort, text: &str) -> Binder {
    Binder {
        name: Symbol::new(name),
        sort,
        text: text.to_string(),
    }
}

/// `owner`, a law of `SOURCE` over `binders`, handed `strategy`.
fn law(owner: &str, binders: Vec<Binder>, strategy: Strategy) -> Obligation {
    Obligation {
        key: DefHash([7; 32]),
        owner: Symbol::new(owner),
        kind: ObligationKind::Law,
        span: Span::DUMMY,
        binders,
        result: None,
        variables: Vec::new(),
        footprint: Some("{m.db.read[users]}".to_string()),
        strategy,
        guards: Vec::new(),
    }
}

fn over_a_bool(owner: &str, strategy: Strategy) -> Obligation {
    law(owner, vec![binder("b", Sort::bool(), "Bool")], strategy)
}

fn over_an_int(owner: &str, strategy: Strategy) -> Obligation {
    law(owner, vec![binder("n", Sort::int(), "Int")], strategy)
}

/// Both points of a `Bool`, as `proof.domain` measures them.
fn both() -> Points {
    Points::Every(Finite {
        shapes: vec![Shape::Scalar {
            name: "Bool".to_string(),
            size: 2,
        }],
        points: 2,
        name: Symbol::new("Bool"),
    })
}

fn unhandled() -> Strategy {
    Strategy::Static(Unsettled::Unhandled("{m.store.read}".to_string()))
}

/// The prover `SOURCE` is discharged with, over a world of no declared types: every binder here is
/// a builtin.
fn with_prover<R>(f: impl FnOnce(&Prover<'_>) -> R) -> R {
    let dir = project(SOURCE);
    let loaded = loaded(dir.path());
    let world = World::default();
    let backend =
        ply_machine::support::prover_backend(&loaded).expect("the program compiles to a tier");
    f(&Prover::new(&loaded, &world, backend))
}

/// Discharged with nothing settled statically, so the strategy is all there is.
fn discharged(obligation: &Obligation) -> Discharge {
    settled_as(obligation, &Static::Inconclusive)
}

fn settled_as(obligation: &Obligation, settled: &Static) -> Discharge {
    with_prover(|prover| prover.discharge_with(obligation, &ProvePlan::default(), settled))
}

fn certificate() -> Certificate {
    Certificate {
        rules: vec![Rule::Propositional],
        steps: 3,
        guard_satisfiable: true,
        sorts: Vec::new(),
    }
}

#[test]
fn a_claim_handed_every_point_is_proved_by_walking_them() {
    let discharge = discharged(&over_a_bool(
        HALVING,
        Strategy::Static(Unsettled::Run(both())),
    ));
    let Discharge::Held(Evidence::Proof(certificate)) = &discharge else {
        panic!("walking both points proves the claim: {discharge:?}");
    };
    assert_eq!(
        certificate.rules,
        [Rule::ExhaustiveEnumeration {
            domain: Symbol::new("Bool"),
            points: 2,
        }]
    );
}

#[test]
fn the_same_claim_handed_a_sample_is_sampled() {
    let discharge = discharged(&over_a_bool(
        HALVING,
        Strategy::Static(Unsettled::Run(Points::Drawn)),
    ));
    let Discharge::Held(Evidence::Cases(report)) = &discharge else {
        panic!("a sample is evidence of cases: {discharge:?}");
    };
    assert_eq!(report.generated, ProvePlan::default().cases);
    assert_eq!(report.kept, report.generated);
}

#[test]
fn a_claim_the_static_prover_leaves_open_is_the_gap_its_strategy_names() {
    let discharge = discharged(&over_a_bool(HALVING, unhandled()));
    assert!(
        matches!(
            &discharge,
            Discharge::Unattempted(Gap::UnhandledEffect(Some(row))) if row == "{m.store.read}"
        ),
        "{discharge:?}"
    );
}

/// A binder the law does not take makes the tier decline every entry: Ply's failure, never a gap.
#[test]
fn a_proposition_whose_entry_the_tier_declines_is_plys_failure_and_not_a_gap() {
    let mismatched = law(
        HALVING,
        vec![
            binder("b", Sort::bool(), "Bool"),
            binder("spare", Sort::bool(), "Bool"),
        ],
        Strategy::Static(Unsettled::Run(Points::Drawn)),
    );
    with_prover(|prover| {
        let plan = ProvePlan::default();
        let discharge = prover.discharge_with(&mismatched, &plan, &Static::Inconclusive);
        let Discharge::Faulted(fault) = &discharge else {
            panic!("a declined entry was reported as {discharge:?}");
        };
        assert_eq!(fault.diagnostic.code, codes::INTERNAL_ERROR, "{fault:?}");
        assert!(
            fault.diagnostic.message.contains("declined"),
            "{}",
            fault.diagnostic.message
        );
        assert_eq!(
            fault.bindings.len(),
            2,
            "the point it was judging: {fault:?}"
        );
        let point = prover.point_at(&mismatched, 0, 0, &plan);
        assert!(matches!(point, Point::Faulted(_)), "{point:?}");
        let judged = prover.judge_at(&mismatched, &plan, &[Value::Bool(true), Value::Bool(false)]);
        assert!(matches!(judged, Outcome::Faulted(_)), "{judged:?}");
    });
}

/// The control: the program dividing by zero is its own raise, a gap however the points are run.
#[test]
fn a_proposition_that_raises_is_still_a_gap() {
    for points in [both(), Points::Drawn] {
        let discharge = discharged(&over_a_bool(
            DIVIDING,
            Strategy::Static(Unsettled::Run(points)),
        ));
        let Discharge::Unattempted(Gap::Raised {
            bindings,
            diagnostic,
            ..
        }) = &discharge
        else {
            panic!("the program's raise was reported as {discharge:?}");
        };
        assert_eq!(diagnostic.code, codes::RUNTIME_ERROR, "{diagnostic:?}");
        assert_eq!(bindings[0].rendered, "false", "{bindings:?}");
    }
}

/// A proof never calls the owner, so what the static prover settled comes before the gap.
#[test]
fn a_claim_the_static_prover_settles_is_proved_whatever_would_follow() {
    let discharge = settled_as(
        &over_a_bool("m.excluded middle", unhandled()),
        &Static::Proved(certificate()),
    );
    assert!(
        matches!(&discharge, Discharge::Held(Evidence::Proof(c)) if *c == certificate()),
        "{discharge:?}"
    );
}

#[test]
fn a_guard_the_static_prover_refutes_is_vacuous_where_the_guard_is_written() {
    let guard = Span::new(SourceId(0), 3, 8);
    let guarded = Obligation {
        guards: vec![guard],
        ..over_a_bool(HALVING, Strategy::Static(Unsettled::Run(Points::Drawn)))
    };
    let discharge = settled_as(&guarded, &Static::Vacuous);
    assert!(
        matches!(
            &discharge,
            Discharge::Vacuous(Vacuity { guard: at, kind: VacuityKind::ProvedUnsatisfiable })
                if *at == guard
        ),
        "{discharge:?}"
    );
}

/// A proof over a domain the prover could not show inhabited stands once a case is kept there,
/// however the points were run.
#[test]
fn an_unwitnessed_proof_stands_once_a_case_is_kept() {
    for points in [both(), Points::Drawn] {
        let discharge = settled_as(
            &over_a_bool(HALVING, Strategy::Static(Unsettled::Run(points))),
            &Static::NeedsWitness(certificate()),
        );
        assert!(
            matches!(&discharge, Discharge::Held(Evidence::Proof(c)) if *c == certificate()),
            "{discharge:?}"
        );
    }
}

#[test]
fn a_hosted_claim_with_no_host_bound_reaches_the_host() {
    let discharge = discharged(&over_an_int("m.doubling is tripling", Strategy::Hosted));
    assert!(
        matches!(
            &discharge,
            Discharge::Unattempted(Gap::ReachesHost(Some(row))) if row == "{m.db.read[users]}"
        ),
        "{discharge:?}"
    );
}

/// The claim reaches no `simulate` region, so each point is one evaluation, and nothing it scheduled
/// is a proof.
#[test]
fn a_claim_handed_an_interleaving_search_is_searched_at_each_of_its_points() {
    let discharge = discharged(&over_a_bool(HALVING, Strategy::Interleave(both())));
    let Discharge::Held(Evidence::Cases(report)) = &discharge else {
        panic!("a search that scheduled nothing proves nothing: {discharge:?}");
    };
    assert_eq!((report.generated, report.kept, report.rejected), (2, 2, 0));
}

#[test]
fn a_claim_searched_over_interleavings_has_no_point_to_draw() {
    with_prover(|prover| {
        let searched = over_a_bool(HALVING, Strategy::Interleave(Points::Drawn));
        assert!(matches!(
            prover.point_at(&searched, 0, 0, &ProvePlan::default()),
            Point::Undrawn(Gap::NotDrawn)
        ));
    });
}

#[test]
fn a_point_of_a_claim_its_strategy_leaves_a_gap_is_that_gap() {
    with_prover(|prover| {
        let point = prover.point_at(
            &over_a_bool(HALVING, unhandled()),
            0,
            0,
            &ProvePlan::default(),
        );
        assert!(
            matches!(
                &point,
                Point::Undrawn(Gap::UnhandledEffect(Some(row))) if row == "{m.store.read}"
            ),
            "{point:?}"
        );
        let hosted = prover.point_at(
            &over_an_int("m.doubling is tripling", Strategy::Hosted),
            0,
            0,
            &ProvePlan::default(),
        );
        assert!(
            matches!(hosted, Point::Undrawn(Gap::ReachesHost(_))),
            "{hosted:?}"
        );
    });
}

/// The point a refutation came from, re-run at its own root and case, draws the same values the
/// search saw before anything shrank them.
#[test]
fn a_refutation_is_re_run_at_the_root_and_case_it_came_from() {
    with_prover(|prover| {
        let obligation = over_an_int(
            "m.doubling is tripling",
            Strategy::Static(Unsettled::Run(Points::Drawn)),
        );
        let Discharge::Refuted(counterexample) =
            prover.discharge_with(&obligation, &ProvePlan::default(), &Static::Inconclusive)
        else {
            panic!("a false law over `Int` must be refuted, not skipped");
        };
        let original: Vec<String> = counterexample
            .original
            .iter()
            .map(|b| b.rendered.clone())
            .collect();
        let (root, case) = (counterexample.root, counterexample.case);
        match prover.point_at(&obligation, root, case, &ProvePlan::default()) {
            Point::Falsified(bindings) => {
                let drawn: Vec<String> = bindings.iter().map(|b| b.rendered.clone()).collect();
                assert_eq!(
                    drawn, original,
                    "the point was not the one the counterexample came from"
                );
            }
            other => panic!("case {case} of root {root} re-ran as {other:?}"),
        }
    });
}

#[test]
fn a_law_that_holds_has_no_point_that_falsifies_it() {
    with_prover(|prover| {
        let obligation = law(
            "m.concatenation preserves length",
            vec![
                binder("a", Sort::bytes(), "Bytes"),
                binder("b", Sort::bytes(), "Bytes"),
            ],
            Strategy::Static(Unsettled::Run(Points::Drawn)),
        );
        let mut kept = 0;
        for case in 0..64 {
            match prover.point_at(&obligation, 0, case, &ProvePlan::default()) {
                Point::Falsified(bindings) => {
                    panic!("case {case} falsifies a law that holds: {bindings:?}")
                }
                Point::Kept(_) => kept += 1,
                _ => {}
            }
        }
        assert!(kept > 0, "no draw was admitted: the search saw nothing");
    });
}
