//! What an over-broad `effect set` costs the scheduler — ADR 0013 §11's
//! required test 8, and the other half of "an alias is annotation-only".
//!
//! The alias itself reaches nothing here: colouring reads `Footprint`, which is
//! atoms. But a `/ {..}` annotation is the *published* signature, so a set
//! wider than the bodies that carry it publishes atoms those bodies never
//! touch, and two tests reaching two endpoints that share one set contend on
//! every atom in it. ADR 0013 §1.6 states that cost; this file measures it,
//! against a control where the same two endpoints write their rows out and land
//! in two groups.
//!
//! It is written this way round on purpose. The failure worth catching is not
//! "the set widened the graph" — that is the documented consequence — but "the
//! set widened the graph and nobody could see which atoms did it", which is the
//! shape of a scheduling regression nobody attributes.
//!
//! A "group" here is what `group_by_conflict` produces: one round of tests that
//! may run at once. Two tests that contend therefore land in *different*
//! groups, and more groups is less concurrency.

use ply_core::{CheckOutput, Footprint};
use ply_span::SourceId;
use ply_syntax::resolve::Resolved;
use ply_test::{group_by_conflict, shared_footprint};

struct Compiled {
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let mut program = ply_syntax::ast::Program::single(module);
        let resolved: Resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled { check }
    }

    fn footprints(&self) -> Vec<Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }

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

/// Two endpoints, one reading `items` and one writing `orders`, and one test
/// reaching each. Written precisely, their footprints are disjoint.
const PRECISE: &str = "\
fn list_items() -> Int / {store.read[items]} = store.all[items]()

fn place_order() -> Int / {store.write[orders]} { store.save[orders](1); 2 }

test \"items\" { assert_eq(list_items(), 0) }

test \"orders\" { assert_eq(place_order(), 2) }
";

/// The same two endpoints, both annotated with one set that covers the union.
/// Neither body changed.
const ALIASED: &str = "\
effect set Desk = {store.read[items], store.write[orders]}

fn list_items() -> Int / {Desk} = store.all[items]()

fn place_order() -> Int / {Desk} { store.save[orders](1); 2 }

test \"items\" { assert_eq(list_items(), 0) }

test \"orders\" { assert_eq(place_order(), 2) }
";

#[test]
fn precise_rows_let_two_disjoint_endpoints_run_side_by_side() {
    let compiled = Compiled::new(&format!("{STORE}{PRECISE}"));
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
/// The bodies are byte for byte what they were above — only the annotation
/// moved, and the scheduler serialised two tests that could have run together.
#[test]
fn one_over_broad_set_serialises_two_endpoints_that_do_not_contend() {
    let compiled = Compiled::new(&format!("{STORE}{ALIASED}"));
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

/// And the atoms that did it are nameable, which is what makes the cost a
/// finding rather than a mystery. The contended atoms are the set's, not the
/// set's *name* — nothing downstream of the parser has ever heard of `Desk`.
#[test]
fn the_atoms_that_serialised_them_are_the_expansions_and_not_a_name() {
    let compiled = Compiled::new(&format!("{STORE}{ALIASED}"));
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

    let precise = Compiled::new(&format!("{STORE}{PRECISE}"));
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

/// The claim ADR 0013 §1.6 makes about *why* it is only a cost and never a
/// soundness defect: the widening is upward. A test's footprint under an alias
/// is a superset of what it is without one, so nothing the scheduler had to
/// separate is now free to run alongside anything.
#[test]
fn an_alias_only_ever_widens_a_tests_footprint() {
    let precise = Compiled::new(&format!("{STORE}{PRECISE}")).footprints();
    let aliased = Compiled::new(&format!("{STORE}{ALIASED}")).footprints();
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
