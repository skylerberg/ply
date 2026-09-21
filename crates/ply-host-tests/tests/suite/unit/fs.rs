use ply_host::fs::*;
use ply_span::{Span, Symbol, codes};
use ply_ty::Resource;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn root() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

fn span() -> Span {
    Span::DUMMY
}

#[test]
fn a_path_under_the_root_resolves() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    std::fs::write(real.join("a.ply"), b"x").unwrap();
    assert_eq!(confine(&real, "a.ply", span()).unwrap(), real.join("a.ply"));
}

#[test]
fn an_absolute_path_and_a_parent_component_are_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    for path in ["/etc/passwd", "../secrets", "src/../../secrets"] {
        let refusal = confine(&real, path, span()).expect_err("it should be refused");
        assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT, "for `{path}`");
    }
}

/// The half a lexical check cannot do.
#[test]
fn a_symlink_out_of_the_root_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let outside_real = outside.path().canonicalize().unwrap();
    std::fs::write(outside_real.join("secrets"), b"s").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_real, real.join("link")).unwrap();
    #[cfg(not(unix))]
    return;

    let refusal = confine(&real, "link/secrets", span()).expect_err("it should be refused");
    assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
}

/// A not-yet-existing target still traverses its parent's link, so the nearest ancestor is checked.
#[test]
fn a_write_through_a_symlinked_directory_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let outside_real = outside.path().canonicalize().unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_real, real.join("out")).unwrap();
    #[cfg(not(unix))]
    return;

    let refusal = confine(&real, "out/artifact.plyx", span()).expect_err("it should be refused");
    assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
}

#[test]
fn a_root_that_is_not_a_directory_does_not_bind() {
    let dir = root();
    let file = dir.path().join("a.ply");
    std::fs::write(&file, b"x").unwrap();
    let mut roots = Roots::new();
    let refused = roots
        .bind("src", &file, span())
        .expect_err("a file is not a root");
    assert_eq!(refused.code, codes::FS_ROOT_INVALID);
    assert!(roots.is_empty());
}

#[test]
fn an_unbound_label_names_the_flag_that_would_bind_it() {
    let d = unbound(Op::ReadFile, &Resource::Named(Symbol::new("src")), span());
    assert_eq!(d.code, codes::FS_ROOT_UNBOUND);
    assert!(
        d.notes.iter().any(|n| n.contains("--fs src=")),
        "the diagnostic should name the flag: {:?}",
        d.notes
    );
}

/// What `disk.rs` serialises a read-merge-write with, as an operation a Ply store can perform.
#[test]
fn a_lock_is_taken_once_and_released_only_by_its_holder() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let lock = real.join("lock");
    let held = Mutex::new(BTreeSet::new());

    assert!(take_lock(&lock, &held));
    assert!(lock.exists(), "the lock is the file's existence");
    // A second taker waits out the holder and answers `false` rather than raising. Waiting the
    // two seconds `fs.lock` waits proves nothing this does not, so the wait is the caller's here.
    let brief = Duration::from_millis(20);
    let waited = Instant::now();
    assert!(!take_lock_within(&lock, &held, brief));
    assert!(waited.elapsed() >= brief, "it gave up before waiting");
    assert!(
        waited.elapsed() < LOCK_WAIT,
        "it waited past the bound it was given"
    );

    assert!(drop_lock(&lock, &held));
    assert!(!lock.exists());
    // Releasing one nothing holds changes nothing, and the next taker gets it at once.
    assert!(!drop_lock(&lock, &held));
    assert!(take_lock(&lock, &held));
    assert!(drop_lock(&lock, &held));
}

/// A lock file another run left behind is one this run cannot remove; only age breaks it.
#[test]
fn a_lock_this_run_did_not_take_is_not_its_to_release() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let lock = real.join("lock");
    std::fs::write(&lock, b"").unwrap();
    let held = Mutex::new(BTreeSet::new());

    assert!(!drop_lock(&lock, &held));
    assert!(lock.exists(), "another holder's lock was removed");
}

/// The holder died without releasing, which a `--jobs` run has to be able to recover from.
#[test]
fn a_lock_older_than_the_stale_age_is_broken_and_taken() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let lock = real.join("lock");
    std::fs::write(&lock, b"").unwrap();
    let old = std::time::SystemTime::now() - LOCK_STALE_AGE - Duration::from_secs(60);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(old)
        .unwrap();

    let held = Mutex::new(BTreeSet::new());
    assert!(take_lock(&lock, &held));
    assert!(drop_lock(&lock, &held));
}

#[test]
fn a_lock_with_no_directory_to_sit_in_is_refused_without_waiting() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let held = Mutex::new(BTreeSet::new());
    let waited = Instant::now();
    assert!(!take_lock(&real.join("absent/lock"), &held));
    assert!(
        waited.elapsed() < LOCK_WAIT,
        "there was nothing to wait for"
    );
}

#[test]
fn the_lock_operations_are_declared_and_confined_like_every_other() {
    assert!(Op::ALL.contains(&Op::Lock));
    assert!(Op::ALL.contains(&Op::Unlock));
    assert_eq!(Op::Lock.name(), "lock");
    assert_eq!(Op::Unlock.name(), "unlock");
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let refusal = confine(&real, "../lock", span()).expect_err("it should be refused");
    assert_eq!(refusal.code, codes::FS_PATH_ESCAPES_ROOT);
    let no_root = unbound(Op::Lock, &Resource::Named(Symbol::new("cache")), span());
    assert_eq!(no_root.code, codes::FS_ROOT_UNBOUND);
    assert!(no_root.message.contains("`fs.lock`"), "{}", no_root.message);
}
