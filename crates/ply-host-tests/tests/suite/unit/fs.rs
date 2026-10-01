use ply_eval::host::MachineId;
use ply_eval::{
    Diagnostic, EffectAtom, HostAnswer, HostRequest, HostRuntime, Mode, Resource, Span, Symbol,
    Value, codes,
};
use ply_host::fs::*;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
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

// --- Appending, ranged reads and syncing ------------------------------------

struct Nothing;

impl HostRuntime for Nothing {
    fn watch(&self, _: &ply_eval::Pending) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }
    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn block_on(&self, _: ply_eval::Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Unit)
    }
}

fn rooted(at: &std::path::Path) -> Arc<FsHost> {
    let mut roots = Roots::new();
    roots.bind("cache", at, span()).expect("the root binds");
    Arc::new(FsHost::new(roots))
}

/// The operation as a run performs it: through the handler, the pool, and back as a `Value`.
fn perform(fs: &Arc<FsHost>, op: Op, label: &str, args: &[Value]) -> Result<Value, Diagnostic> {
    let handlers = registrations(fs);
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == op.name())
        .expect("every operation is registered");
    let answer = handler.call(
        &Nothing,
        &HostRequest {
            atom: EffectAtom::new(
                Symbol::new(EFFECT),
                Resource::Named(Symbol::new(label)),
                Mode::Write,
            ),
            op: declaration,
            args,
            span: span(),
            machine: MachineId(0),
            task: None,
            declared: None,
        },
    )?;
    match answer {
        HostAnswer::Pending(pending) => fs.block_on(pending),
        HostAnswer::Value(v) => Ok(v),
    }
}

fn done(answer: Result<Value, Diagnostic>) -> Value {
    answer.unwrap_or_else(|d| panic!("refused: {} {}", d.code, d.message))
}

fn option(v: &Value) -> Option<&Value> {
    match v {
        Value::Ctor { name, args } if name.as_str() == "Some" => Some(&args[0]),
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("not an Option: {}", other.type_name()),
    }
}

fn maybe_bytes(v: &Value) -> Option<Vec<u8>> {
    option(v).map(|inner| match inner {
        Value::Bytes(b) => b.to_vec(),
        other => panic!("not Bytes: {}", other.type_name()),
    })
}

fn maybe_int(v: &Value) -> Option<i64> {
    option(v).map(|inner| match inner {
        Value::Int(n) => *n,
        other => panic!("not an Int: {}", other.type_name()),
    })
}

fn boolean(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        other => panic!("not a Bool: {}", other.type_name()),
    }
}

fn append(fs: &Arc<FsHost>, path: &str, body: &[u8]) -> Option<i64> {
    maybe_int(&done(perform(
        fs,
        Op::Append,
        "cache",
        &[Value::str(path), Value::bytes(body)],
    )))
}

fn read_at(fs: &Arc<FsHost>, path: &str, offset: i64, len: i64) -> Option<Vec<u8>> {
    maybe_bytes(&done(perform(
        fs,
        Op::ReadAt,
        "cache",
        &[Value::str(path), Value::Int(offset), Value::Int(len)],
    )))
}

/// What a flush of an append-only cache does, and what its cost is: the frame, not the file.
#[test]
fn an_append_creates_the_log_and_answers_where_each_frame_landed() {
    let dir = root();
    let fs = rooted(dir.path());

    assert_eq!(append(&fs, "frontend.dat", b"header"), Some(0));
    assert_eq!(append(&fs, "frontend.dat", b"frame"), Some(6));
    assert_eq!(append(&fs, "frontend.dat", b""), Some(11));
    assert_eq!(append(&fs, "frontend.dat", b"tail"), Some(11));

    let whole = dir.path().canonicalize().unwrap().join("frontend.dat");
    assert_eq!(std::fs::read(&whole).unwrap(), b"headerframetail");
    // The offset an append answered is the one the frame is read back at.
    assert_eq!(read_at(&fs, "frontend.dat", 6, 5), Some(b"frame".to_vec()));
    assert_eq!(read_at(&fs, "frontend.dat", 11, 4), Some(b"tail".to_vec()));
}

#[test]
fn an_append_with_no_directory_to_hold_it_answers_none() {
    let dir = root();
    let fs = rooted(dir.path());
    assert_eq!(append(&fs, "absent/frontend.dat", b"frame"), None);
    // A directory is not a log either.
    std::fs::create_dir(dir.path().canonicalize().unwrap().join("sub")).unwrap();
    assert_eq!(append(&fs, "sub", b"frame"), None);
}

