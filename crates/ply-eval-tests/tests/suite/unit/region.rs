/// A hash map here would put the host's memory layout into a seeded run's answer.
#[test]
fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
    let body = include_str!("../../../../ply-eval/src/region.rs");
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
