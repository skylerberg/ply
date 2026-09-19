//! Cache keys for test results: definitions, the plan searched, and the engine that answered.

use ply_eval::{Plan, Seed};
use ply_ty::DefHash;

/// Domain tags, so a derived key cannot collide with a definition's own untagged hash.
const PLAN_DOMAIN: &[u8] = b"ply.sim.key.1";
pub const SEED_DOMAIN: &[u8] = b"ply.sim.seed.1";
const ENGINE_DOMAIN: &[u8] = b"ply.engine.key.1";

/// Which engine answered, and so whose claim a stored `Pass` is. The evaluator keeps the bare
/// key; every other engine answers in a namespace of its own.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Evaluator,
    /// Tagged by everything that could make a pass under one configuration untrue of another.
    Backend(String),
}

impl Engine {
    pub fn backend(tag: impl Into<String>) -> Engine {
        Engine::Backend(tag.into())
    }

    /// Takes the provider's `name` and `variant` so the engine is named before one is built.
    pub fn of_backend(name: &str, variant: &str, spec: &ply_eval::BackendSpec) -> Engine {
        // A deliberately wrong backend must never write where an honest one reads.
        if spec.mutation == ply_eval::backend::Mutation::None && spec.target.is_none() {
            return Engine::Evaluator;
        }
        let mut tag = name.to_string();
        if !variant.is_empty() {
            tag.push(':');
            tag.push_str(variant);
        }
        tag.push_str(&format!("/wrong:{:?}:{:?}", spec.mutation, spec.target));
        Engine::Backend(tag)
    }

    pub fn label(&self) -> &str {
        match self {
            Engine::Evaluator => "evaluator",
            Engine::Backend(tag) => tag,
        }
    }

    pub fn is_evaluator(&self) -> bool {
        matches!(self, Engine::Evaluator)
    }

    /// `key` in this engine's namespace; the evaluator's is `key` itself.
    fn under(&self, key: DefHash) -> DefHash {
        match self {
            Engine::Evaluator => key,
            Engine::Backend(tag) => {
                let mut hasher = blake3::Hasher::new();
                hasher.update(ENGINE_DOMAIN);
                hasher.update(&key.0);
                hasher.update(tag.as_bytes());
                DefHash(*hasher.finalize().as_bytes())
            }
        }
    }
}

/// A seeded test's key: its definitions and the whole plan that was searched.
pub fn sim_key(test_hash: DefHash, plan: &Plan) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PLAN_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&plan.digest());
    DefHash(*hasher.finalize().as_bytes())
}

/// The per-root key `random` mode also writes, so widening a root set runs only the new roots.
pub fn seed_key(test_hash: DefHash, seed: &Seed, engine: &Engine) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SEED_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&seed.to_bytes());
    engine.under(DefHash(*hasher.finalize().as_bytes()))
}

pub fn writes_seed_keys(plan: &Plan) -> bool {
    plan.mode.caches_per_seed()
        && plan.budget == 1
        && plan.steps == ply_eval::sim::DEFAULT_STEPS
        && plan.path.is_empty()
}

pub fn result_key(test_hash: DefHash, seeded: bool, plan: &Plan, engine: &Engine) -> DefHash {
    engine.under(if seeded {
        sim_key(test_hash, plan)
    } else {
        test_hash
    })
}
