//! What an effect set leaves in the footprints a scheduler colours: `suite.schedule` separates two
//! tests exactly when their atoms conflict, so the atoms are what these hold to real programs.

use crate::fixture::Compiled;

const STORE: &str = "\
effect store {
  read  all[r]() -> Int
  write save[r](n: Int) -> Unit
}
";

const PRECISE: &str = "\
fn list_items() -> Int / {store.read[items]} = store.all[items]()

fn place_order() -> Int / {store.write[orders]} { store.save[orders](1); 2 }

test \"items\" { assert_eq(list_items(), 0) }

test \"orders\" { assert_eq(place_order(), 2) }
";

const ALIASED: &str = "\
effect set Desk = {store.read[items], store.write[orders]}

fn list_items() -> Int / {Desk} = store.all[items]()

fn place_order() -> Int / {Desk} { store.save[orders](1); 2 }

test \"items\" { assert_eq(list_items(), 0) }

test \"orders\" { assert_eq(place_order(), 2) }
";

#[test]
fn precise_rows_leave_two_disjoint_endpoints_nothing_to_contend_over() {
    let compiled = Compiled::anonymous(&format!("{STORE}{PRECISE}"));
    let footprints = compiled.footprints();
    assert!(
        !footprints[0].conflicts_with(&footprints[1]),
        "a reader of `items` and a writer of `orders` share no resource: {footprints:?}"
    );
}

#[test]
fn one_over_broad_set_makes_two_endpoints_that_do_not_contend_conflict() {
    let compiled = Compiled::anonymous(&format!("{STORE}{ALIASED}"));
    let footprints = compiled.footprints();
    assert!(
        footprints[0].conflicts_with(&footprints[1]),
        "both tests now publish `store.write[orders]`: {footprints:?}"
    );
}

#[test]
fn the_atoms_that_serialised_them_are_the_expansions_and_not_a_name() {
    let compiled = Compiled::anonymous(&format!("{STORE}{ALIASED}"));
    let atoms: Vec<String> = compiled.footprints()[0]
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert!(
        atoms.iter().any(|a| a.ends_with("store.write[orders]")),
        "the test that only reads `items` still publishes the set's write: {atoms:?}"
    );
    assert!(
        !atoms.iter().any(|a| a.contains("Desk")),
        "an alias name reaches no footprint: {atoms:?}"
    );

    let precise = Compiled::anonymous(&format!("{STORE}{PRECISE}"));
    let precise_atoms: Vec<String> = precise.footprints()[0]
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert!(
        !precise_atoms
            .iter()
            .any(|a| a.ends_with("store.write[orders]")),
        "the control must not carry the write: {precise_atoms:?}"
    );
}

#[test]
fn an_alias_only_ever_widens_a_tests_footprint() {
    let precise = Compiled::anonymous(&format!("{STORE}{PRECISE}")).footprints();
    let aliased = Compiled::anonymous(&format!("{STORE}{ALIASED}")).footprints();
    for (before, after) in precise.iter().zip(aliased.iter()) {
        for atom in before.atoms() {
            assert!(
                after.atoms().any(|a| a == atom),
                "the alias dropped `{atom}`, which would be a footprint that \
                 under-reports: {before:?} -> {after:?}"
            );
        }
    }
}
