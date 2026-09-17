//! What an obligation's result is cached under.

use crate::{ProvePlan, Tier};
use ply_ty::DefHash;

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
