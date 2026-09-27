use ply_eval::{Plan, Seed, SimMode};
use ply_test::key::{SEED_DOMAIN, result_key, seed_key, sim_key};
use ply_ty::DefHash;

fn hash(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

#[test]
fn an_unseeded_test_keeps_its_own_hash() {
    let plan = Plan::default();
    assert_eq!(result_key(hash(1), false, &plan), hash(1));
}

#[test]
fn a_seeded_test_is_never_keyed_by_its_bare_hash() {
    let plan = Plan::default();
    let key = result_key(hash(1), true, &plan);
    assert_ne!(key, hash(1));
    assert_eq!(key, sim_key(hash(1), &plan));
}

#[test]
fn a_seeded_plan_keeps_the_keys_the_cache_already_holds() {
    let plan = Plan {
        mode: SimMode::Random,
        ..Plan::default()
    };
    assert_eq!(result_key(hash(7), true, &plan), sim_key(hash(7), &plan));
    assert_eq!(seed_key(hash(7), &Seed::root(3)), {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SEED_DOMAIN);
        hasher.update(&hash(7).0);
        hasher.update(&Seed::root(3).to_bytes());
        DefHash(*hasher.finalize().as_bytes())
    });
}
