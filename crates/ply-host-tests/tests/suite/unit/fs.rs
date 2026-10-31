use ply_core::crypto::Framing;
use ply_core::{Diagnostic, Span, codes};
use ply_host::fs::*;
use ply_host::jobs::Failure;
use ply_host::native::{found_read, int_of, parts_of};
use ply_host::pool::{Bell, Run};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
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
    assert_eq!(
        confine(&real, "a.ply", span()).unwrap(),
        Some(real.join("a.ply"))
    );
}

#[test]
fn an_absolute_path_and_a_parent_component_are_rejected() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    for path in ["/etc/passwd", "../secrets", "src/../../secrets"] {
        let rejection = confine(&real, path, span()).expect_err("it should be rejected");
        assert_eq!(rejection.code, codes::FS_PATH_ESCAPES_ROOT, "for `{path}`");
    }
}

/// The half a lexical check cannot do.
#[test]
fn a_symlink_out_of_the_root_is_rejected() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let outside_real = outside.path().canonicalize().unwrap();
    std::fs::write(outside_real.join("secrets"), b"s").unwrap();
    std::os::unix::fs::symlink(&outside_real, real.join("link")).unwrap();

    let rejection = confine(&real, "link/secrets", span()).expect_err("it should be rejected");
    assert_eq!(rejection.code, codes::FS_PATH_ESCAPES_ROOT);
}

/// A not-yet-existing target still traverses its parent's link, so the nearest ancestor is checked.
#[test]
fn a_write_through_a_symlinked_directory_is_rejected() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let outside_real = outside.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(&outside_real, real.join("out")).unwrap();

    let rejection = confine(&real, "out/artifact.plyx", span()).expect_err("it should be rejected");
    assert_eq!(rejection.code, codes::FS_PATH_ESCAPES_ROOT);
}

/// A link to where nothing is resolves to nothing, so it is held by where it leads.
#[test]
fn a_write_through_a_link_to_a_missing_path_outside_the_root_is_rejected() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let made = outside.path().canonicalize().unwrap().join("made");
    std::os::unix::fs::symlink(&made, real.join("link")).unwrap();
    let fs = rooted(&real);

    let rejected = perform(&fs, Op::WriteFile, "link", body(b"x"));
    assert!(!made.exists(), "the write landed outside the root");
    assert_eq!(
        rejected.expect_err("the link leads out").code,
        codes::FS_PATH_ESCAPES_ROOT
    );
}

#[test]
fn a_root_that_is_not_a_directory_does_not_bind() {
    let dir = root();
    let file = dir.path().join("a.ply");
    std::fs::write(&file, b"x").unwrap();
    let mut roots = Roots::new();
    let rejected = roots
        .bind("src", &file, span())
        .expect_err("a file is not a root");
    assert_eq!(rejected.code, codes::FS_ROOT_INVALID);
    assert!(roots.is_empty());
}

#[test]
fn an_unbound_label_names_the_flag_that_would_bind_it() {
    let d = unbound(Op::Lock, "src", span());
    assert_eq!(d.code, codes::FS_ROOT_UNBOUND);
    assert!(d.message.contains("`fs.lock`"), "{}", d.message);
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
    // A second taker waits out the holder and returns `false` rather than raising. Waiting the
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
fn a_lock_with_no_directory_to_sit_in_is_rejected_without_waiting() {
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

// --- Asking the host ----------------------------------------------------------

fn rooted(at: &Path) -> FsHost {
    let mut roots = Roots::new();
    roots.bind("cache", at, span()).expect("the root binds");
    FsHost::new(roots)
}

/// What a job returned, run on this thread.
fn ran(job: ply_host::jobs::FallibleJob) -> Result<Vec<u8>, Diagnostic> {
    job().map_err(|failure| match failure {
        Failure::Rejected(rejection) => rejection,
        Failure::Said(reason) => panic!("the job failed: {reason}"),
    })
}

/// The operation as the host handler asks it, under `label`.
fn asked(
    fs: &FsHost,
    op: Op,
    label: &str,
    path: &str,
    second: Second,
) -> Result<Vec<u8>, Diagnostic> {
    ran(fs.asked(op, label, path.to_string(), second, span())?)
}

/// The operation under `cache`.
fn perform(fs: &FsHost, op: Op, path: &str, second: Second) -> Result<Vec<u8>, Diagnostic> {
    asked(fs, op, "cache", path, second)
}

fn on(fs: &FsHost, op: Op, path: &str, second: Second) -> Vec<u8> {
    perform(fs, op, path, second).unwrap_or_else(|d| panic!("rejected: {} {}", d.code, d.message))
}

fn body(bytes: &[u8]) -> Second {
    Second::Body(bytes.to_vec())
}

fn other(path: &str) -> Second {
    Second::Path(path.to_string())
}

fn did(said: Vec<u8>) -> bool {
    match said.as_slice() {
        [1] => true,
        [0] => false,
        other => panic!("not whether it was done: {other:?}"),
    }
}

fn found(said: &[u8]) -> Option<&[u8]> {
    match said.split_first() {
        Some((1, rest)) => Some(rest),
        Some((0, [])) => None,
        _ => panic!("nothing found or not: {said:?}"),
    }
}

fn found_bytes(said: Vec<u8>) -> Option<Vec<u8>> {
    found(&said).map(<[u8]>::to_vec)
}

fn found_int(said: Vec<u8>) -> Option<i64> {
    found(&said).map(|n| int_of(n).expect("eight bytes"))
}

fn found_text(said: Vec<u8>) -> Option<String> {
    found(&said).map(|t| String::from_utf8(t.to_vec()).expect("text"))
}

fn kind(said: Vec<u8>) -> Kind {
    match said.as_slice() {
        [0] => Kind::File,
        [1] => Kind::Dir,
        [2] => Kind::Symlink,
        [3] => Kind::Missing,
        other => panic!("not a kind: {other:?}"),
    }
}

fn append(fs: &FsHost, path: &str, bytes: &[u8]) -> Option<i64> {
    found_int(on(fs, Op::Append, path, body(bytes)))
}

fn read_at(fs: &FsHost, path: &str, offset: i64, len: i64) -> Option<Vec<u8>> {
    found_bytes(on(fs, Op::ReadAt, path, Second::Range { offset, len }))
}

/// What a flush of an append-only cache does, and what its cost is: the frame, not the file.
#[test]
fn an_append_creates_the_log_and_returns_where_each_frame_landed() {
    let dir = root();
    let fs = rooted(dir.path());

    assert_eq!(append(&fs, "frontend.dat", b"header"), Some(0));
    assert_eq!(append(&fs, "frontend.dat", b"frame"), Some(6));
    assert_eq!(append(&fs, "frontend.dat", b""), Some(11));
    assert_eq!(append(&fs, "frontend.dat", b"tail"), Some(11));

    let whole = dir.path().canonicalize().unwrap().join("frontend.dat");
    assert_eq!(std::fs::read(&whole).unwrap(), b"headerframetail");
    // The offset an append returned is the one the frame is read back at.
    assert_eq!(read_at(&fs, "frontend.dat", 6, 5), Some(b"frame".to_vec()));
    assert_eq!(read_at(&fs, "frontend.dat", 11, 4), Some(b"tail".to_vec()));
}

#[test]
fn an_append_with_no_directory_to_hold_it_returns_none() {
    let dir = root();
    let fs = rooted(dir.path());
    assert_eq!(append(&fs, "absent/frontend.dat", b"frame"), None);
    // A directory is not a log either.
    std::fs::create_dir(dir.path().canonicalize().unwrap().join("sub")).unwrap();
    assert_eq!(append(&fs, "sub", b"frame"), None);
}

#[test]
fn a_ranged_read_returns_what_is_there_and_is_short_past_the_end() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::write(
        dir.path().canonicalize().unwrap().join("a.dat"),
        b"0123456789",
    )
    .unwrap();

    assert_eq!(read_at(&fs, "a.dat", 0, 10), Some(b"0123456789".to_vec()));
    assert_eq!(read_at(&fs, "a.dat", 3, 4), Some(b"3456".to_vec()));
    // Short rather than rejected: a file can end before a range a reader recorded earlier does.
    assert_eq!(read_at(&fs, "a.dat", 8, 100), Some(b"89".to_vec()));
    assert_eq!(read_at(&fs, "a.dat", 10, 5), Some(Vec::new()));
    assert_eq!(read_at(&fs, "a.dat", 99, 5), Some(Vec::new()));
    assert_eq!(read_at(&fs, "a.dat", 4, 0), Some(Vec::new()));
}

#[test]
fn a_ranged_read_of_what_is_not_a_file_is_none() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::create_dir(dir.path().canonicalize().unwrap().join("sub")).unwrap();
    assert_eq!(read_at(&fs, "absent.dat", 0, 4), None);
    assert_eq!(read_at(&fs, "sub", 0, 4), None);
}

