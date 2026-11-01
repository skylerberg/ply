use ply_core::sim::*;

#[test]
fn a_seed_prints_as_its_root_number_and_then_a_colon_and_its_dotted_path_when_the_path_is_not_empty()
 {
    assert_eq!(Seed::at(7, Vec::new()).to_string(), "7");
    assert_eq!(Seed::at(0, vec![1, 0, 2]).to_string(), "0:1.0.2");
    assert_eq!(
        Seed::at(u64::MAX, vec![3]).to_string(),
        "18446744073709551615:3"
    );
}
