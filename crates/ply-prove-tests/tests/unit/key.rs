use ply_prove::key::{prove_key, result_key};
use ply_prove::{ProvePlan, Tier};
use ply_ty::DefHash;

fn key() -> DefHash {
    DefHash([7; 32])
}

#[test]
fn a_sampled_discharge_is_never_written_under_the_bare_key() {
    let plan = ProvePlan::default();
    for tier in [Tier::Property, Tier::Example] {
        let at = result_key(key(), Some(tier), &plan);
        assert_ne!(at, key());
        assert_eq!(at, prove_key(key(), &plan));
    }
}

#[test]
fn a_proof_is_written_under_the_bare_key_and_survives_a_widening() {
    let narrow = ProvePlan::default();
    let wide = ProvePlan {
        cases: narrow.cases * 8,
        roots: vec![0, 1, 2, 3],
        ..narrow.clone()
    };
    assert_eq!(result_key(key(), Some(Tier::Proved), &narrow), key());
    assert_eq!(result_key(key(), Some(Tier::Proved), &wide), key());
}

#[test]
fn widening_the_plan_changes_where_a_sample_is_read_from() {
    let narrow = ProvePlan::default();
    let wider_cases = ProvePlan {
        cases: narrow.cases * 2,
        ..narrow.clone()
    };
    let more_roots = ProvePlan {
        roots: vec![0, 1],
        ..narrow.clone()
    };
    let deeper = ProvePlan {
        prove_budget: narrow.prove_budget * 2,
        ..narrow.clone()
    };
    assert_ne!(prove_key(key(), &narrow), prove_key(key(), &wider_cases));
    assert_ne!(prove_key(key(), &narrow), prove_key(key(), &more_roots));
    assert_ne!(prove_key(key(), &narrow), prove_key(key(), &deeper));
}

/// Failures are never cached, so the shrink budget cannot change a cached claim.
#[test]
fn the_shrink_budget_does_not_move_a_key() {
    let narrow = ProvePlan::default();
    let looser = ProvePlan {
        shrink_budget: narrow.shrink_budget * 4,
        ..narrow.clone()
    };
    assert_eq!(prove_key(key(), &narrow), prove_key(key(), &looser));
}

#[test]
fn the_key_separates_obligations() {
    let plan = ProvePlan::default();
    assert_ne!(prove_key(DefHash([1; 32]), &plan), prove_key(key(), &plan));
}

#[test]
fn an_absent_tier_takes_the_conservative_key() {
    let plan = ProvePlan::default();
    assert_eq!(result_key(key(), None, &plan), prove_key(key(), &plan));
}
