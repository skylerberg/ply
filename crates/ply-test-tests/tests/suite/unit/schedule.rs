use ply_span::Symbol;
use ply_test::schedule::{
    Isolation, SIM_EFFECT, contends_only_over_regions, group_by_conflict, is_seeded, parallelism,
    region_isolated, shared_footprint,
};
use ply_ty::Mode;
use ply_ty::{EffectAtom, Footprint, Resource};

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
    EffectAtom::new(
        effect,
        resource
            .map(|r| Resource::Named(Symbol::new(r)))
            .unwrap_or(Resource::Singleton),
        mode,
    )
}

#[test]
fn a_seed_read_does_not_make_a_test_shared() {
    let f = Footprint::from_atoms([atom(SIM_EFFECT, None, Mode::Read)]);
    assert!(is_seeded(&f));
    assert!(region_isolated(&f));
    assert_eq!(Isolation::of(&f), Isolation::Region);
    assert!(shared_footprint(&f).is_empty());
}

#[test]
fn a_region_label_contends_and_the_seed_beside_it_still_does_not() {
    let f = Footprint::from_atoms([
        atom(SIM_EFFECT, None, Mode::Read),
        atom("cell", Some("users"), Mode::Write),
    ]);
    assert!(is_seeded(&f));
    assert!(!region_isolated(&f));
    assert_eq!(Isolation::of(&f), Isolation::Shared);
    assert!(contends_only_over_regions(&f));
    assert_eq!(
        shared_footprint(&f).to_string(),
        "{cell.write[users]}",
        "the seed must not appear among the atoms that made it shared"
    );
}

#[test]
fn a_simulated_test_still_contends_over_what_its_tasks_touch() {
    let writer = Footprint::from_atoms([
        atom(SIM_EFFECT, None, Mode::Read),
        atom("db", Some("orders"), Mode::Write),
    ]);
    let reader = Footprint::from_atoms([atom("db", Some("orders"), Mode::Read)]);
    assert!(!region_isolated(&writer));
    assert!(!contends_only_over_regions(&writer));
    assert!(shared_footprint(&writer).conflicts_with(&shared_footprint(&reader)));
}

#[test]
fn a_test_that_never_simulated_is_not_seeded() {
    let f = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
    assert!(!is_seeded(&f));
    assert!(!is_seeded(&Footprint::empty()));
}

#[test]
fn adding_isolated_simulated_tests_changes_no_group_count() {
    let shared: Vec<(usize, Footprint)> = vec![
        (
            0,
            Footprint::from_atoms([atom("db", Some("a"), Mode::Write)]),
        ),
        (
            1,
            Footprint::from_atoms([atom("db", Some("a"), Mode::Write)]),
        ),
    ];
    let before = group_by_conflict(&shared).len();
    let mut wider = shared.clone();
    for i in 0..100 {
        wider.push((
            2 + i,
            Footprint::from_atoms([atom(SIM_EFFECT, None, Mode::Read)]),
        ));
    }
    assert_eq!(group_by_conflict(&wider).len(), before);
}

#[test]
fn one_label_separates_its_writers_and_two_labels_do_not() {
    let same: Vec<(usize, Footprint)> = (0..4)
        .map(|i| {
            (
                i,
                Footprint::from_atoms([atom("cell", Some("users"), Mode::Write)]),
            )
        })
        .collect();
    assert_eq!(group_by_conflict(&same).len(), 4);

    let distinct: Vec<(usize, Footprint)> = (0..4)
        .map(|i| {
            (
                i,
                Footprint::from_atoms([atom("cell", Some(&format!("r{i}")), Mode::Write)]),
            )
        })
        .collect();
    assert_eq!(group_by_conflict(&distinct).len(), 1);
}

#[test]
fn parallelism_counts_the_region_contended_apart_from_the_rest() {
    let footprints = [
        Footprint::empty(),
        Footprint::from_atoms([atom("cell", Some("users"), Mode::Write)]),
        Footprint::from_atoms([atom("cell", Some("users"), Mode::Write)]),
        Footprint::from_atoms([atom("db", Some("orders"), Mode::Write)]),
        Footprint::from_atoms([
            atom("cell", Some("users"), Mode::Write),
            atom("db", Some("orders"), Mode::Write),
        ]),
    ];
    let scheduled: Vec<(usize, Footprint)> = footprints.iter().cloned().enumerate().collect();
    let groups = group_by_conflict(&scheduled);
    let p = parallelism(footprints.iter(), &scheduled, &groups);

    assert_eq!(p.total, 5);
    assert_eq!(p.isolated, 1);
    assert_eq!(p.shared, 4);
    assert_eq!(
        p.region_contended, 2,
        "the mixed footprint contends over a real resource too and is not this number"
    );
    assert!(p.holds(), "{p:?}");
}