#[test]
fn the_bound_is_on_one_call_and_points_at_the_ranged_read() {
    let dir = root();
    let fs = rooted(dir.path());
    let rejected = perform(
        &fs,
        Op::ReadAt,
        "a.dat",
        Second::Range {
            offset: 0,
            len: MAX_READ_BYTES as i64 + 1,
        },
    )
    .expect_err("more than one read returns");
    assert_eq!(rejected.code, codes::FS_FILE_TOO_LARGE);
    assert!(
        rejected.notes.iter().any(|n| n.contains("fs.read_at")),
        "the rejection should name what reads a large file: {:?}",
        rejected.notes
    );
    // The bound is reachable, so the largest allowed call is not itself rejected.
    assert_eq!(read_at(&fs, "a.dat", 0, MAX_READ_BYTES as i64), None);
}

#[test]
fn a_sync_flushes_a_file_or_a_directory_and_returns_false_for_neither() {
    let dir = root();
    let fs = rooted(dir.path());
    assert_eq!(append(&fs, "frontend.dat", b"frame"), Some(0));

    let sync = |path: &str| did(on(&fs, Op::Sync, path, Second::None));
    // The data file, then the directory whose rename must survive with it.
    assert!(sync("frontend.dat"));
    assert!(sync(""));
    assert!(!sync("absent.dat"));
}

/// Every operation names its root by its label, and a label no root is bound to reaches no file.
#[test]
fn every_operation_rejects_a_label_no_root_is_bound_to() {
    let dir = root();
    let fs = rooted(dir.path());
    for op in Op::ALL {
        for (path, second, _) in given(op, "a") {
            let rejected =
                asked(&fs, op, "elsewhere", &path, second).expect_err("`elsewhere` has no root");
            assert_eq!(rejected.code, codes::FS_ROOT_UNBOUND, "{}", op.what());
        }
    }
    for chunk in [Chunk::Read(1), Chunk::Write(b"x".to_vec()), Chunk::Close] {
        let rejected = fs
            .on_file("elsewhere", 1, chunk, span())
            .map(|_| ())
            .expect_err("`elsewhere` has no root");
        assert_eq!(rejected.code, codes::FS_ROOT_UNBOUND);
    }
}

// --- Copies, trees, temporary directories, modes, links, walks and stamps ---

fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_copy_carries_the_bytes_and_permission_bits_and_never_a_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::fs::set_permissions(real.join("a.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::create_dir(real.join("d")).unwrap();

    let copy = |from: &str, to: &str| did(on(&fs, Op::Copy, from, other(to)));
    assert!(copy("a.txt", "b.txt"));
    assert_eq!(std::fs::read(real.join("b.txt")).unwrap(), b"x");
    assert_eq!(file_mode(&real.join("b.txt")), 0o600);
    // Over a file that is there, as a write is.
    std::fs::write(real.join("c.txt"), b"old").unwrap();
    assert!(copy("a.txt", "c.txt"));
    assert_eq!(std::fs::read(real.join("c.txt")).unwrap(), b"x");
    assert!(!copy("d", "e"), "a directory is not copied as a file");
    assert!(!copy("a.txt", "d"), "nor is a file copied over one");
    assert!(
        !copy("a.txt", "absent/b.txt"),
        "the destination's directory must be there"
    );
    assert!(!copy("absent.txt", "f.txt"));
}

#[test]
fn a_tree_is_removed_whole_and_a_link_in_it_is_not_followed() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir_all(real.join("t/sub")).unwrap();
    std::fs::write(real.join("t/sub/f.txt"), b"f").unwrap();
    std::fs::write(real.join("keep.txt"), b"k").unwrap();
    std::os::unix::fs::symlink("../keep.txt", real.join("t/link")).unwrap();

    let remove_tree = |path: &str| did(on(&fs, Op::RemoveTree, path, Second::None));
    assert!(remove_tree("t"));
    assert!(!real.join("t").exists());
    assert_eq!(std::fs::read(real.join("keep.txt")).unwrap(), b"k");
    assert!(!remove_tree("absent"));
    for whole in ["", ".", "./"] {
        assert!(
            !remove_tree(whole),
            "`{whole}` names the root, which no operation removes"
        );
    }
    assert!(real.exists());
    assert!(remove_tree("keep.txt"), "a file is a tree of one");
}

