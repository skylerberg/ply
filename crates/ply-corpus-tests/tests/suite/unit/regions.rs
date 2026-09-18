use ply_corpus::regions::{
    Corpus, Hypothetical, analyse, colour, hypothetical, makespan, region_footprint,
};
use ply_span::Symbol;
use ply_syntax::ast::Mode;
use ply_ty::{EffectAtom, Footprint, Resource};

fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, Resource::Named(Symbol::new(resource)), mode)
}

fn fp(atoms: impl IntoIterator<Item = EffectAtom>) -> Footprint {
    Footprint::from_atoms(atoms)
}

#[test]
fn colouring_reproduces_the_runners_own_grouping() {
    let tests: Vec<(usize, Footprint)> = vec![
        (0, fp([atom("db", "a", Mode::Write)])),
        (1, fp([atom("db", "a", Mode::Read)])),
        (2, fp([atom("db", "b", Mode::Write)])),
        (3, fp([atom("cell", "s", Mode::Write)])),
        (4, Footprint::empty()),
        (
            5,
            fp([atom("db", "a", Mode::Write), atom("db", "b", Mode::Write)]),
        ),
        (6, fp([atom("sim", "x", Mode::Read)])),
    ];
    let projected: Vec<Footprint> = tests.iter().map(|(_, f)| region_footprint(f)).collect();
    assert_eq!(
        colour(&tests, &projected),
        ply_test::group_by_conflict(&tests)
    );
}

#[test]
fn two_tests_sharing_one_cell_label_split_without_forking() {
    let corpus = Corpus {
        root: "unit".into(),
        keys: vec!["m.a".into(), "m.b".into()],
        footprints: vec![
            fp([atom("cell", "users", Mode::Write)]),
            fp([atom("cell", "users", Mode::Write)]),
        ],
        millis: vec![1.0, 1.0],
        worker_setup_millis: 0.0,
        measured_suite_millis: None,
        measured_sequential_millis: None,
    };
    let cost = analyse(&corpus, 8);
    assert_eq!(cost.isolated_today, 2);
    assert_eq!(cost.today.groups, 1);
    assert_eq!(cost.without_forking.groups, 2);
    assert_eq!(cost.newly_serialized, 2);
    assert_eq!(cost.wall_clock_ratio(), 2.0);
}

#[test]
fn pure_tests_cost_nothing_under_either_model() {
    let mut footprints = vec![fp([atom("db", "a", Mode::Write)])];
    footprints.extend((0..100).map(|_| Footprint::empty()));
    let corpus = Corpus {
        root: "unit".into(),
        keys: (0..footprints.len()).map(|i| format!("m.t{i}")).collect(),
        footprints,
        millis: vec![1.0; 101],
        worker_setup_millis: 0.0,
        measured_suite_millis: None,
        measured_sequential_millis: None,
    };
    let cost = analyse(&corpus, 8);
    assert_eq!(cost.newly_serialized, 0);
    assert_eq!(cost.today.groups, cost.without_forking.groups);
    assert_eq!(cost.pure, 100);
}

#[test]
fn a_seeded_test_is_untouched_by_the_change() {
    let corpus = Corpus {
        root: "unit".into(),
        keys: vec!["m.a".into(), "m.b".into()],
        footprints: vec![
            fp([atom("sim", "x", Mode::Read)]),
            fp([atom("sim", "x", Mode::Read)]),
        ],
        millis: vec![1.0, 1.0],
        worker_setup_millis: 0.0,
        measured_suite_millis: None,
        measured_sequential_millis: None,
    };
    let cost = analyse(&corpus, 8);
    assert_eq!(cost.newly_serialized, 0);
    assert_eq!(cost.seeded_only, 2);
    assert_eq!(cost.without_forking.groups, 1);
}

#[test]
fn a_lone_cell_label_does_not_serialize() {
    let corpus = Corpus {
        root: "unit".into(),
        keys: vec!["m.a".into(), "m.b".into()],
        footprints: vec![
            fp([atom("cell", "users", Mode::Write)]),
            fp([atom("cell", "orders", Mode::Write)]),
        ],
        millis: vec![1.0, 1.0],
        worker_setup_millis: 0.0,
        measured_suite_millis: None,
        measured_sequential_millis: None,
    };
    let cost = analyse(&corpus, 8);
    assert_eq!(cost.world_backed, 2);
    assert_eq!(cost.newly_serialized, 0);
    assert_eq!(cost.without_forking.groups, 1);
}

#[test]
fn makespan_charges_a_barrier_between_groups() {
    let groups = vec![vec![0, 1, 2, 3], vec![4]];
    let millis = vec![1.0, 1.0, 1.0, 1.0, 10.0];
    assert_eq!(makespan(&groups, &millis, 2, 0.0), 2.0 + 10.0);
    assert_eq!(makespan(&groups, &millis, 0, 0.0), 1.0 + 10.0);
    assert_eq!(makespan(&groups, &millis, 1, 0.0), 4.0 + 10.0);
}

#[test]
fn a_hypothetical_corpus_is_a_pure_function_of_its_shape() {
    let shape = Hypothetical {
        cell_tests: 40,
        labels: 4,
        shared_tests: 10,
        shared_labels: 3,
        pure_tests: 100,
        seed: 7,
    };
    let a = hypothetical(shape);
    let b = hypothetical(shape);
    assert_eq!(a.footprints, b.footprints);
    assert_eq!(a.footprints.len(), 150);
}
