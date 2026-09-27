//! Cache keys for test results: the definitions, and the plan that was searched.

use ply_eval::{Plan, Seed};
use ply_ty::DefHash;

/// Domain tags, so a derived key cannot collide with a definition's own untagged hash.
const PLAN_DOMAIN: &[u8] = b"ply.sim.key.1";
pub const SEED_DOMAIN: &[u8] = b"ply.sim.seed.1";

/// A seeded test's key: its definitions and the whole plan that was searched.
pub fn sim_key(test_hash: DefHash, plan: &Plan) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PLAN_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&plan.digest());
    DefHash(*hasher.finalize().as_bytes())
}

/// The per-root key `random` mode also writes, so widening a root set runs only the new roots.
pub fn seed_key(test_hash: DefHash, seed: &Seed) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SEED_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&seed.to_bytes());
    DefHash(*hasher.finalize().as_bytes())
}

pub fn writes_seed_keys(plan: &Plan) -> bool {
    plan.mode.caches_per_seed()
        && plan.budget == 1
        && plan.steps == ply_eval::sim::DEFAULT_STEPS
        && plan.path.is_empty()
}

pub fn result_key(test_hash: DefHash, seeded: bool, plan: &Plan) -> DefHash {
    if seeded {
        sim_key(test_hash, plan)
    } else {
        test_hash
    }
}