#[test]
fn a_temporary_directory_is_new_each_time_and_named_as_the_root_names_it() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir(real.join("scratch")).unwrap();

    let temp = |under: &str, prefix: &str| {
        found_text(on(
            &fs,
            Op::TempDir,
            under,
            Second::Name(prefix.to_string()),
        ))
    };
    let one = temp("scratch", "t-").expect("a directory is made");
    let two = temp("./scratch/", "t-").expect("and another");
    assert_ne!(one, two);
    for made in [&one, &two] {
        assert!(made.starts_with("scratch/t-"), "{made}");
        assert!(real.join(made).is_dir(), "{made}");
    }
    let top = temp("", "x").expect("the root takes one too");
    assert!(top.starts_with('x') && !top.contains('/'), "{top}");
    assert_eq!(temp("scratch", "a/b"), None, "a prefix is part of one name");
    assert_eq!(temp("absent", "t-"), None);
}

#[test]
fn canonical_returns_the_real_path_and_nothing_for_a_missing_one() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::os::unix::fs::symlink("a.txt", real.join("l")).unwrap();

    let canonical = |path: &str| found_text(on(&fs, Op::Canonical, path, Second::None));
    let file = real.join("a.txt").display().to_string();
    assert_eq!(canonical("a.txt"), Some(file.clone()));
    assert_eq!(
        canonical("./l"),
        Some(file),
        "a link resolves to its target"
    );
    assert_eq!(canonical(""), Some(real.display().to_string()));
    assert_eq!(canonical("absent"), None);
}

#[test]
fn a_mode_reads_and_sets_the_nine_bits() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("run.sh"), b"#!/bin/sh\n").unwrap();

    assert!(did(on(&fs, Op::SetMode, "run.sh", Second::Mode(0o750))));
    assert_eq!(file_mode(&real.join("run.sh")), 0o750);
    assert_eq!(
        found_int(on(&fs, Op::Mode, "run.sh", Second::None)),
        Some(0o750)
    );
    assert_eq!(found_int(on(&fs, Op::Mode, "absent", Second::None)), None);
    assert!(!did(on(&fs, Op::SetMode, "absent", Second::Mode(0o644))));
}

#[test]
fn a_link_is_made_inside_the_root_and_one_out_of_it_is_rejected() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::fs::create_dir(real.join("d")).unwrap();

    let link = |path: &str, target: &str| {
        perform(&fs, Op::Symlink, path, Second::Target(target.to_string()))
    };
    let made = |path: &str, target: &str| did(link(path, target).expect("the link stays"));
    assert!(made("l", "a.txt"));
    assert!(made("d/up", "../a.txt"));
    assert_eq!(
        found_text(on(&fs, Op::ReadLink, "d/up", Second::None)),
        Some("../a.txt".to_string())
    );
    assert_eq!(kind(on(&fs, Op::Kind, "l", Second::None)), Kind::Symlink);
    assert_eq!(
        found_text(on(&fs, Op::ReadLink, "a.txt", Second::None)),
        None
    );
    assert!(
        !made("l", "a.txt"),
        "a path that is there is not made a link"
    );
    // A target is read from the directory the link is made in, wherever a link above it leads.
    std::fs::create_dir_all(real.join("d/e/f")).unwrap();
    std::os::unix::fs::symlink("..", real.join("d/back")).unwrap();
    std::os::unix::fs::symlink("e/f", real.join("d/deep")).unwrap();
    assert!(made("d/deep/up", "../../../a.txt"));
    assert_eq!(std::fs::read(real.join("d/e/f/up")).unwrap(), b"x");
    for (path, target) in [
        ("out", "../elsewhere"),
        ("d/out", "../../x"),
        ("abs", "/etc"),
        ("d/back/esc", "../x"),
    ] {
        let rejected = link(path, target).expect_err("the target leaves the root");
        assert_eq!(
            rejected.code,
            codes::FS_PATH_ESCAPES_ROOT,
            "{path} -> {target}"
        );
    }
}

/// The entries a walk returned, each its path and kind.
fn entries(said: Vec<u8>) -> Option<Vec<(String, Kind)>> {
    found(&said).map(|flat| {
        parts_of(flat)
            .expect("parts")
            .into_iter()
            .map(|entry| match parts_of(entry).expect("parts").as_slice() {
                [path, k] => (
                    String::from_utf8(path.to_vec()).expect("text"),
                    kind(k.to_vec()),
                ),
                other => panic!("not an entry: {other:?}"),
            })
            .collect()
    })
}

#[test]
fn a_walk_is_depth_first_in_byte_order_and_never_follows_a_link() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir_all(real.join("a/z")).unwrap();
    std::fs::write(real.join("a/b.txt"), b"b").unwrap();
    std::fs::write(real.join("a/z/y.txt"), b"y").unwrap();
    std::fs::write(real.join("a-c.txt"), b"c").unwrap();
    std::os::unix::fs::symlink("a", real.join("link")).unwrap();

    let walk = |path: &str| entries(on(&fs, Op::Walk, path, Second::None));
    let entry = |p: &str, k: Kind| (p.to_string(), k);
    assert_eq!(
        walk(""),
        Some(vec![
            entry("a", Kind::Dir),
            entry("a/b.txt", Kind::File),
            entry("a/z", Kind::Dir),
            entry("a/z/y.txt", Kind::File),
            entry("a-c.txt", Kind::File),
            entry("link", Kind::Symlink),
        ])
    );
    assert_eq!(
        walk("./a"),
        Some(vec![
            entry("a/b.txt", Kind::File),
            entry("a/z", Kind::Dir),
            entry("a/z/y.txt", Kind::File),
        ])
    );
    assert_eq!(walk("a/b.txt"), None);
    assert_eq!(walk("absent"), None);
}