#[test]
fn a_ranged_read_answers_what_is_there_and_is_short_past_the_end() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::write(
        dir.path().canonicalize().unwrap().join("a.dat"),
        b"0123456789",
    )
    .unwrap();

    assert_eq!(read_at(&fs, "a.dat", 0, 10), Some(b"0123456789".to_vec()));
    assert_eq!(read_at(&fs, "a.dat", 3, 4), Some(b"3456".to_vec()));
    // Short rather than refused: a file can end before a range a reader recorded earlier does.
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

/// A file cannot answer a negative range, so it is arithmetic that went wrong rather than a state.
#[test]
fn a_negative_offset_or_length_is_refused_rather_than_clamped() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::write(dir.path().canonicalize().unwrap().join("a.dat"), b"0123").unwrap();

    for (offset, len) in [(-1, 4), (0, -1), (-1, -1)] {
        let refused = perform(
            &fs,
            Op::ReadAt,
            "cache",
            &[Value::str("a.dat"), Value::Int(offset), Value::Int(len)],
        )
        .expect_err("a negative range");
        assert_eq!(refused.code, codes::RUNTIME_ERROR, "{offset}..{len}");
    }
}

#[test]
fn the_bound_is_on_one_call_and_points_at_the_ranged_read() {
    let dir = root();
    let fs = rooted(dir.path());
    let refused = perform(
        &fs,
        Op::ReadAt,
        "cache",
        &[
            Value::str("a.dat"),
            Value::Int(0),
            Value::Int(MAX_READ_BYTES as i64 + 1),
        ],
    )
    .expect_err("more than one read answers");
    assert_eq!(refused.code, codes::FS_FILE_TOO_LARGE);
    assert!(
        refused.notes.iter().any(|n| n.contains("fs.read_at")),
        "the refusal should name what reads a large file: {:?}",
        refused.notes
    );
    // The bound is reachable, so the largest allowed call is not itself refused.
    assert_eq!(read_at(&fs, "a.dat", 0, MAX_READ_BYTES as i64), None);
}

#[test]
fn a_sync_flushes_a_file_or_a_directory_and_answers_false_for_neither() {
    let dir = root();
    let fs = rooted(dir.path());
    assert_eq!(append(&fs, "frontend.dat", b"frame"), Some(0));

    let sync = |path: &str| boolean(&done(perform(&fs, Op::Sync, "cache", &[Value::str(path)])));
    // The data file, then the directory whose rename must survive with it.
    assert!(sync("frontend.dat"));
    assert!(sync(""));
    assert!(!sync("absent.dat"));
}

#[test]
fn the_new_operations_are_confined_like_every_other() {
    let dir = root();
    let fs = rooted(dir.path());
    for op in [Op::Append, Op::Sync] {
        let args: Vec<Value> = match op {
            Op::Append => vec![Value::str("../escape.dat"), Value::bytes(b"x")],
            _ => vec![Value::str("../escape.dat")],
        };
        let refused = perform(&fs, op, "cache", &args).expect_err("it leaves the root");
        assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT, "{}", op.name());
    }
    let refused = perform(
        &fs,
        Op::ReadAt,
        "cache",
        &[Value::str("../escape.dat"), Value::Int(0), Value::Int(4)],
    )
    .expect_err("it leaves the root");
    assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT);

    // And an unbound label reaches no file at all.
    let refused = perform(
        &fs,
        Op::Append,
        "elsewhere",
        &[Value::str("a.dat"), Value::bytes(b"x")],
    )
    .expect_err("`elsewhere` has no root");
    assert_eq!(refused.code, codes::FS_ROOT_UNBOUND);
}

