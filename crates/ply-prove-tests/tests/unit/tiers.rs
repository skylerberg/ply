//! What each search's evidence establishes. Whether a claim is filed, under which key, and whether
//! a run of them fails are the program's rules, pinned beside them in `crates/ply-prove/ply` and
//! `crates/ply-cli/ply`.

use ply_prove::{CaseReport, Certificate, Evidence, MIN_PROPERTY_CASES, Rule, Tier};

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