#[test]
fn a_listing_says_of_each_name_in_byte_order_what_kind_says_of_its_path() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir_all(real.join("a/z")).unwrap();
    std::fs::write(real.join("a/b.txt"), b"b").unwrap();
    std::os::unix::fs::symlink("z", real.join("a/link")).unwrap();

    let listed = entries(on(&fs, Op::ListDir, "a", Second::None)).expect("a directory");
    let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["b.txt", "link", "z"]);
    for (name, listed_kind) in &listed {
        let path = format!("a/{name}");
        assert_eq!(kind(on(&fs, Op::Kind, &path, Second::None)), *listed_kind);
    }
    assert_eq!(entries(on(&fs, Op::ListDir, "a/b.txt", Second::None)), None);
}

#[test]
fn a_stamp_sets_the_modification_time() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::fs::create_dir(real.join("d")).unwrap();

    let stamp = |path: &str, ms: i64| did(on(&fs, Op::SetModified, path, Second::Millis(ms)));
    let modified = |path: &str| found_int(on(&fs, Op::ModifiedMs, path, Second::None));
    for path in ["a.txt", "d"] {
        assert!(stamp(path, 1_000_000_000_000), "{path}");
        assert_eq!(modified(path), Some(1_000_000_000_000), "{path}");
    }
    assert!(!stamp("absent", 1));
}

#[test]
fn every_operation_that_names_a_second_path_confines_it_too() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    for (op, path, second) in [
        (Op::Copy, "a.txt", other("../b.txt")),
        (Op::Copy, "../a.txt", other("b.txt")),
        (Op::RemoveTree, "../x", Second::None),
        (Op::TempDir, "../", Second::Name("t".into())),
        (Op::Walk, "/", Second::None),
        (Op::Canonical, "../a.txt", Second::None),
        (Op::Mode, "../a.txt", Second::None),
        (Op::SetMode, "../a.txt", Second::Mode(0o644)),
        (Op::Symlink, "../l", Second::Target("a.txt".into())),
        (Op::ReadLink, "../l", Second::None),
        (Op::SetModified, "../a.txt", Second::Millis(1)),
    ] {
        let rejected = perform(&fs, op, path, second).expect_err("it leaves the root");
        assert_eq!(rejected.code, codes::FS_PATH_ESCAPES_ROOT, "{}", op.what());
    }
}

// --- Open files ---

fn opened(fs: &FsHost, label: &str, path: &str, how: Opening) -> Result<i64, Vec<u8>> {
    let said = asked(fs, Op::Open, label, path, Second::Opening(how))
        .unwrap_or_else(|d| panic!("rejected: {} {}", d.code, d.message));
    match said.split_first() {
        Some((1, descriptor)) => Ok(int_of(descriptor).expect("eight bytes")),
        Some((0, reason)) => Err(reason.to_vec()),
        _ => panic!("not what an open returns: {said:?}"),
    }
}

fn on_file(fs: &FsHost, label: &str, descriptor: i64, chunk: Chunk) -> Vec<u8> {
    ran(fs
        .on_file(label, descriptor, chunk, span())
        .unwrap_or_else(|d| panic!("rejected: {} {}", d.code, d.message)))
    .unwrap_or_else(|d| panic!("rejected: {} {}", d.code, d.message))
}

fn chunk(fs: &FsHost, label: &str, descriptor: i64, max: u64) -> Option<Vec<u8>> {
    found_bytes(on_file(fs, label, descriptor, Chunk::Read(max)))
}

fn closed(fs: &FsHost, label: &str, descriptor: i64) -> bool {
    did(on_file(fs, label, descriptor, Chunk::Close))
}

#[test]
fn a_descriptor_is_open_under_the_root_that_opened_it_until_it_is_closed() {
    let (here, there) = (root(), root());
    std::fs::write(here.path().join("log"), b"abcdef").unwrap();
    let mut roots = Roots::new();
    roots.bind("cache", here.path(), span()).unwrap();
    roots.bind("other", there.path(), span()).unwrap();
    let fs = FsHost::new(roots);

    let first = opened(&fs, "cache", "log", Opening::Read).expect("the file opens");
    assert_eq!(chunk(&fs, "cache", first, 4), Some(b"abcd".to_vec()));
    // Another root's label reaches nothing this root opened: not to read it, not to close it.
    assert_eq!(chunk(&fs, "other", first, 4), None);
    assert!(!closed(&fs, "other", first));
    assert_eq!(chunk(&fs, "cache", first, 4), Some(b"ef".to_vec()));
    assert_eq!(chunk(&fs, "cache", first, 4), Some(Vec::new()));

    assert!(closed(&fs, "cache", first));
    assert!(!closed(&fs, "cache", first));
    assert_eq!(chunk(&fs, "cache", first, 4), None);
    // A descriptor names one open, so the next open returns another.
    let second = opened(&fs, "cache", "log", Opening::Read).expect("the file opens again");
    assert_ne!(second, first);
}

#[test]
fn an_open_says_why_a_file_did_not_open_and_is_confined_like_every_other() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::create_dir(dir.path().join("d")).unwrap();
    assert_eq!(
        opened(&fs, "cache", "absent", Opening::Read),
        Err(FileError::NotFound.flat())
    );
    for how in [Opening::Read, Opening::Write, Opening::Append] {
        assert_eq!(
            opened(&fs, "cache", "d", how),
            Err(FileError::NotAFile.flat())
        );
    }
    assert_eq!(
        opened(&fs, "cache", "no/f", Opening::Write),
        Err(FileError::NotFound.flat())
    );

    let rejected = perform(&fs, Op::Open, "../escape", Second::Opening(Opening::Read))
        .expect_err("it leaves the root");
    assert_eq!(rejected.code, codes::FS_PATH_ESCAPES_ROOT);
}

#[test]
fn a_chunk_costs_at_most_what_it_asks_for_and_the_bound_is_on_one_read() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::write(dir.path().join("log"), vec![7u8; 4096]).unwrap();
    let file = opened(&fs, "cache", "log", Opening::Read).expect("the file opens");
    assert_eq!(chunk(&fs, "cache", file, 0), Some(Vec::new()));
    assert_eq!(chunk(&fs, "cache", file, 100).map(|b| b.len()), Some(100));

    let over = fs
        .on_file("cache", file, Chunk::Read(MAX_READ_BYTES + 1), span())
        .map(|_| ())
        .expect_err("more than one read returns");
    assert_eq!(over.code, codes::FS_FILE_TOO_LARGE);
    // The rejection did not move the descriptor.
    assert_eq!(chunk(&fs, "cache", file, 4096).map(|b| b.len()), Some(3996));
}

// --- Exclusive creates, looks, scans, names and room ---