#[test]
fn the_new_operations_are_declared_with_the_arity_they_take() {
    let dir = root();
    let fs = rooted(dir.path());
    let handlers = registrations(&fs);
    for op in [Op::ReadAt, Op::Append, Op::Sync] {
        assert!(Op::ALL.contains(&op), "{}", op.name());
        let (declaration, _) = handlers
            .iter()
            .find(|(d, _)| d.op.as_str() == op.name())
            .unwrap_or_else(|| panic!("`{}` is not registered", op.name()));
        // Every one of them waits on a disk, so every one waits in the pool.
        assert!(declaration.blocking, "{}", op.name());
        assert!(
            declaration.path.starts_with("ply_host::fs::"),
            "{}",
            op.name()
        );
    }
    assert_eq!(Op::ReadAt.name(), "read_at");
    assert_eq!(Op::Append.name(), "append");
    assert_eq!(Op::Sync.name(), "sync");

    // Arity is inference's, so the wrong count means the module was never checked.
    let refused = perform(&fs, Op::ReadAt, "cache", &[Value::str("a.dat")])
        .expect_err("a ranged read takes three");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
}

// --- Copies, trees, temporary directories, modes, links, walks and stamps ---

fn on(fs: &Arc<FsHost>, op: Op, args: &[Value]) -> Value {
    done(perform(fs, op, "cache", args))
}

fn maybe_str(v: &Value) -> Option<String> {
    option(v).map(|inner| match inner {
        Value::Str(s) => s.to_string(),
        other => panic!("not a String: {}", other.type_name()),
    })
}

