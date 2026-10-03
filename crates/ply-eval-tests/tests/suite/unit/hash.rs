use ply_eval::DefHash;

#[test]
fn hex_round_trips_and_short_is_a_prefix() {
    let h = DefHash::of(b"fn f() = 1");
    assert_eq!(h.to_hex().len(), 64);
    assert_eq!(h.short().len(), 12);
    assert!(h.to_hex().starts_with(&h.short()));
    assert_eq!(DefHash::from_hex(&h.to_hex()), Some(h));
    assert_eq!(h.to_string(), h.short());
    assert_eq!(DefHash::from_hex("nonsense"), None);
    assert_eq!(DefHash::from_hex(&"z".repeat(64)), None);
}

#[test]
fn a_hash_serializes_as_a_hex_string() {
    let h = DefHash::of(b"fn f() = 1");
    let json = serde_json::to_string(&h).unwrap();
    assert_eq!(json, format!("\"{}\"", h.to_hex()));
    assert_eq!(serde_json::from_str::<DefHash>(&json).unwrap(), h);
    assert!(serde_json::from_str::<DefHash>("\"short\"").is_err());
}

/// `main` calls `f`; `f` and `g` call each other; `g` calls `leaf`; `stray` reaches `leaf` too.
fn graph() -> ply_eval::HashOutput {
    let mut out = ply_eval::HashOutput::default();
    for (name, deps) in [
        ("main", vec!["f"]),
        ("f", vec!["g"]),
        ("g", vec!["f", "leaf"]),
        ("leaf", vec![]),
        ("stray", vec!["leaf"]),
    ] {
        out.deps.insert(
            ply_eval::Symbol::new(name),
            deps.into_iter().map(ply_eval::Symbol::new).collect(),
        );
    }
    out
}

fn names(xs: &[&str]) -> std::collections::BTreeSet<ply_eval::Symbol> {
    xs.iter().map(ply_eval::Symbol::new).collect()
}

#[test]
fn reach_follows_references_through_a_cycle_and_keeps_its_roots() {
    let h = graph();
    assert_eq!(
        h.reach([&ply_eval::Symbol::new("main")]),
        names(&["f", "g", "leaf", "main"])
    );
    assert_eq!(h.reach([&ply_eval::Symbol::new("leaf")]), names(&["leaf"]));
    // A name with no row still reaches itself, as a closure always held its definition.
    assert_eq!(h.reach([&ply_eval::Symbol::new("gone")]), names(&["gone"]));
}

#[test]
fn reaches_among_answers_each_name_what_a_walk_from_it_reaches_within_the_set() {
    let h = graph();
    let among = names(&["f", "g", "leaf", "main"]);
    let reached = h.reaches_among(&among);
    for name in &among {
        let walked: std::collections::BTreeSet<_> =
            h.reach([name]).intersection(&among).cloned().collect();
        assert_eq!(reached[name], walked, "{name:?}");
    }
    // `stray` is outside the set, so nothing reaches through it and it reaches nothing here.
    assert!(!reached.contains_key(&ply_eval::Symbol::new("stray")));
    assert_eq!(
        reached[&ply_eval::Symbol::new("f")],
        names(&["f", "g", "leaf"])
    );
}
