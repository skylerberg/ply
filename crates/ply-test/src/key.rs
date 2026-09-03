//! What a test's result is cached under: its definitions, the plan searched, and the engine that
//! answered.

use ply_eval::{Plan, Seed};
use ply_hash::DefHash;

/// Domain tags, so a derived key cannot collide with a definition's own hash, which is `blake3`
/// over normalized bytes carrying no tag.
const PLAN_DOMAIN: &[u8] = b"ply.sim.key.1";
const SEED_DOMAIN: &[u8] = b"ply.sim.seed.1";
const ENGINE_DOMAIN: &[u8] = b"ply.engine.key.1";

/// Which execution strategy answered, and therefore whose claim a stored `Pass` is.
///
/// A `Pass` says a test passed *under some engine*. The evaluator is authoritative and keeps the
/// bare key, so every cache written before engines were told apart stays readable and an
/// unbacked run is unaffected. Every other engine answers in a namespace of its own: a pass it
/// earns is true of it and says nothing about the evaluator, and a pass the evaluator earned says
/// nothing about it. That is what lets a backed run select against its own history instead of
/// running everything, without a backend's answer ever being mistaken for the evaluator's.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Evaluator,
    /// A backend, tagged by everything that decides what it answers — its name, and whatever else
    /// would make a pass under one configuration untrue of another under the same name.
    Backend(String),
}

impl Engine {
    pub fn backend(tag: impl Into<String>) -> Engine {
        Engine::Backend(tag.into())
    }

    /// The engine a run with this backend records under and selects against.
    ///
    /// `name` and `variant` are the provider's — [`ply_eval::Provider::name`] and
    /// [`ply_eval::Provider::variant`]. They are arguments rather than read from a provider so
    /// that a command which must name the engine *before* it builds one — every command that
    /// selects tests, since selection decides whether a provider is worth building at all — gets
    /// the answer the run will give.
    pub fn of_backend(name: &str, variant: &str, spec: &ply_eval::BackendSpec) -> Engine {
        let mut tag = name.to_string();
        if !variant.is_empty() {
            tag.push(':');
            tag.push_str(variant);
        }
        // A backend that is wrong on purpose gets a namespace of its own, so that a corrupted run
        // can never write where an honest one reads even if a caller hands it a real store. The
        // command refuses it a store at all; this is the belt to that pair of braces.
        if spec.mutation != ply_eval::backend::Mutation::None || spec.target.is_some() {
            tag.push_str(&format!("/wrong:{:?}:{:?}", spec.mutation, spec.target));
        }
        Engine::Backend(tag)
    }

    /// How this engine names itself in a report.
    pub fn label(&self) -> &str {
        match self {
            Engine::Evaluator => "evaluator",
            Engine::Backend(tag) => tag,
        }
    }

    pub fn is_evaluator(&self) -> bool {
        matches!(self, Engine::Evaluator)
    }

    /// `key` as this engine's own, which for the evaluator is `key` itself.
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
pub fn seed_key(test_hash: DefHash, seed: &Seed, engine: &Engine) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SEED_DOMAIN);
    hasher.update(&test_hash.0);
    hasher.update(&seed.to_bytes());
    engine.under(DefHash(*hasher.finalize().as_bytes()))
}

/// Whether a plan's per-root results may be cached individually.
pub fn writes_seed_keys(plan: &Plan) -> bool {
    plan.mode.caches_per_seed()
        && plan.budget == 1
        && plan.steps == ply_eval::sim::DEFAULT_STEPS
        && plan.path.is_empty()
}

/// The key a test's result belongs under.
pub fn result_key(test_hash: DefHash, seeded: bool, plan: &Plan, engine: &Engine) -> DefHash {
    engine.under(if seeded {
        sim_key(test_hash, plan)
    } else {
        test_hash
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::SimMode;

    fn hash(byte: u8) -> DefHash {
        DefHash([byte; 32])
    }

    fn backend() -> Engine {
        Engine::backend("cranelift")
    }

    #[test]
    fn an_unseeded_test_keeps_its_own_hash() {
        let plan = Plan::default();
        assert_eq!(
            result_key(hash(1), false, &plan, &Engine::Evaluator),
            hash(1)
        );
    }

    /// The rule that stops a run under one plan from reading a pass another plan earned, and the
    /// one whose absence is silent.
    #[test]
    fn a_seeded_test_is_never_keyed_by_its_bare_hash() {
        let plan = Plan::default();
        let key = result_key(hash(1), true, &plan, &Engine::Evaluator);
        assert_ne!(key, hash(1));
        assert_eq!(key, sim_key(hash(1), &plan));
    }

    /// The same rule one axis over: a backend's pass is a claim about the backend, so it may not be
    /// read as the evaluator's and the evaluator's may not be read as its.
    #[test]
    fn an_engine_never_reads_another_engines_key() {
        let plan = Plan::default();
        for seeded in [false, true] {
            let evaluator = result_key(hash(1), seeded, &plan, &Engine::Evaluator);
            let backed = result_key(hash(1), seeded, &plan, &backend());
            assert_ne!(evaluator, backed);
        }
        assert_ne!(
            seed_key(hash(1), &Seed::root(0), &Engine::Evaluator),
            seed_key(hash(1), &Seed::root(0), &backend())
        );
    }

    /// Two backends are two engines, so one's pass is not the other's either.
    #[test]
    fn two_backends_are_two_namespaces() {
        let plan = Plan::default();
        assert_ne!(
            result_key(hash(1), false, &plan, &Engine::backend("cranelift")),
            result_key(hash(1), false, &plan, &Engine::backend("reference"))
        );
    }

    /// The evaluator's keys are the ones this cache has always used, so adding engines reads every
    /// cache written before them rather than orphaning it.
    #[test]
    fn the_evaluator_keeps_the_keys_the_cache_already_holds() {
        let plan = Plan {
            mode: SimMode::Random,
            ..Plan::default()
        };
        assert_eq!(
            result_key(hash(7), true, &plan, &Engine::Evaluator),
            sim_key(hash(7), &plan)
        );
        assert_eq!(seed_key(hash(7), &Seed::root(3), &Engine::Evaluator), {
            let mut hasher = blake3::Hasher::new();
            hasher.update(SEED_DOMAIN);
            hasher.update(&hash(7).0);
            hasher.update(&Seed::root(3).to_bytes());
            DefHash(*hasher.finalize().as_bytes())
        });
    }
}
