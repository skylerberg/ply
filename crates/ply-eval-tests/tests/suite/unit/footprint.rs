use ply_eval::{EffectAtom, Footprint, Mode, Resource, Symbol, atom_texts, label_var_name};

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
    let mut footprint = Footprint::from_atoms([peek, atom("net", None, Mode::Write)]);
    footprint.resolve_modes(&declared);
    assert_eq!(footprint.to_string(), "{net.peek, net.write}");
    assert!(
        footprint
            .atoms()
            .any(|a| a.op.is_some() && a.mode == Mode::Read)
    );
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
fn an_operation_atom_prints_its_operation_in_place_of_the_mode() {
    let conn = || Resource::Named(Symbol::new("conn"));
    let footprint = Footprint::from_atoms([
        EffectAtom::operation("net", conn(), Mode::Write, "send"),
        EffectAtom::new("net", conn(), Mode::Write),
    ]);
    assert_eq!(footprint.to_string(), "{net.write[conn], net.send[conn]}");
}

/// A footprint names its labels in the order its atoms name them.
#[test]
fn a_footprints_labels_are_named_in_the_order_its_atoms_name_them() {
    let bound = Footprint::from_atoms([
        EffectAtom::operation("net", Resource::Var(0), Mode::Write, "recv"),
        EffectAtom::operation("net", Resource::Var(1), Mode::Write, "send"),
        EffectAtom::new("net", Resource::Named(Symbol::new("conn")), Mode::Write),
    ]);
    assert_eq!(
        atom_texts(&bound.0),
        ["net.write[conn]", "net.recv[l]", "net.send[m]"]
    );
    assert!(atom_texts(&Footprint::empty().0).is_empty());
}

/// A label variable may not take the name of a resource in the same text, or the text would say
/// the two are one; it steps to the next letter, and past the last to the round.
#[test]
fn a_label_variable_steps_past_a_resource_of_its_name() {
    let op =
        |resource: Resource, name: &str| EffectAtom::operation("net", resource, Mode::Write, name);
    let named = |name: &str| Resource::Named(Symbol::new(name));
    let send = op(named("l"), "send");
    let recv = op(Resource::Var(0), "recv");
    let texts = |atoms: Vec<EffectAtom>| atom_texts(&Footprint::from_atoms(atoms).0);
    assert_eq!(
        texts(vec![send.clone(), recv.clone()]),
        ["net.send[l]", "net.recv[m]"]
    );
    // One operation under a resource and under the variable: two atoms, and two names.
    assert_eq!(
        texts(vec![send.clone(), op(Resource::Var(0), "send")]),
        ["net.send[l]", "net.send[m]"]
    );
    let close = op(named("m"), "close");
    assert_eq!(
        texts(vec![send.clone(), close.clone(), recv.clone()]),
        ["net.send[l]", "net.close[m]", "net.recv[n]"]
    );
    let connect = op(named("n"), "connect");
    assert_eq!(
        texts(vec![send, close, connect, recv]),
        [
            "net.send[l]",
            "net.close[m]",
            "net.connect[n]",
            "net.recv[l1]"
        ]
    );
    // On its own a label has no resource to step past.
    assert_eq!(label_var_name(0), "l");
    assert_eq!(label_var_name(3), "l1");
}
