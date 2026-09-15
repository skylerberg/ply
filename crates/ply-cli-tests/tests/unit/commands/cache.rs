use ply_cli::cli::{CacheScope, InspectArgs};
use ply_cli::commands::cache::*;
use ply_cli::style::Style;
use ply_cli::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_hash::DefHash;
use ply_store::{CacheStats, Outcome, Store};
use std::path::Path;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn scope(dir: &Path) -> CacheScope {
    CacheScope {
        path: dir.to_path_buf(),
        json: false,
    }
}

/// Populates the front-end cache the only way anything ever should: by running the real front
/// end over a real project.
fn checked(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = project(files);
    let mut store = Store::open(dir.path()).unwrap();
    ply_cli::driver::load_incremental(dir.path(), &mut store).unwrap();
    store.flush().unwrap();
    dir
}

#[test]
fn clearing_empties_a_populated_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.put(DefHash([7u8; 32]), Outcome::Pass);
    store.flush().unwrap();
    assert_eq!(Store::open(dir.path()).unwrap().len(), 1);

    assert_eq!(clear(&scope(dir.path()), Style::plain()), EXIT_OK);
    assert_eq!(Store::open(dir.path()).unwrap().len(), 0);
}

#[test]
fn stats_on_a_directory_with_no_cache_creates_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(stats(&scope(dir.path()), Style::plain()), EXIT_OK);
    assert!(dir.path().join(ply_store::CACHE_DIR_NAME).is_dir());
}

#[test]
fn a_corrupt_cache_degrades_to_empty_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    Store::open(dir.path()).unwrap();
    let path = dir.path().join(ply_store::CACHE_DIR_NAME);
    std::fs::write(path.join("results.json"), "{ not json at all").unwrap();

    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(store.len(), 0);
    let warnings = store.take_warnings();
    assert!(!warnings.is_empty());
    assert!(!warnings_json(&warnings).as_array().unwrap().is_empty());
}

#[test]
fn inspect_finds_a_function_by_simple_name_and_prints_a_resolved_type() {
    let dir = checked(&[(
        "user.ply",
        "effect db {\n  read all[t]() -> List<Int>\n}\n\
         fn active(n: Int) -> List<Int> / {db.read[users]} = db.all[users]()\n",
    )]);
    let store = Store::open(dir.path()).unwrap();
    let found = store.lookup("active");
    assert_eq!(found.len(), 1, "expected one match, got {found:?}");

    let entry = Entry::of(&found[0], &store);
    assert_eq!(entry.title, "user.active");
    assert_eq!(entry.kind, "fn");
    assert_eq!(
        entry.rows(),
        [
            (
                "type",
                "(Int) -> List<Int> / {user.db.read[users]}".to_string()
            ),
            ("footprint", "{user.db.read[users]}".to_string()),
        ],
        "a serialized scheme is exactly what this command exists not to print"
    );
    assert!(entry.location.as_deref().unwrap().contains("user.ply:4:"));
    assert!(entry.result_line().contains("not a test"));
}

#[test]
fn inspect_matches_a_hash_prefix_and_agrees_with_the_name_it_found() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
    let store = Store::open(dir.path()).unwrap();
    let by_name = store.lookup("one");
    assert_eq!(by_name.len(), 1);

    let prefix = &by_name[0].hash().to_hex()[..6];
    let by_hash = store.lookup(prefix);
    assert_eq!(by_hash.len(), 1);
    assert_eq!(by_hash[0].hash(), by_name[0].hash());
    assert_eq!(
        Entry::of(&by_hash[0], &store).title,
        Entry::of(&by_name[0], &store).title
    );
}

/// Three characters is not a hash prefix, and a name that happens to be hex still has to match
/// as a name.
#[test]
fn a_query_shorter_than_four_characters_is_never_read_as_a_hash() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
    let store = Store::open(dir.path()).unwrap();
    let hex = store.lookup("one")[0].hash().to_hex();
    assert!(store.lookup(&hex[..3]).is_empty());
    assert!(!store.lookup(&hex[..4]).is_empty());
}

