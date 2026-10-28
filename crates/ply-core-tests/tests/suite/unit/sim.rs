use ply_core::sim::*;

#[test]
fn a_seed_prints_its_root_and_then_its_path() {
    assert_eq!(Seed::at(7, Vec::new()).to_string(), "7");
    assert_eq!(Seed::at(0, vec![1, 0, 2]).to_string(), "0:1.0.2");
    assert_eq!(
        Seed::at(u64::MAX, vec![3]).to_string(),
        "18446744073709551615:3"
    );
}
