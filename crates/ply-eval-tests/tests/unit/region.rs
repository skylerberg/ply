/// The same rule `sim`, `sched` and `explore` are held to, and for the same reason: this module
/// holds a live region's state, so a hash map named here would put the host's memory layout
/// into a seeded run's answer just as surely as one named in the scheduler.
#[test]
fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
    let body = include_str!("../../../ply-eval/src/region.rs");
    for banned in [
        "HashMap",
        "HashSet",
        "FxHashMap",
        "FxHashSet",
        "SystemTime",
        "Instant",
        "thread::",
        "rayon",
        "as_ptr",
        "strong_count",
    ] {
        assert!(
            !body.contains(banned),
            "`{banned}` appears in ply_eval::region; a seeded run must be a \
             function of its definitions and its seed and nothing else"
        );
    }
}