#[test]
fn a_file_made_exclusively_is_its_owners_alone_and_is_made_only_where_nothing_is() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir(real.join("d")).unwrap();
    std::fs::write(real.join("d/a.txt"), b"x").unwrap();
    std::os::unix::fs::symlink("nowhere", real.join("d/l")).unwrap();

    let made = opened(&fs, "cache", "d/new", Opening::Create).expect("nothing is there");
    assert!(did(on_file(
        &fs,
        "cache",
        made,
        Chunk::Write(b"mine".to_vec())
    )));
    assert!(closed(&fs, "cache", made));
    assert_eq!(std::fs::read(real.join("d/new")).unwrap(), b"mine");
    assert_eq!(file_mode(&real.join("d/new")), 0o600);

    for there in ["d/new", "d/a.txt", "d", "d/l"] {
        assert_eq!(
            opened(&fs, "cache", there, Opening::Create),
            Err(FileError::Exists.flat()),
            "{there}"
        );
    }
    // The link was rejected as itself: nothing was made where it points.
    assert!(!real.join("d/nowhere").exists());
    assert_eq!(
        opened(&fs, "cache", "no/new", Opening::Create),
        Err(FileError::NotFound.flat())
    );
    assert_eq!(
        opened(&fs, "cache", "d/a.txt/new", Opening::Create),
        Err(FileError::NotADirectory.flat())
    );
}

fn sealed(encoding: &str, secret: &[u8]) -> Second {
    let framing = Framing::named(encoding, Op::WriteSecret.what(), span()).expect("an encoding");
    Second::Sealed(framing.framed(b"key ", secret, b"\n"))
}

fn write_secret(fs: &FsHost, path: &str, encoding: &str) -> Result<Vec<u8>, Diagnostic> {
    perform(fs, Op::WriteSecret, path, sealed(encoding, b"\x01\x23\xab"))
}

/// The secret a file holds between `before` and `after`, kept by a run and taken back from it.
fn read_secret(
    fs: &FsHost,
    path: &str,
    before: &[u8],
    encoding: &str,
    after: &[u8],
) -> Option<Vec<u8>> {
    let run = Run::new(Arc::new(Bell::default()));
    let framing = Framing::named(encoding, Op::ReadSecret.what(), span()).expect("an encoding");
    let said = on(
        fs,
        Op::ReadSecret,
        path,
        Second::Frame {
            before: before.to_vec(),
            framing,
            after: after.to_vec(),
            keeper: Arc::downgrade(&run),
        },
    );
    found_int(said).map(|number| run.take_secret(number).expect("the run keeps it").to_vec())
}

#[test]
fn a_secret_is_written_in_its_frame_as_its_owners_alone_whatever_was_there() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("old.key"), b"a longer plain file than the key").unwrap();
    std::fs::set_permissions(real.join("old.key"), std::fs::Permissions::from_mode(0o644)).unwrap();

    for (path, encoding, line) in [
        ("new.key", "hex", &b"key 0123ab\n"[..]),
        ("old.key", "base64", b"key ASOr\n"),
        ("raw.key", "raw", b"key \x01\x23\xab\n"),
    ] {
        assert!(
            did(write_secret(&fs, path, encoding).expect("a write")),
            "{path}"
        );
        assert_eq!(std::fs::read(real.join(path)).unwrap(), line, "{path}");
        assert_eq!(file_mode(&real.join(path)), 0o600, "{path}");
        assert_eq!(
            read_secret(&fs, path, b"key ", encoding, b"\n"),
            Some(b"\x01\x23\xab".to_vec()),
            "{path}"
        );
    }
    assert!(!did(
        write_secret(&fs, "no/new.key", "hex").expect("a write")
    ));
    assert!(!real.join("no").exists());
}

#[test]
fn a_secret_is_read_back_only_from_inside_the_frame_it_was_written_in() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.key"), b"key 0123ab\n").unwrap();
    std::fs::write(real.join("upper.key"), b"key 0123AB\n").unwrap();
    std::fs::write(real.join("odd.key"), b"key 0123a\n").unwrap();

    assert_eq!(read_secret(&fs, "a.key", b"key ", "hex", b""), None);
    assert_eq!(read_secret(&fs, "a.key", b"kex ", "hex", b"\n"), None);
    assert_eq!(
        read_secret(&fs, "a.key", b"key 0123ab", "hex", b"\n"),
        Some(Vec::new())
    );
    assert_eq!(
        read_secret(&fs, "a.key", b"key 0123ab\n", "raw", b"\n"),
        None
    );
    assert_eq!(read_secret(&fs, "upper.key", b"key ", "hex", b"\n"), None);
    assert_eq!(read_secret(&fs, "odd.key", b"key ", "hex", b"\n"), None);
    assert_eq!(read_secret(&fs, "none.key", b"", "raw", b""), None);
    assert_eq!(
        read_secret(&fs, "a.key", b"", "raw", b""),
        Some(b"key 0123ab\n".to_vec())
    );
}

/// A secret read after its run ended is kept by no one, and returns as one not there.
#[test]
fn a_secret_read_for_a_run_that_has_ended_is_stored_by_no_one() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.key"), b"key 0123ab\n").unwrap();
    let framing = Framing::named("hex", Op::ReadSecret.what(), span()).expect("an encoding");
    let said = on(
        &fs,
        Op::ReadSecret,
        "a.key",
        Second::Frame {
            before: b"key ".to_vec(),
            framing,
            after: b"\n".to_vec(),
            keeper: Weak::new(),
        },
    );
    assert_eq!(found_int(said), None);
}

#[test]
fn a_secret_write_rejects_an_unknown_encoding_and_a_path_out_of_the_root() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    for what in [Op::WriteSecret.what(), Op::ReadSecret.what()] {
        let Err(unknown) = Framing::named("rot13", what, span()) else {
            panic!("no such encoding")
        };
        assert_eq!(unknown.code, codes::RUNTIME_ERROR);
        assert!(unknown.message.contains(what), "{}", unknown.message);
    }
    let escaped = write_secret(&fs, "../out.key", "hex").expect_err("it leaves the root");
    assert_eq!(escaped.code, codes::FS_PATH_ESCAPES_ROOT);
    assert!(std::fs::read_dir(&real).unwrap().next().is_none());
}

