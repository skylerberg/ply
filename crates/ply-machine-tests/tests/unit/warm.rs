use ply_eval::DefHash;
use ply_machine::load::{Found, Loaded, stamp_of};
use ply_machine::warm::*;
use ply_store::ContentHash;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    dir
}

/// A `Loaded` is expensive here, so the state is exercised through the questions the watch loop asks.
fn held(root: &std::path::Path, files: &[&str]) -> Warm {
    let mut warm = Warm::default();
    warm.keep(fake_loaded(root, files));
    warm
}

/// A walk that read nothing must not answer like a walk that found a tree, or the watcher polling
/// it compares two such answers, finds them equal and stops noticing saves.
#[test]
fn a_tree_that_cannot_be_walked_is_not_a_tree_that_has_not_moved() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let walked = tree_stamps(dir.path()).expect("a readable tree stamps");
    assert_eq!(walked.len(), 1);
    assert_eq!(tree_stamps(&dir.path().join("gone")), None);
}

#[test]
fn an_unmoved_tree_has_not_moved() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let warm = held(dir.path(), &["m.ply"]);
    assert!(!warm.tree_moved(dir.path()));
    // And with nothing held there is nothing to reuse, whatever the stamps say.
    let bare = Warm {
        stamps: warm.stamps.clone(),
        ..Warm::default()
    };
    assert!(bare.tree_moved(dir.path()));
}

#[test]
fn a_rewritten_file_moves_the_tree() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let warm = held(dir.path(), &["m.ply"]);
    std::fs::write(dir.path().join("m.ply"), "fn a() -> Int = 2222222\n").unwrap();
    assert!(
        warm.tree_moved(dir.path()),
        "a file whose length changed did not move the tree"
    );
}

/// A new file changes the program without touching any file the held state knows, so `tree_moved` walks the tree too.
#[test]
fn a_new_file_moves_the_tree() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let warm = held(dir.path(), &["m.ply"]);
    assert!(!warm.tree_moved(dir.path()));
    std::fs::write(dir.path().join("n.ply"), "fn b() -> Int = 2\n").unwrap();
    assert!(
        warm.tree_moved(dir.path()),
        "a file that appeared did not move the tree"
    );
}

#[test]
fn a_deleted_file_moves_the_tree() {
    let dir = project(&[
        ("m.ply", "fn a() -> Int = 1\n"),
        ("n.ply", "fn b() -> Int = 2\n"),
    ]);
    let warm = held(dir.path(), &["m.ply", "n.ply"]);
    std::fs::remove_file(dir.path().join("n.ply")).unwrap();
    assert!(warm.tree_moved(dir.path()));
}

/// Every run writes the cache directory, so a walk that counted it would never settle.
#[test]
fn the_cache_directory_is_not_the_program() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    std::fs::create_dir(dir.path().join(".ply-cache")).unwrap();
    std::fs::write(dir.path().join(".ply-cache").join("frontend.ply"), "x").unwrap();
    let warm = held(dir.path(), &["m.ply"]);
    assert!(!warm.tree_moved(dir.path()));
}

/// An iteration that fails part way must not leave behind a state no run finished with.
#[test]
fn taking_leaves_nothing_behind() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let mut warm = held(dir.path(), &["m.ply"]);
    let (taken, reuse) = warm.take(dir.path());
    assert!(taken.is_some());
    assert_eq!(reuse, Reuse::Whole);
    assert!(warm.held.is_none());
    assert!(warm.take(dir.path()).0.is_none());
}

#[test]
fn a_moved_tree_is_not_reused() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let mut warm = held(dir.path(), &["m.ply"]);
    std::fs::write(dir.path().join("m.ply"), "fn a() -> Int = 999999\n").unwrap();
    let (taken, reuse) = warm.take(dir.path());
    assert!(taken.is_none());
    assert!(matches!(reuse, Reuse::Reloaded { .. }));
}

