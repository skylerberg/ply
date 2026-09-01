//! What a seeded test is cached under.

use ply_eval::{Plan, Seed};
use ply_hash::DefHash;

/// Domain tags, so a derived key cannot collide with a definition's own hash, which is `blake3`
/// over normalized bytes carrying no tag.
const PLAN_DOMAIN: &[u8] = b"ply.sim.key.1";
const SEED_DOMAIN: &[u8] = b"ply.sim.seed.1";

/// The cache key of a seeded test: its definitions and the whole plan that was searched.
pub fn sim_key(test_hash: DefHash, plan: &Plan) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PLAN_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&plan.digest());
    DefHash(*hasher.finalize().as_bytes())
}

/// The per-root key `random` mode additionally writes, so that widening a root set runs only the
/// roots that are new.
pub fn seed_key(test_hash: DefHash, seed: &Seed) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SEED_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&seed.to_bytes());
    DefHash(*hasher.finalize().as_bytes())
}

/// Whether a plan's per-root results may be cached individually.
pub fn writes_seed_keys(plan: &Plan) -> bool {
    plan.mode.caches_per_seed()
        && plan.budget == 1
        && plan.steps == ply_eval::sim::DEFAULT_STEPS
        && plan.path.is_empty()
}

/// The key a test's result belongs under.
pub fn result_key(test_hash: DefHash, seeded: bool, plan: &Plan) -> DefHash {
    if seeded {
        sim_key(test_hash, plan)
    } else {
        test_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::SimMode;

    fn hash(byte: u8) -> DefHash {
        DefHash([byte; 32])
    }

    #[test]
    fn an_unseeded_test_keeps_its_own_hash() {
        let plan = Plan::default();
        assert_eq!(result_key(hash(1), false, &plan), hash(1));
    }

    /// The rule that stops a run under one plan from reading a pass another plan earned, and the
    /// one whose absence is silent.
    #[test]
    fn a_seeded_test_is_never_keyed_by_its_bare_hash() {
        let plan = Plan::default();
        let key = result_key(hash(1), true, &plan);
        assert_ne!(key, hash(1));
        assert_eq!(key, sim_key(hash(1), &plan));
    }

    #[test]
    fn widening_the_search_changes_the_key() {
        let narrow = Plan::default();
        let wide = Plan {
            budget: narrow.budget * 2,
            ..narrow.clone()
        };
        let more_roots = Plan {
            roots: vec![0, 1],
            ..narrow.clone()
        };
        assert_ne!(sim_key(hash(1), &narrow), sim_key(hash(1), &wide));
        assert_ne!(sim_key(hash(1), &narrow), sim_key(hash(1), &more_roots));
    }

    #[test]
    fn the_key_separates_definitions_from_the_plan() {
        let plan = Plan::default();
        assert_ne!(sim_key(hash(1), &plan), sim_key(hash(2), &plan));
    }

    /// Both derived keys are domain-tagged, so no plan and no seed can produce the hash of some
    /// other test's definitions.
    #[test]
    fn the_two_derived_namespaces_do_not_meet() {
        let plan = Plan::once(Seed::root(7));
        assert_ne!(sim_key(hash(1), &plan), seed_key(hash(1), &Seed::root(7)));
    }

    #[test]
    fn each_root_gets_its_own_key() {
        assert_ne!(
            seed_key(hash(1), &Seed::root(7)),
            seed_key(hash(1), &Seed::root(8))
        );
        assert_ne!(
            seed_key(hash(1), &Seed::root(7)),
            seed_key(hash(1), &Seed::at(7, vec![1]))
        );
    }

    /// A `dpor` root's exploration does not decompose, so there is no per-root fact to cache and
    /// widening its budget re-runs it.
    #[test]
    fn only_random_mode_caches_per_root() {
        assert!(writes_seed_keys(&Plan::random(4)));
        assert!(!writes_seed_keys(&Plan::default()));
        assert!(!writes_seed_keys(&Plan::once(Seed::root(0))));
        assert_eq!(Plan::default().mode, SimMode::Dpor);
    }

    /// `seed_key` names a test and a seed, so it cannot stand for a claim that also depends on how
    /// long each root was allowed to search.
    #[test]
    fn a_random_plan_that_varies_the_step_budget_does_not_decompose() {
        let deeper = Plan {
            steps: ply_eval::sim::DEFAULT_STEPS * 2,
            ..Plan::random(4)
        };
        assert!(!writes_seed_keys(&deeper));
        assert_ne!(
            sim_key(hash(1), &deeper),
            sim_key(hash(1), &Plan::random(4))
        );
    }
}