/// What one look read, as [`Stat::flat`] writes it.
fn looked(flat: &[u8]) -> Stat {
    let [k, size, modified, mode, links, device, id] = parts_of(flat)
        .expect("parts")
        .try_into()
        .expect("seven parts");
    let int = |n: &[u8]| int_of(n).expect("eight bytes");
    Stat {
        kind: kind(k.to_vec()),
        size: int(size),
        modified: int(modified),
        mode: u32::try_from(int(mode)).expect("nine bits"),
        links: int(links),
        device: int(device),
        id: int(id),
    }
}

fn stat(fs: &FsHost, path: &str) -> Option<Stat> {
    found(&on(fs, Op::Stat, path, Second::None)).map(looked)
}

fn linked(fs: &FsHost, path: &str, to: &str) -> Result<(), Vec<u8>> {
    let said = on(fs, Op::Link, path, other(to));
    match said.split_first() {
        Some((1, [])) => Ok(()),
        Some((0, reason)) => Err(reason.to_vec()),
        _ => panic!("not what a link returns: {said:?}"),
    }
}

#[test]
fn one_look_reads_a_path_as_itself_and_nothing_for_what_is_not_there() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir(real.join("d")).unwrap();
    std::fs::write(real.join("d/a.txt"), b"abc").unwrap();
    std::fs::set_permissions(real.join("d/a.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("a.txt", real.join("d/l")).unwrap();

    let meta = std::fs::metadata(real.join("d/a.txt")).unwrap();
    let file = stat(&fs, "d/a.txt").expect("a file is there");
    assert_eq!(
        file,
        Stat {
            kind: Kind::File,
            size: 3,
            modified: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            mode: 0o640,
            links: 1,
            device: meta.dev() as i64,
            id: meta.ino() as i64,
        }
    );

    let folder = stat(&fs, "d").expect("a directory is there");
    assert_eq!((folder.kind, folder.size, folder.links), (Kind::Dir, 0, 1));
    assert_eq!(folder.mode, file_mode(&real.join("d")));

    // The link is read, not followed: it is no file, and it is not the file it points at.
    let link = stat(&fs, "d/l").expect("a link is there");
    assert_eq!(
        (link.kind, link.size, link.mode, link.links),
        (Kind::Symlink, 0, 0o777, 1)
    );
    assert_ne!(link.id, file.id);

    assert_eq!(stat(&fs, "d/absent"), None);
    let rejected = perform(&fs, Op::Stat, "../d", Second::None).expect_err("it leaves the root");
    assert_eq!(rejected.code, codes::FS_PATH_ESCAPES_ROOT);
}

#[test]
fn a_scan_is_a_walk_with_a_look_at_each_entry_and_goes_below_only_when_asked() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir_all(real.join("a/z")).unwrap();
    std::fs::write(real.join("a/b.txt"), b"b").unwrap();
    std::fs::write(real.join("a/z/y.txt"), b"yy").unwrap();
    std::fs::write(real.join("a-c.txt"), b"").unwrap();
    std::os::unix::fs::symlink("a", real.join("link")).unwrap();

    let scan = |path: &str, deep: bool| -> Option<Vec<(String, Kind, i64)>> {
        let said = on(&fs, Op::Scan, path, Second::Deep(deep));
        found(&said).map(|flat| {
            parts_of(flat)
                .expect("parts")
                .into_iter()
                .map(|seen| match parts_of(seen).expect("parts").as_slice() {
                    [path, stat] => {
                        let found = looked(stat);
                        (
                            String::from_utf8(path.to_vec()).expect("text"),
                            found.kind,
                            found.size,
                        )
                    }
                    other => panic!("not an entry: {other:?}"),
                })
                .collect()
        })
    };
    let entry = |p: &str, k: Kind, size: i64| (p.to_string(), k, size);
    assert_eq!(
        scan("", true),
        Some(vec![
            entry("a", Kind::Dir, 0),
            entry("a/b.txt", Kind::File, 1),
            entry("a/z", Kind::Dir, 0),
            entry("a/z/y.txt", Kind::File, 2),
            entry("a-c.txt", Kind::File, 0),
            entry("link", Kind::Symlink, 0),
        ])
    );
    assert_eq!(
        scan("", false),
        Some(vec![
            entry("a", Kind::Dir, 0),
            entry("a-c.txt", Kind::File, 0),
            entry("link", Kind::Symlink, 0),
        ])
    );
    assert_eq!(
        scan("./a/", false),
        Some(vec![
            entry("a/b.txt", Kind::File, 1),
            entry("a/z", Kind::Dir, 0)
        ])
    );
    assert_eq!(scan("a/b.txt", true), None);
    assert_eq!(scan("absent", true), None);
}

#[test]
fn a_file_takes_a_second_name_and_nothing_else_does() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir(real.join("d")).unwrap();
    std::fs::write(real.join("d/a.txt"), b"one").unwrap();
    std::os::unix::fs::symlink("d/a.txt", real.join("l")).unwrap();

    assert_eq!(linked(&fs, "twin.txt", "d/a.txt"), Ok(()));
    std::fs::write(real.join("twin.txt"), b"both").unwrap();
    assert_eq!(std::fs::read(real.join("d/a.txt")).unwrap(), b"both");
    let (first, second) = (
        stat(&fs, "d/a.txt").expect("the file"),
        stat(&fs, "twin.txt").expect("its second name"),
    );
    assert_eq!((first.links, second.links), (2, 2));
    assert_eq!((first.device, first.id), (second.device, second.id));

    // A copy onto the file itself, by either name, would empty it before reading it.
    for (from, to) in [("twin.txt", "d/a.txt"), ("d/a.txt", "d/a.txt")] {
        assert!(did(on(&fs, Op::Copy, from, other(to))), "{from} -> {to}");
        assert_eq!(std::fs::read(real.join("d/a.txt")).unwrap(), b"both");
    }

    let rejected = |path: &str, to: &str, reason: FileError| {
        assert_eq!(linked(&fs, path, to), Err(reason.flat()), "{path} -> {to}");
    };
    rejected("n", "absent", FileError::NotFound);
    rejected("n", "d", FileError::NotAFile);
    rejected("n", "l", FileError::NotAFile);
    rejected("l", "d/a.txt", FileError::Exists);
    rejected("twin.txt", "d/a.txt", FileError::Exists);
    rejected("no/n", "d/a.txt", FileError::NotFound);
    rejected("d/a.txt/n", "d/a.txt", FileError::NotADirectory);
    assert!(!real.join("n").exists());

    for (path, to) in [("../n", "d/a.txt"), ("n", "/d/a.txt")] {
        let rejected = perform(&fs, Op::Link, path, other(to)).expect_err("it leaves the root");
        assert_eq!(rejected.code, codes::FS_PATH_ESCAPES_ROOT, "{path} -> {to}");
    }
}