/// Most saves rewrite the same bytes, which a stamp alone cannot tell from an edit.
#[test]
fn a_file_written_with_the_same_bytes_is_reused_whole() {
    let text = "fn a() -> Int = 1\n";
    let dir = project(&[("m.ply", text)]);
    let mut warm = held(dir.path(), &["m.ply"]);

    // Stamp set rather than waited for: a rewrite inside one timestamp tick would leave it equal and test nothing.
    std::fs::write(dir.path().join("m.ply"), text).unwrap();
    warm.stamps
        .insert(dir.path().join("m.ply"), (None, u64::MAX));
    assert!(
        warm.tree_moved(dir.path()),
        "the stamp did not move, so this test is not exercising the path it is about"
    );

    let (taken, reuse) = warm.take(dir.path());
    assert_eq!(reuse, Reuse::Whole, "a save that changed nothing reloaded");
    assert!(taken.is_some());
}

/// An iteration reports long after it read the tree, and a save can land in between. The baseline
/// is the bytes the load read, so that save is a change the next iteration sees; a baseline taken
/// from the disk at the end of the iteration would swallow it and serve a stale front end.
#[test]
fn a_save_made_after_the_load_is_a_change_and_not_the_baseline() {
    let dir = project(&[("m.ply", "fn a() -> Int = 1\n")]);
    let mut warm = Warm::default();
    let loaded = fake_loaded(dir.path(), &["m.ply"]);

    std::fs::write(dir.path().join("m.ply"), "fn a() -> Int = 999999\n").unwrap();
    warm.keep(loaded);

    assert!(
        warm.tree_moved(dir.path()),
        "the save landed inside the iteration and the watcher stopped seeing it"
    );
    let (taken, reuse) = warm.take(dir.path());
    assert!(taken.is_none(), "the edit was folded into the baseline");
    assert!(matches!(reuse, Reuse::Reloaded { changed: 1 }), "{reuse:?}");
}

/// Keyed on `defs` alone, an edit to a test would reuse a unit holding the old test.
#[test]
fn a_held_unit_is_reused_only_for_the_program_it_was_compiled_from() {
    struct Nothing;
    impl ply_eval::Provider for Nothing {
        fn attach(&'static self) -> std::rc::Rc<dyn ply_eval::Compiled> {
            unreachable!("this provider is never attached")
        }
        fn name(&self) -> &'static str {
            "nothing"
        }
        fn len(&self) -> usize {
            0
        }
        fn offers(&self) -> ply_eval::backend::Offers {
            ply_eval::backend::Offers::default()
        }
        fn relocate(&self, _: &ply_eval::Front, _: &ply_eval::SourceMap) -> bool {
            true
        }
    }
    let provider: &'static dyn ply_eval::Provider = Box::leak(Box::new(Nothing));
    let mut warm = Warm::default();
    // The digest is the compiler's now: a program that moved is a different digest, so the unit
    // is kept only under the identity it was compiled from.
    let unit_for = |warm: &Warm, digest: DefHash| {
        let front = ply_eval::Front {
            hashes_digest: digest,
            ..Default::default()
        };
        warm.unit_for(&front, &ply_eval::SourceMap::new())
    };

    let compiled = DefHash([9; 32]);
    warm.keep_unit(compiled, provider);
    assert!(
        unit_for(&warm, compiled).is_some(),
        "the same program keeps its unit"
    );
    assert!(
        unit_for(&warm, DefHash([8; 32])).is_none(),
        "another program does not reuse it"
    );
}

/// As a load leaves it: each file stamped before its bytes were read.
fn fake_loaded(root: &std::path::Path, files: &[&str]) -> Loaded {
    Loaded {
        root: root.to_path_buf(),
        files: files
            .iter()
            .map(|f| {
                let path = root.join(f);
                let stamp = stamp_of(&path);
                let content = ContentHash::of(&std::fs::read(&path).unwrap_or_default());
                Found {
                    path,
                    stamp,
                    content,
                }
            })
            .collect(),
        sources: ply_eval::SourceMap::new(),
        front: Default::default(),
        check: Default::default(),
        hashes: Default::default(),
        frontend: Default::default(),
        promised: false,
    }
}
