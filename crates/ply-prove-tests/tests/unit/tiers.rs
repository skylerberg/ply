//! What each search's evidence establishes. Whether a claim is filed, under which key, and whether
//! a run of them fails are the program's rules, pinned beside them in `crates/ply-prove/ply` and
//! `crates/ply-cli/ply`.

use ply_eval::{Exploration, Plan, Seed, SimMode};
use ply_prove::{
    CaseReport, Certificate, Evidence, MIN_PROPERTY_CASES, Rule, Tier, interleaving_proves,
};

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
    // Exhaustive over schedules says nothing about the values that were sampled.
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
