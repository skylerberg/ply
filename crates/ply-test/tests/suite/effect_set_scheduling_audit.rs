//! What an over-broad `effect set` costs the scheduler — the required tests's required test 8, and the
//! other half of "an alias is annotation-only".

use crate::fixture::Compiled;
use ply_core::Footprint;
use ply_test::{group_by_conflict, shared_footprint};

impl Compiled {
    fn groups(&self) -> Vec<Vec<usize>> {
        let scheduled: Vec<(usize, Footprint)> =
            self.footprints().into_iter().enumerate().collect();
        group_by_conflict(&scheduled)
    }
}

const STORE: &str = "\
effect store {
  read  all[r]() -> Int
  write save[r](n: Int) -> Unit
}
";

/// Two endpoints, one reading `items` and one writing `orders`, and one test reaching each.
const PRECISE: &str = "\
fn list_items() -> Int / {store.read[items]} = store.all[items]()

fn place_order() -> Int / {store.write[orders]} { store.save[orders](1); 2 }

test \"items\" { assert_eq(list_items(), 0) }

test \"orders\" { assert_eq(place_order(), 2) }
";

/// The same two endpoints, both annotated with one set that covers the union.
const ALIASED: &str = "\
effect set Desk = {store.read[items], store.write[orders]}

fn list_items() -> Int / {Desk} = store.all[items]()

fn place_order() -> Int / {Desk} { store.save[orders](1); 2 }

test \"items\" { assert_eq(list_items(), 0) }

test \"orders\" { assert_eq(place_order(), 2) }
";

#[test]
fn precise_rows_let_two_disjoint_endpoints_run_side_by_side() {
    let compiled = Compiled::anonymous(&format!("{STORE}{PRECISE}"));
    let footprints = compiled.footprints();
    assert!(
        !footprints[0].conflicts_with(&footprints[1]),
        "a reader of `items` and a writer of `orders` share no resource: {footprints:?}"
    );
    assert_eq!(
        compiled.groups(),
        vec![vec![0, 1]],
        "one group is one round of concurrent tests"
    );
}

/// The headline: one set, two endpoints, and now they cannot share a round.
#[test]
fn one_over_broad_set_serialises_two_endpoints_that_do_not_contend() {
    let compiled = Compiled::anonymous(&format!("{STORE}{ALIASED}"));
    let footprints = compiled.footprints();
    assert!(
        footprints[0].conflicts_with(&footprints[1]),
        "both tests now publish `store.write[orders]`: {footprints:?}"
    );
    assert_eq!(
        compiled.groups().len(),
        2,
        "two rounds where the precise rows needed one: {:?}",
        compiled.groups()
    );
}

/// And the atoms that did it are nameable, which is what makes the cost a finding rather than a
/// mystery.
#[test]
fn the_atoms_that_serialised_them_are_the_expansions_and_not_a_name() {
    let compiled = Compiled::anonymous(&format!("{STORE}{ALIASED}"));
    let footprints = compiled.footprints();
    let atoms: Vec<String> = shared_footprint(&footprints[0])
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
    let precise_atoms: Vec<String> = shared_footprint(&precise.footprints()[0])
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

/// The claim the cost of an over-broad alias makes about *why* it is only a cost and never a soundness defect: the
/// widening is upward.
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