fn ctor_name(v: &Value) -> String {
    match v {
        Value::Ctor { name, .. } => name.as_str().to_string(),
        other => panic!("not a constructor: {}", other.type_name()),
    }
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

fn access_of(bits: u32) -> Value {
    record(vec![
        ("execute", Value::Bool(bits & 1 != 0)),
        ("read", Value::Bool(bits & 4 != 0)),
        ("write", Value::Bool(bits & 2 != 0)),
    ])
}

fn mode_value(bits: u32) -> Value {
    record(vec![
        ("group", access_of(bits >> 3 & 7)),
        ("other", access_of(bits & 7)),
        ("owner", access_of(bits >> 6 & 7)),
    ])
}

fn mode_bits_of(v: &Value) -> u32 {
    let Value::Record(fields) = v else {
        panic!("not a mode: {}", v.type_name())
    };
    let triple = |who: &str| {
        let Some(Value::Record(access)) = fields.named(who) else {
            panic!("no `{who}`")
        };
        let bit = |name: &str, value: u32| match access.named(name) {
            Some(Value::Bool(true)) => value,
            _ => 0,
        };
        bit("read", 4) | bit("write", 2) | bit("execute", 1)
    };
    triple("owner") << 6 | triple("group") << 3 | triple("other")
}

fn file_mode(path: &std::path::Path) -> u32 {
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

    let copy =
        |from: &str, to: &str| boolean(&on(&fs, Op::Copy, &[Value::str(from), Value::str(to)]));
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

    let remove_tree = |path: &str| boolean(&on(&fs, Op::RemoveTree, &[Value::str(path)]));
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
        maybe_str(&on(
            &fs,
            Op::TempDir,
            &[Value::str(under), Value::str(prefix)],
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
fn canonical_answers_the_real_path_and_nothing_for_a_missing_one() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::os::unix::fs::symlink("a.txt", real.join("l")).unwrap();

    let canonical = |path: &str| maybe_str(&on(&fs, Op::Canonical, &[Value::str(path)]));
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

    assert!(boolean(&on(
        &fs,
        Op::SetMode,
        &[Value::str("run.sh"), mode_value(0o750)]
    )));
    assert_eq!(file_mode(&real.join("run.sh")), 0o750);
    let read = on(&fs, Op::Mode, &[Value::str("run.sh")]);
    assert_eq!(mode_bits_of(option(&read).expect("a mode")), 0o750);
    assert!(option(&on(&fs, Op::Mode, &[Value::str("absent")])).is_none());
    assert!(!boolean(&on(
        &fs,
        Op::SetMode,
        &[Value::str("absent"), mode_value(0o644)]
    )));
}

#[test]
fn a_link_is_made_inside_the_root_and_one_out_of_it_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::fs::create_dir(real.join("d")).unwrap();

    let link = |path: &str, target: &str| {
        perform(
            &fs,
            Op::Symlink,
            "cache",
            &[Value::str(path), Value::str(target)],
        )
    };
    assert!(boolean(&done(link("l", "a.txt"))));
    assert!(boolean(&done(link("d/up", "../a.txt"))));
    assert_eq!(
        maybe_str(&on(&fs, Op::ReadLink, &[Value::str("d/up")])),
        Some("../a.txt".to_string())
    );
    assert_eq!(
        ctor_name(&on(&fs, Op::Kind, &[Value::str("l")])),
        "std.fs.Symlink"
    );
    assert_eq!(
        maybe_str(&on(&fs, Op::ReadLink, &[Value::str("a.txt")])),
        None
    );
    assert!(
        !boolean(&done(link("l", "a.txt"))),
        "a path that is there is not made a link"
    );
    for (path, target) in [
        ("out", "../elsewhere"),
        ("d/out", "../../x"),
        ("abs", "/etc"),
    ] {
        let refused = link(path, target).expect_err("the target leaves the root");
        assert_eq!(
            refused.code,
            codes::FS_PATH_ESCAPES_ROOT,
            "{path} -> {target}"
        );
    }
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

    let walk = |path: &str| -> Option<Vec<(String, String)>> {
        option(&on(&fs, Op::Walk, &[Value::str(path)])).map(|entries| {
            let Value::List(entries) = entries else {
                panic!("not a list")
            };
            entries
                .iter()
                .map(|e| {
                    let Value::Record(fields) = e else {
                        panic!("not a record")
                    };
                    let path = match fields.named("path") {
                        Some(Value::Str(s)) => s.to_string(),
                        _ => panic!("no path"),
                    };
                    (path, ctor_name(fields.named("kind").expect("a kind")))
                })
                .collect()
        })
    };
    let entry = |p: &str, k: &str| (p.to_string(), format!("std.fs.{k}"));
    assert_eq!(
        walk(""),
        Some(vec![
            entry("a", "Dir"),
            entry("a/b.txt", "File"),
            entry("a/z", "Dir"),
            entry("a/z/y.txt", "File"),
            entry("a-c.txt", "File"),
            entry("link", "Symlink"),
        ])
    );
    assert_eq!(
        walk("./a"),
        Some(vec![
            entry("a/b.txt", "File"),
            entry("a/z", "Dir"),
            entry("a/z/y.txt", "File"),
        ])
    );
    assert_eq!(walk("a/b.txt"), None);
    assert_eq!(walk("absent"), None);
}

#[test]
fn a_stamp_sets_the_modification_time_and_one_before_the_epoch_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    std::fs::create_dir(real.join("d")).unwrap();

    let stamp = |path: &str, ms: i64| {
        perform(
            &fs,
            Op::SetModified,
            "cache",
            &[Value::str(path), Value::Int(ms)],
        )
    };
    let modified = |path: &str| maybe_int(&on(&fs, Op::ModifiedMs, &[Value::str(path)]));
    for path in ["a.txt", "d"] {
        assert!(boolean(&done(stamp(path, 1_000_000_000_000))), "{path}");
        assert_eq!(modified(path), Some(1_000_000_000_000), "{path}");
    }
    assert!(!boolean(&done(stamp("absent", 1))));
    let refused = stamp("a.txt", -1).expect_err("before the epoch");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
}

#[test]
fn every_operation_that_names_a_second_path_confines_it_too() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();
    for (op, args) in [
        (Op::Copy, vec![Value::str("a.txt"), Value::str("../b.txt")]),
        (Op::Copy, vec![Value::str("../a.txt"), Value::str("b.txt")]),
        (Op::RemoveTree, vec![Value::str("../x")]),
        (Op::TempDir, vec![Value::str("../"), Value::str("t")]),
        (Op::Walk, vec![Value::str("/")]),
        (Op::Canonical, vec![Value::str("../a.txt")]),
        (Op::Mode, vec![Value::str("../a.txt")]),
        (Op::SetMode, vec![Value::str("../a.txt"), mode_value(0o644)]),
        (Op::Symlink, vec![Value::str("../l"), Value::str("a.txt")]),
        (Op::ReadLink, vec![Value::str("../l")]),
        (Op::SetModified, vec![Value::str("../a.txt"), Value::Int(1)]),
    ] {
        let refused = perform(&fs, op, "cache", &args).expect_err("it leaves the root");
        assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT, "{}", op.name());
    }
    for op in [
        Op::Copy,
        Op::TempDir,
        Op::SetMode,
        Op::Symlink,
        Op::SetModified,
    ] {
        let refused =
            perform(&fs, op, "cache", &[Value::str("a.txt")]).expect_err("it takes two arguments");
        assert_eq!(refused.code, codes::RUNTIME_ERROR, "{}", op.name());
    }
}
