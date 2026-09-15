use ply_eval::builtins::Builtin;

/// [`Analysis::walk_callback`] reads the callback out of the **last** argument, which is true
/// of all eight and is not a rule the type system enforces.
#[test]
fn the_callback_builtins_are_the_eight_this_module_knows() {
    let mut names: Vec<&str> = Builtin::all()
        .iter()
        .filter(|b| b.higher_order())
        .map(|b| b.name())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "bytes_position",
            "cell_update",
            "filter",
            "fold",
            "iterate",
            "map",
            "map_fold",
            "map_update"
        ],
        "a callback builtin was added or removed; `walk_callback` reads the function out of \
         the last argument and has to be checked against the new one"
    );
    for b in Builtin::all().iter().filter(|b| b.higher_order()) {
        let (min, max) = b.arity();
        assert_eq!(
            min,
            max,
            "`{}` has a variable arity, so its last argument is not always its callback",
            b.name()
        );
    }
}

/// A run must be a function of its definitions and nothing else, so nothing here may reach a
/// hash-ordered collection or a clock.
#[test]
fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
    let body = include_str!("../../../../ply-eval/src/region_kind.rs");
    for banned in [
        "HashMap",
        "HashSet",
        "FxHashMap",
        "FxHashSet",
        "SystemTime",
        "Instant",
        "thread::",
        "rayon",
    ] {
        assert!(
            !body.contains(banned),
            "`{banned}` appears in ply_eval::region_kind; an inferred region kind is part of \
             what a program means and may not depend on a hasher's seed"
        );
    }
}
