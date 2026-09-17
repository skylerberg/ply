use ply_eval::{Exploration, Plan, Seed, SimMode};
use ply_prove::{
    CaseReport, Certificate, Counterexample, Coverage, Discharge, Evidence, Frame, Gap,
    MIN_PROPERTY_CASES, Obligation, ObligationKind, ProvePlan, ProveReport, Rule, Tier, Vacuity,
    VacuityKind, frame_of, interleaving_proves,
};
use ply_span::Span;
use ply_ty::DefHash;
use ply_ty::Mode;
use ply_ty::{EffectAtom, Footprint, Resource};
use std::time::Duration;

fn cases(kept: u32) -> Evidence {
    Evidence::Cases(CaseReport {
        generated: 200,
        kept,
        rejected: 200 - kept,
        roots: vec![0],
        instantiations: Vec::new(),
    })
}

fn certificate() -> Certificate {
    Certificate {
        rules: vec![Rule::LinearArithmetic],
        steps: 41,
        guard_satisfiable: true,
        sorts: Vec::new(),
    }
}

/// The rule the whole module exists to make unavailable to get wrong: a tier is computed from
/// the evidence, and only a certificate computes to `Proved`.
#[test]
fn only_a_certificate_yields_proved() {
    assert_eq!(Evidence::Proof(certificate()).tier(), Tier::Proved);
    for kept in [0, 1, 24, 25, 200] {
        assert_ne!(cases(kept).tier(), Tier::Proved);
    }
}

#[test]
fn the_kept_count_alone_separates_property_from_example() {
    assert_eq!(cases(MIN_PROPERTY_CASES).tier(), Tier::Property);
    assert_eq!(cases(MIN_PROPERTY_CASES - 1).tier(), Tier::Example);
    assert_eq!(cases(0).tier(), Tier::Example);
}

/// `min` on two tiers is "report the weaker", so the ordering has to be the strength ordering
/// and not the declaration order it happens to share.
#[test]
fn tiers_order_by_strength() {
    assert!(Tier::Example < Tier::Property);
    assert!(Tier::Property < Tier::Proved);
    assert_eq!(Tier::Proved.min(Tier::Example), Tier::Example);
}

#[test]
fn nothing_but_a_hold_has_a_tier_or_is_cached() {
    let outcomes = [
        Discharge::Refuted(Counterexample {
            bindings: Vec::new(),
            original: Vec::new(),
            shrinks: 0,
            root: 0,
            case: 0,
            race: None,
            sim_seed: None,
        }),
        Discharge::Vacuous(Vacuity {
            guard: Span::DUMMY,
            kind: VacuityKind::ProvedUnsatisfiable,
        }),
        Discharge::Unattempted(Gap::UnhandledEffect(Footprint::empty())),
    ];
    for outcome in outcomes {
        assert_eq!(outcome.tier(), None);
        assert!(!outcome.holds());
        assert!(!outcome.is_cacheable());
        assert!(!outcome.is_plan_independent());
    }
}

/// The asymmetry the whole operational value of `proved` rests on: a proof is valid under every
/// plan, a sample is a claim about the plan that took it.
#[test]
fn only_a_proof_is_plan_independent() {
    assert!(Discharge::Held(Evidence::Proof(certificate())).is_plan_independent());
    assert!(!Discharge::Held(cases(200)).is_plan_independent());
    assert!(!Discharge::Held(cases(3)).is_plan_independent());
}

#[test]
fn a_frame_names_the_writes_and_nothing_else() {
    assert_eq!(frame_of(&Footprint::empty()), Frame::Pure);

    let read = EffectAtom::new("db", Resource::Named("users".into()), Mode::Read);
    assert_eq!(
        frame_of(&Footprint::from_atoms([read.clone()])),
        Frame::Pure,
        "a read changes nothing, so it does not narrow a frame"
    );

    let write = EffectAtom::new("db", Resource::Named("orders".into()), Mode::Write);
    let Frame::Writes(writes) = frame_of(&Footprint::from_atoms([read, write])) else {
        panic!("a write must produce a frame");
    };
    assert_eq!(writes.len(), 1);
    assert!(writes.contains(&("db".into(), Resource::Named("orders".into()))));
}

fn exploration(exhaustive: bool, exhausted: bool) -> Exploration {
    Exploration {
        explored: 12,
        exhaustive,
        exhausted,
        naive: None,
        steps: 40,
        virtual_time: 0,
        failure: None,
        race: None,
    }
}

#[test]
fn an_exhaustive_search_proves_only_when_the_value_domain_was_covered_too() {
    let plan = Plan::default();
    assert_eq!(plan.mode, SimMode::Dpor);
    assert!(interleaving_proves(&plan, &exploration(true, false), true));
    // The condition an implementer drops: exhaustive over *schedules* says nothing about the
    // values that were sampled.
    assert!(!interleaving_proves(
        &plan,
        &exploration(true, false),
        false
    ));
}

#[test]
fn a_sampled_or_spent_search_never_proves() {
    let exhaustive = exploration(true, false);
    assert!(!interleaving_proves(&Plan::random(4), &exhaustive, true));
    assert!(!interleaving_proves(
        &Plan::once(Seed::root(7)),
        &exhaustive,
        true
    ));
    assert!(!interleaving_proves(
        &Plan::default(),
        &exploration(false, true),
        true
    ));
}

#[test]
fn a_failing_search_never_proves() {
    let mut failed = exploration(true, false);
    failed.failure = Some(Seed::root(3));
    assert!(!interleaving_proves(&Plan::default(), &failed, true));
}

#[test]
fn a_plan_digest_ignores_the_shrink_budget_and_root_spelling() {
    let base = ProvePlan::default();
    let looser = ProvePlan {
        shrink_budget: base.shrink_budget * 4,
        ..base.clone()
    };
    assert_eq!(base.digest(), looser.digest());

    let respelled = ProvePlan {
        roots: vec![2, 0, 2, 1],
        ..base.clone()
    };
    let sorted = ProvePlan {
        roots: vec![0, 1, 2],
        ..base.clone()
    };
    assert_eq!(respelled.digest(), sorted.digest());
    assert_ne!(base.digest(), sorted.digest());

    let wider = ProvePlan {
        cases: base.cases * 2,
        ..base.clone()
    };
    assert_ne!(base.digest(), wider.digest());

    let deeper = ProvePlan {
        prove_budget: base.prove_budget * 2,
        ..base.clone()
    };
    assert_ne!(base.digest(), deeper.digest());
}

#[test]
fn a_report_fails_on_a_refutation_or_a_vacuity_and_not_on_a_gap() {
    let obligation = Obligation {
        key: DefHash([0; 32]),
        owner: "m.f".into(),
        kind: ObligationKind::Ensures { index: 0 },
        span: Span::DUMMY,
        frame: Frame::Pure,
        binders: Vec::new(),
        guarded: false,
        host: false,
        footprint: Footprint::empty(),
    };
    let report = |discharge| ProveReport {
        obligations: vec![(obligation.clone(), discharge)],
        coverage: Coverage::default(),
        plan: ProvePlan::default(),
        cached: 0,
        duration: Duration::ZERO,
    };
    assert!(
        !report(Discharge::Unattempted(Gap::UnhandledEffect(
            Footprint::empty()
        )))
        .failed()
    );
    assert!(
        report(Discharge::Vacuous(Vacuity {
            guard: Span::DUMMY,
            kind: VacuityKind::NoCaseKept { generated: 200 },
        }))
        .failed()
    );
    assert!(!report(Discharge::Held(cases(200))).failed());
}
