use ply_span::Symbol;
use ply_ty::*;
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
fn a_mode_atom_covers_its_operations_and_an_operation_atom_only_itself() {
    let write = atom("net", Some("conn"), Mode::Write);
    let read = atom("net", Some("conn"), Mode::Read);
    let conn = || Resource::Named(Symbol::new("conn"));
    let send = EffectAtom::operation("net", conn(), Mode::Write, "send");
    let recv = EffectAtom::operation("net", conn(), Mode::Write, "recv");
    let peek = EffectAtom::operation("net", conn(), Mode::Read, "peek");
    assert!(write.covers(&send));
    assert!(write.covers(&write));
    assert!(send.covers(&send));
    assert!(!send.covers(&write));
    assert!(!send.covers(&recv));
    assert!(!read.covers(&send));
    assert!(read.covers(&peek));
    assert!(!atom("net", Some("other"), Mode::Write).covers(&send));
    assert_eq!(send.mode_atom(), write);
    let declared = Footprint::from_atoms([write.clone()]);
    assert!(declared.covers(&send) && declared.covers(&recv) && !declared.covers(&peek));
    let named = Footprint::from_atoms([send.clone()]);
    assert!(named.covers(&send) && !named.covers(&recv) && !named.covers(&write));
}

#[test]
fn an_operation_atom_takes_its_mode_from_the_declaration() {
    let peek = EffectAtom::operation("net", Resource::Singleton, Mode::Write, "peek");
    let declared = |effect: &Symbol, op: &Symbol| {
        (effect.as_str() == "net" && op.as_str() == "peek").then_some(Mode::Read)
    };
    assert_eq!(peek.clone().with_declared_mode(&declared).mode, Mode::Read);
    let mut footprint = Footprint::from_atoms([peek.clone(), atom("net", None, Mode::Write)]);
    footprint.resolve_modes(&declared);
    assert_eq!(footprint.to_string(), "{net.peek, net.write}");
    let mut row = Row::closed([peek]);
    row.resolve_modes(&declared);
    assert!(row.atoms.iter().all(|a| a.mode == Mode::Read));
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