#[test]
fn the_room_of_a_file_system_is_returned_for_a_path_that_names_something() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();

    let room = |path: &str| -> Option<[i64; 5]> {
        found(&on(&fs, Op::Space, path, Second::None)).map(|flat| {
            let parts: [&[u8]; 5] = parts_of(flat)
                .expect("parts")
                .try_into()
                .expect("five parts");
            parts.map(|n| int_of(n).expect("eight bytes"))
        })
    };
    let [total, free, available, inodes, inodes_free] = room("").expect("the root is there");
    assert!(total > 0 && free <= total && available <= free);
    assert!(inodes_free <= inodes);
    // One file system holds the root and the file in it.
    assert_eq!(room("a.txt").map(|r| r[0]), Some(total));
    assert_eq!(room("absent"), None);
}

// --- Links that lead out of the root ---

/// Where a link that leads out of the root stands in the path an operation is given.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Way {
    /// The last name is a link to a path outside the root where nothing is.
    Dangling,
    /// The last name is a link to a file outside the root.
    ToFile,
    /// A name below a link to a path outside the root where nothing is.
    BelowDangling,
    /// A file outside the root, named through a link to its directory.
    Through,
    /// A name that is not there, through a link to a directory outside the root.
    ThroughToNew,
    /// `..` after a link to a directory inside the root.
    Climbing,
}

impl Way {
    const ALL: [Way; 6] = [
        Way::Dangling,
        Way::ToFile,
        Way::BelowDangling,
        Way::Through,
        Way::ThroughToNew,
        Way::Climbing,
    ];

    fn path(self) -> &'static str {
        match self {
            Way::Dangling => "gone",
            Way::ToFile => "file",
            Way::BelowDangling => "gone/new",
            Way::Through => "out/secret.txt",
            Way::ThroughToNew => "out/new",
            Way::Climbing => "in/../plain.txt",
        }
    }

    /// Whether the path names the link, which an operation that follows no link acts on as itself.
    fn names_the_link(self) -> bool {
        matches!(self, Way::Dangling | Way::ToFile)
    }
}

/// A root holding a file, a directory and the links each `Way` names, and the directory outside it
/// that they lead to.
fn linked_out() -> (tempfile::TempDir, tempfile::TempDir) {
    let (inside, outside) = (root(), root());
    let (here, there) = (inside.path(), outside.path().canonicalize().unwrap());
    std::fs::write(there.join("secret.txt"), b"secret").unwrap();
    std::fs::create_dir(there.join("kept")).unwrap();
    std::fs::write(there.join("kept/more.txt"), b"more").unwrap();
    std::fs::write(here.join("plain.txt"), b"plain").unwrap();
    std::fs::create_dir(here.join("dir")).unwrap();
    std::fs::write(here.join("dir/inner.txt"), b"inner").unwrap();
    let link = |to: PathBuf, name: &str| std::os::unix::fs::symlink(to, here.join(name)).unwrap();
    link(there.join("gone"), "gone");
    link(there.join("secret.txt"), "file");
    link(there.clone(), "out");
    link(PathBuf::from("dir"), "in");
    (inside, outside)
}

/// Everything at and under `dir`: each path with its mode, how many names it has, when it was
/// modified and a file's bytes.
fn held(dir: &Path) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let mut ahead = vec![dir.to_path_buf()];
    while let Some(at) = ahead.pop() {
        let meta = std::fs::symlink_metadata(&at).unwrap();
        let body = if meta.is_file() {
            std::fs::read(&at).unwrap()
        } else {
            Vec::new()
        };
        out.push(format!(
            "{} {:o} {} {}.{} {body:?}",
            at.display(),
            meta.mode(),
            meta.nlink(),
            meta.mtime(),
            meta.mtime_nsec()
        ));
        if meta.is_dir() {
            ahead.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
        }
    }
    out.sort();
    out
}

/// Every way `op` is given `path`: the path it is asked of, what it is handed beside it, and
/// whether it then acts on a link that is the path's last name rather than on what the link leads
/// to. No arm is a wildcard, so an operation added to the family does not compile until it says
/// how it takes a path.
fn given(op: Op, path: &str) -> Vec<(String, Second, bool)> {
    let p = || path.to_string();
    let plain = || "plain.txt".to_string();
    match op {
        Op::ReadFile
        | Op::ListDir
        | Op::Resolved
        | Op::FileSize
        | Op::ModifiedMs
        | Op::CreateDir
        | Op::Sync
        | Op::Canonical
        | Op::Mode
        | Op::Walk
        | Op::Space => vec![(p(), Second::None, false)],
        Op::Kind
        | Op::Exists
        | Op::Remove
        | Op::Lock
        | Op::Unlock
        | Op::RemoveTree
        | Op::ReadLink
        | Op::Stat => vec![(p(), Second::None, true)],
        Op::ReadAt => vec![(p(), Second::Range { offset: 0, len: 4 }, false)],
        Op::WriteFile | Op::Append => vec![(p(), body(b"x"), false)],
        Op::WriteSecret => vec![(p(), sealed("hex", b"x"), false)],
        Op::ReadSecret => vec![(
            p(),
            Second::Frame {
                before: Vec::new(),
                framing: Framing::named("raw", op.what(), span()).expect("an encoding"),
                after: Vec::new(),
                keeper: Weak::new(),
            },
            false,
        )],
        Op::TempDir => vec![(p(), Second::Name("t-".into()), false)],
        Op::SetMode => vec![(p(), Second::Mode(0o600), false)],
        Op::SetModified => vec![(p(), Second::Millis(1000), false)],
        Op::Scan => vec![(p(), Second::Deep(true), false)],
        Op::Symlink => vec![(p(), Second::Target(plain()), true)],
        Op::Open => vec![
            (p(), Second::Opening(Opening::Read), false),
            (p(), Second::Opening(Opening::Write), false),
            (p(), Second::Opening(Opening::Append), false),
            (p(), Second::Opening(Opening::Create), true),
        ],
        Op::Rename => vec![
            (p(), other("moved"), true),
            (plain(), Second::Path(p()), true),
        ],
        Op::Copy => vec![
            (p(), other("copied"), false),
            (plain(), Second::Path(p()), false),
        ],
        Op::Link => vec![
            (p(), Second::Path(plain()), true),
            ("named".to_string(), Second::Path(p()), true),
        ],
        // An open file is named by the descriptor `open` returned, and takes no path.
        Op::ReadChunk | Op::WriteChunk | Op::Close => Vec::new(),
    }
}

