use ply_eval::{Plan, Seed, SimMode};
use ply_hash::DefHash;
use ply_test::key::{Engine, SEED_DOMAIN, result_key, seed_key, sim_key};

fn hash(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

fn backend() -> Engine {
    Engine::backend("c")
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
        result_key(hash(1), false, &plan, &Engine::backend("c")),
        result_key(hash(1), false, &plan, &Engine::backend("c:wrong:stale"))
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