#[test]
fn inspect_prints_an_effect_declaration_operation_by_operation() {
    let dir = checked(&[(
        "store.ply",
        "nondet effect wall {\n  read now() -> Int\n  write set[c](t: Int) -> Unit\n}\n",
    )]);
    let store = Store::open(dir.path()).unwrap();
    let found = store.lookup("wall");
    assert_eq!(found.len(), 1);

    let entry = Entry::of(&found[0], &store);
    assert_eq!(entry.kind, "effect");
    let Interface::Effect { nondet, operations } = &entry.interface else {
        panic!("expected an effect declaration");
    };
    assert!(nondet);
    assert!(operations.iter().any(|o| o == "read now() -> Int"));
    assert!(
        operations.iter().any(|o| o == "write set[r](Int) -> Unit"),
        "a resource-parameterized operation must say so: {operations:?}"
    );

    // One operation or ten, `operations` is an array either way.
    assert_eq!(
        entry.to_json()["interface"]["operations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(entry.to_json()["interface"]["nondet"], true);
}

#[test]
fn inspect_reports_a_test_with_its_cached_outcome() {
    let dir = checked(&[("m.ply", "test \"one is one\" { assert_eq(1, 1) }\n")]);
    let store = Store::open(dir.path()).unwrap();
    let found = store.lookup("one is one");
    assert_eq!(found.len(), 1, "got {found:?}");

    let entry = Entry::of(&found[0], &store);
    assert_eq!(entry.kind, "test");
    assert!(entry.result_line().contains("not proven"));

    let hash = found[0].hash();
    let mut store = Store::open(dir.path()).unwrap();
    store.put(hash, Outcome::Pass);
    store.flush().unwrap();

    let store = Store::open(dir.path()).unwrap();
    let entry = Entry::of(&store.lookup("one is one")[0], &store);
    assert!(
        entry.result_line().contains("passed"),
        "{}",
        entry.result_line()
    );
}

/// Two modules declaring one simple name is an honest ambiguity — the store holds no namespace
/// that could pick between them — so both are printed, in an order that must not depend on how
/// the store iterates.
#[test]
fn a_simple_name_declared_twice_yields_both_in_a_stable_order() {
    let dir = checked(&[
        ("alpha.ply", "fn shared() -> Int = 1\n"),
        ("beta.ply", "fn shared() -> Int = 2\n"),
    ]);
    let store = Store::open(dir.path()).unwrap();
    let mut found = store.lookup("shared");
    assert_eq!(found.len(), 2);
    found.sort_by_key(order_key);
    let names: Vec<String> = found.iter().map(|f| Entry::of(f, &store).title).collect();
    assert_eq!(names, ["alpha.shared", "beta.shared"]);

    let mut again = store.lookup("shared");
    again.sort_by_key(order_key);
    assert_eq!(
        again.iter().map(order_key).collect::<Vec<_>>(),
        found.iter().map(order_key).collect::<Vec<_>>()
    );
}

#[test]
fn an_edited_file_withholds_a_line_and_column_rather_than_guessing() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\nfn two() -> Int = 2\n")]);
    let store = Store::open(dir.path()).unwrap();
    let found = store.lookup("two");
    assert!(Entry::of(&found[0], &store).location.is_some());

    std::fs::write(dir.path().join("m.ply"), "fn one() -> Int = 1\n").unwrap();
    let entry = Entry::of(&found[0], &store);
    assert!(entry.stale);
    assert_eq!(entry.location, None);
}

/// The case a length check cannot catch.
#[test]
fn an_edit_that_preserves_the_length_is_still_stale() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\nfn two() -> Int = 2\n")]);
    let store = Store::open(dir.path()).unwrap();
    let found = store.lookup("two");
    assert!(Entry::of(&found[0], &store).location.is_some());

    let same_length = "fn ONE() -> Int = 7\nfn two() -> Int = 2\n";
    std::fs::write(dir.path().join("m.ply"), same_length).unwrap();
    let entry = Entry::of(&found[0], &store);
    assert!(
        entry.stale,
        "a same-length edit must not pass for unchanged"
    );
    assert_eq!(entry.location, None);
}

#[test]
fn inspect_of_nothing_is_e0101_and_exits_two() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
    let args = InspectArgs {
        query: "no_such_definition".into(),
        path: dir.path().to_path_buf(),
        json: false,
    };
    assert_eq!(inspect(&args, Style::plain()), EXIT_COMPILE_ERROR);
}

#[test]
fn compact_keeps_what_the_project_still_declares() {
    let dir = checked(&[
        ("keep.ply", "fn kept() -> Int = 1\n"),
        ("gone.ply", "fn dropped() -> Int = 2\n"),
    ]);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.stats().sources, 2);

    std::fs::remove_file(dir.path().join("gone.ply")).unwrap();
    assert_eq!(compact(&scope(dir.path()), Style::plain()), EXIT_OK);

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.stats().sources, 1);
    assert!(!store.lookup("kept").is_empty());
    assert!(store.lookup("dropped").is_empty());
}

#[test]
fn compact_never_drops_a_result() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
    let mut store = Store::open(dir.path()).unwrap();
    store.put(DefHash([3u8; 32]), Outcome::Pass);
    store.flush().unwrap();

    std::fs::remove_file(dir.path().join("m.ply")).unwrap();
    assert_eq!(compact(&scope(dir.path()), Style::plain()), EXIT_OK);

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.len(), 1);
    assert_eq!(store.stats().sources, 0);
}

#[test]
fn compaction_is_reported_even_when_it_reclaims_nothing() {
    let dir = checked(&[("m.ply", "fn one() -> Int = 1\n")]);
    assert_eq!(compact(&scope(dir.path()), Style::plain()), EXIT_OK);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.stats().sources, 1, "a live file must survive");
}

#[test]
fn a_half_wasted_data_file_is_where_compaction_starts_being_suggested() {
    let mut stats = CacheStats {
        data_bytes: 1000,
        garbage_bytes: Some(400),
        ..CacheStats::default()
    };
    assert!(!compact_suggested(&stats));
    stats.garbage_bytes = Some(501);
    assert!(compact_suggested(&stats));

    stats.garbage_bytes = None;
    assert!(
        !compact_suggested(&stats),
        "an unmeasurable ratio is not a reason to suggest anything"
    );
    assert_eq!(garbage_ratio(&stats), None);
}

#[test]
fn sizes_are_readable_rather_than_raw() {
    assert_eq!(bytes(0), "0 B");
    assert_eq!(bytes(999), "999 B");
    assert_eq!(bytes(1024), "1.00 KB");
    assert_eq!(bytes(12_750_000), "12.16 MB");
}
