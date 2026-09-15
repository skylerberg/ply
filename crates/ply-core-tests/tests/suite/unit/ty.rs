use ply_core::ty::*;
use ply_span::Symbol;
use ply_syntax::ast::Mode;
use std::collections::BTreeSet;

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
fn reads_of_the_same_resource_do_not_conflict() {
    let a = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
    let b = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
    assert!(!a.conflicts_with(&b));
}

#[test]
fn a_write_conflicts_with_a_read_of_the_same_resource() {
    let r = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
    let w = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
    assert!(r.conflicts_with(&w));
    assert!(w.conflicts_with(&r));
}

#[test]
fn writes_to_distinct_resources_do_not_conflict() {
    let a = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
    let b = Footprint::from_atoms([atom("db", Some("orders"), Mode::Write)]);
    assert!(!a.conflicts_with(&b));
}

#[test]
fn same_resource_name_under_different_effects_does_not_conflict() {
    let a = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
    let b = Footprint::from_atoms([atom("cache", Some("users"), Mode::Write)]);
    assert!(!a.conflicts_with(&b));
}

#[test]
fn singleton_resources_conflict_only_within_their_effect() {
    let a = Footprint::from_atoms([atom("clock", None, Mode::Write)]);
    let b = Footprint::from_atoms([atom("clock", None, Mode::Read)]);
    let c = Footprint::from_atoms([atom("random", None, Mode::Write)]);
    assert!(a.conflicts_with(&b));
    assert!(!a.conflicts_with(&c));
}

#[test]
fn the_empty_footprint_conflicts_with_nothing() {
    let e = Footprint::empty();
    let w = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
    assert!(!e.conflicts_with(&w));
    assert!(!w.conflicts_with(&e));
    assert!(!e.conflicts_with(&e));
}

#[test]
fn row_display_round_trips_the_surface_syntax() {
    let r = Row::closed([atom("db", Some("users"), Mode::Read)]);
    assert_eq!(r.to_string(), "{db.read[users]}");
    let open = Row {
        atoms: r.atoms.clone(),
        tail: Some(RowVar(3)),
    };
    assert_eq!(open.to_string(), "{db.read[users] | e3}");
    assert_eq!(Row::empty().to_string(), "{}");
}

#[test]
fn without_removes_handled_atoms_and_keeps_the_tail() {
    let read = atom("db", Some("users"), Mode::Read);
    let write = atom("db", Some("users"), Mode::Write);
    let row = Row {
        atoms: [read.clone(), write.clone()].into(),
        tail: Some(RowVar(1)),
    };
    let handled: BTreeSet<_> = [read].into();
    let out = row.without(&handled);
    assert_eq!(out.atoms, [write].into());
    assert_eq!(out.tail, Some(RowVar(1)));
}
