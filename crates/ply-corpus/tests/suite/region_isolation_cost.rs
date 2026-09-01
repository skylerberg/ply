//! ADR 0017 §6's number, taken against real projects rather than against footprints somebody typed.

use ply_core::Footprint;
use ply_corpus::regions::{self, Corpus, Hypothetical};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels under the repository root")
        .to_path_buf()
}

/// One test per label, so the counterfactual is a clique of `per_label` tests per label and the
/// group count is exactly `per_label`.
fn cell_project(dir: &Path, cells: usize, labels: usize, pure: usize) {
    let mut src = String::new();
    for i in 0..cells {
        let label = i % labels;
        src.push_str(&format!(
            "fn touch{i}() -> Int / {{cell.read[r{label}], cell.write[r{label}]}} =\n  \
             with_cell[r{label}](0) {{ c -> {{ cell_set(c, {i}); cell_get(c) }} }}\n\n\
             test \"cell test {i}\" {{ assert_eq(touch{i}(), {i}) }}\n\n"
        ));
    }
    for i in 0..pure {
        src.push_str(&format!(
            "test \"pure test {i}\" {{ assert_eq({i} + 1, {}) }}\n",
            i + 1
        ));
    }
    std::fs::create_dir_all(dir).expect("the scratch project directory");
    std::fs::write(dir.join("main.ply"), src).expect("writing the scratch project");
}

/// The mechanism, end to end and on the real runner: a declared `cell` footprint reaches a test,
/// the scheduler calls the test isolated anyway, and the counterfactual is one group per label.
#[test]
fn a_cell_atom_reaches_a_footprint_and_only_forking_hides_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let root = dir.path().join("cells");
    cell_project(&root, 24, 4, 8);

    let corpus = regions::measure(&root, 4, false).expect("the scratch project must run green");
    assert!(
        regions::effects_present(&corpus.footprints).contains("cell"),
        "the corpus was written so that a `cell` atom reaches a footprint"
    );

    let cost = regions::analyse(&corpus, 4);
    assert_eq!(cost.tests, 32);
    assert_eq!(
        cost.isolated_today, 32,
        "forking makes every one of them isolated, which is the property being priced"
    );
    assert_eq!(cost.today.groups, 1);
    assert_eq!(
        cost.without_forking.groups, 6,
        "six tests share each of four labels, so the clique is six colours wide"
    );
    assert_eq!(cost.newly_serialized, 24);
    assert!(
        cost.without_forking.critical_path_millis > cost.today.critical_path_millis,
        "six barriers where there was one has to cost something"
    );
}

/// A label nobody else names conflicts with nothing, so the exemption was buying it nothing.
#[test]
fn cell_tests_on_distinct_labels_are_free_to_lose_the_exemption() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let root = dir.path().join("cells");
    cell_project(&root, 16, 16, 8);

    let corpus = regions::measure(&root, 4, false).expect("the scratch project must run green");
    let cost = regions::analyse(&corpus, 4);
    assert_eq!(cost.world_backed, 16);
    assert_eq!(cost.newly_serialized, 0);
    assert_eq!(cost.today.groups, cost.without_forking.groups);
}

/// The measured answer for the corpus ADR 0017 §6 quotes.
#[test]
fn the_examples_suite_loses_nothing_to_the_region_model() {
    let root = repo_root().join("examples");
    let corpus = regions::measure(&root, 8, false).expect("`examples/` must run green");

    assert!(
        !regions::effects_present(&corpus.footprints).contains("cell"),
        "no test in `examples/` carries a `cell` atom; if one now does, the cost is no longer zero"
    );

    let cost = regions::analyse(&corpus, 8);
    assert_eq!(cost.tests, 186);
    assert_eq!(cost.isolated_today, 176);
    assert_eq!(cost.pure, 165);
    assert_eq!(cost.seeded_only, 11);
    assert_eq!(cost.world_backed, 0);
    assert_eq!(cost.newly_serialized, 0);
    assert_eq!(cost.today.groups, cost.without_forking.groups);
    assert_eq!(
        cost.today.critical_path_millis,
        cost.without_forking.critical_path_millis
    );
}

/// The colouring the counterfactual is read off has to be the one the runner uses, on a real corpus
/// and not only on the seven hand-made footprints the unit test carries.
#[test]
fn the_examples_colouring_is_the_runners_own() {
    let root = repo_root().join("examples");
    let corpus = regions::measure(&root, 8, false).expect("`examples/` must run green");
    let scheduled: Vec<(usize, Footprint)> =
        corpus.footprints.iter().cloned().enumerate().collect();
    let projected: Vec<Footprint> = corpus
        .footprints
        .iter()
        .map(regions::region_footprint)
        .collect();
    assert_eq!(
        regions::colour(&scheduled, &projected),
        ply_test::group_by_conflict(&scheduled)
    );
}

/// What the cost would be if the shape existed, so the verdict rests on a measured exposure rather
/// than on the absence of an example.
#[test]
fn the_exposure_is_a_group_per_test_only_at_one_label() {
    let shape = |labels: usize| {
        let corpus = regions::hypothetical(Hypothetical {
            cell_tests: 176,
            labels,
            shared_tests: 10,
            shared_labels: 3,
            pure_tests: 165,
            seed: 1,
        });
        regions::analyse(&corpus, 8)
    };
    assert_eq!(shape(1).without_forking.groups, 176);
    assert!(shape(8).without_forking.groups <= 30);
    assert_eq!(
        shape(176).newly_serialized,
        118,
        "collisions, not the count"
    );
}

/// A group is a barrier, so the wall-clock model must not be `sum / jobs`.
#[test]
fn the_wall_clock_model_charges_the_barrier_it_claims_to() {
    let corpus = Corpus {
        root: "unit".into(),
        keys: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        footprints: vec![
            Footprint::empty(),
            Footprint::empty(),
            Footprint::empty(),
            Footprint::empty(),
        ],
        millis: vec![4.0, 1.0, 1.0, 1.0],
        worker_setup_millis: 0.0,
        measured_suite_millis: None,
        measured_sequential_millis: None,
    };
    let one_group = regions::analyse(&corpus, 8);
    assert_eq!(one_group.today.makespan_millis, 4.0);

    let four_groups: Vec<Vec<usize>> = (0..4).map(|i| vec![i]).collect();
    assert_eq!(regions::makespan(&four_groups, &corpus.millis, 8, 0.0), 7.0);
}