#[test]
fn no_operation_reaches_through_a_link_out_of_the_root() {
    for op in Op::ALL {
        for way in Way::ALL {
            for (path, second, itself) in given(op, way.path()) {
                let (inside, outside) = linked_out();
                let fs = rooted(inside.path());
                let before = held(outside.path());
                let result = perform(&fs, op, &path, second);
                let said = format!("{} given `{}` ({way:?})", op.what(), way.path());
                if itself && way.names_the_link() {
                    assert!(result.is_ok(), "{said} acts on the link itself");
                } else {
                    let rejected = result
                        .err()
                        .unwrap_or_else(|| panic!("{said} was not rejected"));
                    assert_eq!(rejected.code, codes::FS_PATH_ESCAPES_ROOT, "{said}");
                }
                assert_eq!(
                    held(outside.path()),
                    before,
                    "{said} changed what is outside the root"
                );
            }
        }
    }
}

#[test]
fn an_operation_on_a_link_itself_reads_moves_and_removes_one_that_leads_out() {
    for name in ["gone", "file", "out"] {
        let (inside, outside) = linked_out();
        let here = inside.path().canonicalize().unwrap();
        let fs = rooted(&here);
        let before = held(outside.path());
        let target = std::fs::read_link(here.join(name)).unwrap();

        assert_eq!(kind(on(&fs, Op::Kind, name, Second::None)), Kind::Symlink);
        assert!(did(on(&fs, Op::Exists, name, Second::None)));
        assert_eq!(stat(&fs, name).map(|found| found.kind), Some(Kind::Symlink));
        assert_eq!(
            found_text(on(&fs, Op::ReadLink, name, Second::None)),
            target.to_str().map(str::to_string)
        );
        assert_eq!(
            opened(&fs, "cache", name, Opening::Create),
            Err(FileError::Exists.flat())
        );
        assert_eq!(linked(&fs, "named", name), Err(FileError::NotAFile.flat()));
        assert_eq!(
            linked(&fs, name, "plain.txt"),
            Err(FileError::Exists.flat())
        );
        assert!(!did(on(
            &fs,
            Op::Symlink,
            name,
            Second::Target("plain.txt".into())
        )));

        assert!(did(on(&fs, Op::Rename, name, other("moved"))));
        assert_eq!(std::fs::read_link(here.join("moved")).unwrap(), target);
        std::fs::write(here.join("new.txt"), b"new").unwrap();
        assert!(did(on(&fs, Op::Rename, "new.txt", other("moved"))));
        assert_eq!(std::fs::read(here.join("moved")).unwrap(), b"new");

        std::os::unix::fs::symlink(&target, here.join("again")).unwrap();
        let remove = if name == "out" {
            Op::RemoveTree
        } else {
            Op::Remove
        };
        assert!(did(on(&fs, remove, "again", Second::None)));
        assert!(std::fs::symlink_metadata(here.join("again")).is_err());
        assert_eq!(held(outside.path()), before, "through `{name}`");
    }
}

/// A chain of `links` links under `at`, the first named `first`, the last leading to `to`.
fn chain(at: &Path, first: &str, links: usize, to: &str) {
    for i in 0..links {
        let name = if i == 0 {
            first.to_string()
        } else {
            format!("{first}-{i}")
        };
        let next = if i + 1 == links {
            to.to_string()
        } else {
            format!("{first}-{}", i + 1)
        };
        std::os::unix::fs::symlink(next, at.join(name)).unwrap();
    }
}

#[test]
fn a_link_inside_the_root_is_followed_as_far_as_a_path_may_pass_and_no_further() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::create_dir(real.join("d")).unwrap();
    std::fs::write(real.join("d/a.txt"), b"a").unwrap();
    let read = |path: &str| found_bytes(on(&fs, Op::ReadFile, path, Second::None));
    let write = |path: &str| did(on(&fs, Op::WriteFile, path, body(b"w")));

    // A link to what is not there yet is written through, and one that names the root's own
    // absolute path stays inside it.
    std::os::unix::fs::symlink("d/new.txt", real.join("fresh")).unwrap();
    assert!(write("fresh"));
    assert_eq!(std::fs::read(real.join("d/new.txt")).unwrap(), b"w");
    std::os::unix::fs::symlink(real.join("d"), real.join("whole")).unwrap();
    assert_eq!(read("whole/a.txt"), Some(b"a".to_vec()));
    std::os::unix::fs::symlink("..", real.join("d/up")).unwrap();
    assert_eq!(read("d/up/d/a.txt"), Some(b"a".to_vec()));

    chain(&real, "forty", 40, "d/a.txt");
    assert_eq!(read("forty"), Some(b"a".to_vec()));
    chain(&real, "more", 41, "d/a.txt");
    chain(&real, "round", 2, "round");
    for nowhere in ["more", "round", "round/below"] {
        assert_eq!(read(nowhere), None, "{nowhere}");
        assert!(!write(nowhere), "{nowhere}");
        assert_eq!(
            opened(&fs, "cache", nowhere, Opening::Write),
            Err(FileError::NotFound.flat()),
            "{nowhere}"
        );
        assert_eq!(
            kind(on(&fs, Op::Resolved, nowhere, Second::None)),
            Kind::Missing,
            "{nowhere}"
        );
    }
    assert_eq!(std::fs::read(real.join("d/a.txt")).unwrap(), b"a");
    assert_eq!(
        kind(on(&fs, Op::Kind, "round", Second::None)),
        Kind::Symlink
    );

    // A link's target that steps back out of a name that is not there names nothing, as the
    // system resolves it.
    std::os::unix::fs::symlink("absent/../d/a.txt", real.join("back")).unwrap();
    assert_eq!(read("back"), None);
    assert!(!write("back"));
}

/// What a read appends is found after the leading byte, and a read that fails finds nothing.
#[test]
fn a_read_is_found_in_place_or_not_at_all() {
    let read = found_read(3, |out| {
        out.extend_from_slice(b"abc");
        Ok(3)
    });
    assert_eq!(found_bytes(read), Some(b"abc".to_vec()));
    let failed = found_read(3, |_| Err(std::io::Error::other("unreadable")));
    assert_eq!(found_bytes(failed), None);
}
