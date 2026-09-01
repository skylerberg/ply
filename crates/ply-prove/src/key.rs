//! What an obligation's result is cached under.

use crate::{ProvePlan, Tier};
use ply_hash::DefHash;

/// Domain tag, so a plan-keyed result cannot collide with the bare obligation key, which is itself
/// a `blake3` over normalized bytes.
const PLAN_DOMAIN: &[u8] = b"ply.prove.key.1";

/// The key everything weaker than a proof is written under.
pub fn prove_key(key: DefHash, plan: &ProvePlan) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PLAN_DOMAIN);
    hasher.update(&key.0);
    hasher.update(&plan.digest());
    DefHash(*hasher.finalize().as_bytes())
}

/// Where a discharge of this tier belongs.
pub fn result_key(key: DefHash, tier: Option<Tier>, plan: &ProvePlan) -> DefHash {
    match tier {
        Some(Tier::Proved) => key,
        _ => prove_key(key, plan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> DefHash {
        DefHash([7; 32])
    }

    /// The rule that stops a run under one plan from reading a discharge another plan earned, and
    /// the one whose absence is silent.
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

    /// A wider shrink budget can only produce a smaller counterexample, and failures are never
    /// cached, so it cannot change a cached claim.
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

    /// An unknown tier takes the plan key: the bare key is the one that survives a widening, so it
    /// is the one nothing may be written under by accident.
    #[test]
    fn an_absent_tier_takes_the_conservative_key() {
        let plan = ProvePlan::default();
        assert_eq!(result_key(key(), None, &plan), prove_key(key(), &plan));
    }
}
