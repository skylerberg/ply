use ply_eval::host::MachineId;
use ply_eval::{
    Diagnostic, EffectAtom, HostAnswer, HostRequest, HostRuntime, Mode, Resource, Span, Symbol,
    Value, codes,
};
use ply_host::fs::*;
use ply_host::pool::Pooled;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
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
    assert_eq!(
        confine(&real, "a.ply", span()).unwrap(),
        Some(real.join("a.ply"))
    );
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

/// A link to where nothing is resolves to nothing, so it is held by where it leads.
#[test]
fn a_write_through_a_link_to_a_missing_path_outside_the_root_is_refused() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let outside = root();
    let made = outside.path().canonicalize().unwrap().join("made");
    std::os::unix::fs::symlink(&made, real.join("link")).unwrap();
    let fs = rooted(&real);

    let refused = perform(
        &fs,
        Op::WriteFile,
        "cache",
        &[Value::str("link"), Value::bytes(b"x")],
    );
    assert!(!made.exists(), "the write landed outside the root");
    assert_eq!(
        refused.expect_err("the link leads out").code,
        codes::FS_PATH_ESCAPES_ROOT
    );
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
        HostAnswer::Pending(pending) => fs.pool().block_on(pending),
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
    // A target is read from the directory the link is made in, wherever a link above it leads.
    std::fs::create_dir_all(real.join("d/e/f")).unwrap();
    std::os::unix::fs::symlink("..", real.join("d/back")).unwrap();
    std::os::unix::fs::symlink("e/f", real.join("d/deep")).unwrap();
    assert!(boolean(&done(link("d/deep/up", "../../../a.txt"))));
    assert_eq!(std::fs::read(real.join("d/e/f/up")).unwrap(), b"x");
    for (path, target) in [
        ("out", "../elsewhere"),
        ("d/out", "../../x"),
        ("abs", "/etc"),
        ("d/back/esc", "../x"),
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

// --- Open files ---

fn opened(fs: &Arc<FsHost>, label: &str, path: &str, how: &str) -> Result<i64, String> {
    let how = Value::ctor(format!("std.fs.{how}"), Vec::new());
    match &done(perform(fs, Op::Open, label, &[Value::str(path), how])) {
        Value::Ctor { name, args } if name.as_str() == "Ok" => match &args[0] {
            Value::Int(descriptor) => Ok(*descriptor),
            other => panic!("not a descriptor: {}", other.type_name()),
        },
        Value::Ctor { name, args } if name.as_str() == "Err" => Err(ctor_name(&args[0])),
        other => panic!("not a `Result`: {}", other.type_name()),
    }
}

fn chunk(fs: &Arc<FsHost>, label: &str, descriptor: i64, max: i64) -> Option<Vec<u8>> {
    maybe_bytes(&done(perform(
        fs,
        Op::ReadChunk,
        label,
        &[Value::Int(descriptor), Value::Int(max)],
    )))
}

fn closed(fs: &Arc<FsHost>, label: &str, descriptor: i64) -> bool {
    boolean(&done(perform(
        fs,
        Op::Close,
        label,
        &[Value::Int(descriptor)],
    )))
}

#[test]
fn a_descriptor_is_open_under_the_root_that_opened_it_until_it_is_closed() {
    let (here, there) = (root(), root());
    std::fs::write(here.path().join("log"), b"abcdef").unwrap();
    let mut roots = Roots::new();
    roots.bind("cache", here.path(), span()).unwrap();
    roots.bind("other", there.path(), span()).unwrap();
    let fs = Arc::new(FsHost::new(roots));

    let first = opened(&fs, "cache", "log", "ToRead").expect("the file opens");
    assert_eq!(chunk(&fs, "cache", first, 4), Some(b"abcd".to_vec()));
    // Another root's label reaches nothing this root opened: not to read it, not to close it.
    assert_eq!(chunk(&fs, "other", first, 4), None);
    assert!(!closed(&fs, "other", first));
    assert_eq!(chunk(&fs, "cache", first, 4), Some(b"ef".to_vec()));
    assert_eq!(chunk(&fs, "cache", first, 4), Some(Vec::new()));

    assert!(closed(&fs, "cache", first));
    assert!(!closed(&fs, "cache", first));
    assert_eq!(chunk(&fs, "cache", first, 4), None);
    // A descriptor names one open, so the next open answers another.
    let second = opened(&fs, "cache", "log", "ToRead").expect("the file opens again");
    assert_ne!(second, first);
}

#[test]
fn an_open_says_why_a_file_did_not_open_and_is_confined_like_every_other() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::create_dir(dir.path().join("d")).unwrap();
    assert_eq!(
        opened(&fs, "cache", "absent", "ToRead"),
        Err("std.fs.NotFound".into())
    );
    for how in ["ToRead", "ToWrite", "ToAppend"] {
        assert_eq!(
            opened(&fs, "cache", "d", how),
            Err("std.fs.NotAFile".into())
        );
    }
    assert_eq!(
        opened(&fs, "cache", "no/f", "ToWrite"),
        Err("std.fs.NotFound".into())
    );

    let how = Value::ctor("std.fs.ToRead", Vec::new());
    let refused = perform(
        &fs,
        Op::Open,
        "cache",
        &[Value::str("../escape"), how.clone()],
    )
    .expect_err("it leaves the root");
    assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT);
    let refused = perform(&fs, Op::Open, "elsewhere", &[Value::str("a"), how])
        .expect_err("`elsewhere` has no root");
    assert_eq!(refused.code, codes::FS_ROOT_UNBOUND);
}

#[test]
fn a_chunk_costs_at_most_what_it_asks_for_and_the_bound_is_on_one_read() {
    let dir = root();
    let fs = rooted(dir.path());
    std::fs::write(dir.path().join("log"), vec![7u8; 4096]).unwrap();
    let file = opened(&fs, "cache", "log", "ToRead").expect("the file opens");
    assert_eq!(chunk(&fs, "cache", file, 0), Some(Vec::new()));
    assert_eq!(chunk(&fs, "cache", file, 100).map(|b| b.len()), Some(100));

    let over = perform(
        &fs,
        Op::ReadChunk,
        "cache",
        &[Value::Int(file), Value::Int(MAX_READ_BYTES as i64 + 1)],
    )
    .expect_err("more than one read answers");
    assert_eq!(over.code, codes::FS_FILE_TOO_LARGE);
    let negative = perform(
        &fs,
        Op::ReadChunk,
        "cache",
        &[Value::Int(file), Value::Int(-1)],
    )
    .expect_err("a negative length");
    assert_eq!(negative.code, codes::RUNTIME_ERROR);
    // Neither refusal moved the descriptor.
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

    let made = opened(&fs, "cache", "d/new", "ToCreate").expect("nothing is there");
    assert!(boolean(&on(
        &fs,
        Op::WriteChunk,
        &[Value::Int(made), Value::bytes(b"mine")]
    )));
    assert!(closed(&fs, "cache", made));
    assert_eq!(std::fs::read(real.join("d/new")).unwrap(), b"mine");
    assert_eq!(file_mode(&real.join("d/new")), 0o600);

    for there in ["d/new", "d/a.txt", "d", "d/l"] {
        assert_eq!(
            opened(&fs, "cache", there, "ToCreate"),
            Err("std.fs.Exists".into()),
            "{there}"
        );
    }
    // The link was refused as itself: nothing was made where it points.
    assert!(!real.join("d/nowhere").exists());
    assert_eq!(
        opened(&fs, "cache", "no/new", "ToCreate"),
        Err("std.fs.NotFound".into())
    );
    assert_eq!(
        opened(&fs, "cache", "d/a.txt/new", "ToCreate"),
        Err("std.fs.NotADirectory".into())
    );
}

fn maybe_sealed(v: &Value) -> Option<Vec<u8>> {
    option(v).map(|inner| match inner {
        Value::Secret(held) => held.bytes().to_vec(),
        other => panic!("not a Secret: {}", other.type_name()),
    })
}

fn write_secret(
    fs: &Arc<FsHost>,
    path: &str,
    secret: &Value,
    encoding: &str,
) -> Result<Value, Diagnostic> {
    perform(
        fs,
        Op::WriteSecret,
        "cache",
        &[
            Value::str(path),
            Value::bytes(b"key "),
            secret.clone(),
            Value::str(encoding),
            Value::bytes(b"\n"),
        ],
    )
}

fn read_secret(
    fs: &Arc<FsHost>,
    path: &str,
    before: &[u8],
    encoding: &str,
    after: &[u8],
) -> Option<Vec<u8>> {
    maybe_sealed(&done(perform(
        fs,
        Op::ReadSecret,
        "cache",
        &[
            Value::str(path),
            Value::bytes(before),
            Value::str(encoding),
            Value::bytes(after),
        ],
    )))
}

#[test]
fn a_secret_is_written_in_its_frame_as_its_owners_alone_whatever_was_there() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("old.key"), b"a longer plain file than the key").unwrap();
    std::fs::set_permissions(real.join("old.key"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let key = Value::secret_bytes(b"\x01\x23\xab");

    for (path, encoding, line) in [
        ("new.key", "hex", &b"key 0123ab\n"[..]),
        ("old.key", "base64", b"key ASOr\n"),
        ("raw.key", "raw", b"key \x01\x23\xab\n"),
    ] {
        assert!(
            boolean(&done(write_secret(&fs, path, &key, encoding))),
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
    assert!(!boolean(&done(write_secret(
        &fs,
        "no/new.key",
        &key,
        "hex"
    ))));
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

#[test]
fn a_secret_write_refuses_a_plain_body_an_unknown_encoding_and_a_path_out_of_the_root() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    let key = Value::secret_bytes(b"hunter2");

    let plain = write_secret(&fs, "plain.key", &Value::bytes(b"hunter2"), "hex")
        .expect_err("a plain body is not a secret");
    assert!(!plain.message.contains("hunter2"), "{}", plain.message);
    let unknown = write_secret(&fs, "a.key", &key, "rot13").expect_err("no such encoding");
    assert_eq!(unknown.code, codes::RUNTIME_ERROR);
    assert!(
        unknown.message.contains("`fs.write_secret`"),
        "{}",
        unknown.message
    );
    assert!(!unknown.message.contains("hunter2"), "{}", unknown.message);
    let unread = perform(
        &fs,
        Op::ReadSecret,
        "cache",
        &[
            Value::str("a.key"),
            Value::bytes(b""),
            Value::str("base32-typed"),
            Value::bytes(b""),
        ],
    )
    .expect_err("a typed text is read by `secret_decode` alone");
    assert_eq!(unread.code, codes::RUNTIME_ERROR);
    let escaped = write_secret(&fs, "../out.key", &key, "hex").expect_err("it leaves the root");
    assert_eq!(escaped.code, codes::FS_PATH_ESCAPES_ROOT);
    assert!(std::fs::read_dir(&real).unwrap().next().is_none());
}

/// A `std.fs.Stat`, as a test reads one.
#[derive(Debug, PartialEq)]
struct Looked {
    kind: String,
    size: i64,
    modified: i64,
    mode: u32,
    links: i64,
    device: i64,
    id: i64,
}

fn looked(v: &Value) -> Looked {
    let Value::Record(fields) = v else {
        panic!("not a stat: {}", v.type_name())
    };
    let int = |name: &str| match fields.named(name) {
        Some(Value::Int(n)) => *n,
        _ => panic!("no `{name}`"),
    };
    let modified = match fields.named("modified") {
        Some(Value::Ctor { name, args }) if name.as_str() == "Instant" => match args.as_slice() {
            [Value::Int(nanos)] => *nanos,
            _ => panic!("an `Instant` holds one `Int`"),
        },
        _ => panic!("no `modified`"),
    };
    Looked {
        kind: ctor_name(fields.named("kind").expect("a kind")),
        size: int("size"),
        modified,
        mode: mode_bits_of(fields.named("mode").expect("a mode")),
        links: int("links"),
        device: int("device"),
        id: int("id"),
    }
}

fn stat(fs: &Arc<FsHost>, path: &str) -> Option<Looked> {
    option(&on(fs, Op::Stat, &[Value::str(path)])).map(looked)
}

fn linked(fs: &Arc<FsHost>, path: &str, to: &str) -> Result<(), String> {
    match &on(fs, Op::Link, &[Value::str(path), Value::str(to)]) {
        Value::Ctor { name, args } if name.as_str() == "Ok" => {
            assert!(matches!(args.as_slice(), [Value::Unit]));
            Ok(())
        }
        Value::Ctor { name, args } if name.as_str() == "Err" => Err(ctor_name(&args[0])),
        other => panic!("not a `Result`: {}", other.type_name()),
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
        Looked {
            kind: "std.fs.File".into(),
            size: 3,
            modified: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            mode: 0o640,
            links: 1,
            device: meta.dev() as i64,
            id: meta.ino() as i64,
        }
    );

    let folder = stat(&fs, "d").expect("a directory is there");
    assert_eq!(
        (folder.kind.as_str(), folder.size, folder.links),
        ("std.fs.Dir", 0, 1)
    );
    assert_eq!(folder.mode, file_mode(&real.join("d")));

    // The link is read, not followed: it is no file, and it is not the file it points at.
    let link = stat(&fs, "d/l").expect("a link is there");
    assert_eq!(
        (link.kind.as_str(), link.size, link.mode, link.links),
        ("std.fs.Symlink", 0, 0o777, 1)
    );
    assert_ne!(link.id, file.id);

    assert_eq!(stat(&fs, "d/absent"), None);
    let refused =
        perform(&fs, Op::Stat, "cache", &[Value::str("../d")]).expect_err("it leaves the root");
    assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT);
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

    let scan = |path: &str, deep: bool| -> Option<Vec<(String, String, i64)>> {
        option(&on(&fs, Op::Scan, &[Value::str(path), Value::Bool(deep)])).map(|entries| {
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
                    let found = looked(fields.named("stat").expect("a stat"));
                    (path, found.kind, found.size)
                })
                .collect()
        })
    };
    let entry = |p: &str, k: &str, size: i64| (p.to_string(), format!("std.fs.{k}"), size);
    assert_eq!(
        scan("", true),
        Some(vec![
            entry("a", "Dir", 0),
            entry("a/b.txt", "File", 1),
            entry("a/z", "Dir", 0),
            entry("a/z/y.txt", "File", 2),
            entry("a-c.txt", "File", 0),
            entry("link", "Symlink", 0),
        ])
    );
    assert_eq!(
        scan("", false),
        Some(vec![
            entry("a", "Dir", 0),
            entry("a-c.txt", "File", 0),
            entry("link", "Symlink", 0),
        ])
    );
    assert_eq!(
        scan("./a/", false),
        Some(vec![entry("a/b.txt", "File", 1), entry("a/z", "Dir", 0)])
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
        assert!(
            boolean(&on(&fs, Op::Copy, &[Value::str(from), Value::str(to)])),
            "{from} -> {to}"
        );
        assert_eq!(std::fs::read(real.join("d/a.txt")).unwrap(), b"both");
    }

    let refused = |path: &str, to: &str, why: &str| {
        assert_eq!(
            linked(&fs, path, to),
            Err(format!("std.fs.{why}")),
            "{path} -> {to}"
        );
    };
    refused("n", "absent", "NotFound");
    refused("n", "d", "NotAFile");
    refused("n", "l", "NotAFile");
    refused("l", "d/a.txt", "Exists");
    refused("twin.txt", "d/a.txt", "Exists");
    refused("no/n", "d/a.txt", "NotFound");
    refused("d/a.txt/n", "d/a.txt", "NotADirectory");
    assert!(!real.join("n").exists());

    for (path, to) in [("../n", "d/a.txt"), ("n", "/d/a.txt")] {
        let refused = perform(&fs, Op::Link, "cache", &[Value::str(path), Value::str(to)])
            .expect_err("it leaves the root");
        assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT, "{path} -> {to}");
    }
}

#[test]
fn the_room_of_a_file_system_is_answered_for_a_path_that_names_something() {
    let dir = root();
    let real = dir.path().canonicalize().unwrap();
    let fs = rooted(&real);
    std::fs::write(real.join("a.txt"), b"x").unwrap();

    let room = |path: &str| -> Option<[i64; 5]> {
        option(&on(&fs, Op::Space, &[Value::str(path)])).map(|space| {
            let Value::Record(fields) = space else {
                panic!("not a record")
            };
            ["total", "free", "available", "inodes", "inodes_free"].map(|name| {
                match fields.named(name) {
                    Some(Value::Int(n)) => *n,
                    _ => panic!("no `{name}`"),
                }
            })
        })
    };
    let [total, free, available, inodes, inodes_free] = room("").expect("the root is there");
    assert!(total > 0 && free <= total && available <= free);
    assert!(inodes_free <= inodes);
    // One file system holds the root and the file in it.
    assert_eq!(room("a.txt").map(|r| r[0]), Some(total));
    assert_eq!(room("absent"), None);
}

#[test]
fn a_look_a_scan_the_room_and_a_second_name_are_registered_and_wait_in_the_pool() {
    let dir = root();
    let fs = rooted(dir.path());
    let handlers = registrations(&fs);
    for (op, name, arity) in [
        (Op::Stat, "stat", 1),
        (Op::Scan, "scan", 2),
        (Op::Space, "space", 1),
        (Op::Link, "link", 2),
    ] {
        assert_eq!((op.name(), op.arity()), (name, arity));
        let (declaration, _) = handlers
            .iter()
            .find(|(d, _)| d.op.as_str() == name)
            .unwrap_or_else(|| panic!("`{name}` is not registered"));
        assert!(declaration.blocking, "{name}");
        assert_eq!(declaration.path, format!("ply_host::fs::{name}"));
        let refused = perform(&fs, op, "elsewhere", &vec![Value::str("a"); arity])
            .expect_err("`elsewhere` has no root");
        assert_eq!(refused.code, codes::FS_ROOT_UNBOUND, "{name}");
    }
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

/// Every way `op` is given `path`, each with whether it then acts on a link that is the path's
/// last name rather than on what the link leads to. No arm is a wildcard, so an operation added to
/// the family does not compile until it says how it takes a path.
fn given(op: Op, path: &str) -> Vec<(Vec<Value>, bool)> {
    let p = || Value::str(path);
    let how = |name: &str| Value::ctor(format!("std.fs.{name}"), Vec::new());
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
        | Op::Space => vec![(vec![p()], false)],
        Op::Kind
        | Op::Exists
        | Op::Remove
        | Op::Lock
        | Op::Unlock
        | Op::RemoveTree
        | Op::ReadLink
        | Op::Stat => vec![(vec![p()], true)],
        Op::ReadAt => vec![(vec![p(), Value::Int(0), Value::Int(4)], false)],
        Op::WriteFile | Op::Append => vec![(vec![p(), Value::bytes(b"x")], false)],
        Op::WriteSecret => vec![(
            vec![
                p(),
                Value::bytes(b"key "),
                Value::secret_bytes(b"x"),
                Value::str("hex"),
                Value::bytes(b"\n"),
            ],
            false,
        )],
        Op::ReadSecret => vec![(
            vec![p(), Value::bytes(b""), Value::str("raw"), Value::bytes(b"")],
            false,
        )],
        Op::TempDir => vec![(vec![p(), Value::str("t-")], false)],
        Op::SetMode => vec![(vec![p(), mode_value(0o600)], false)],
        Op::SetModified => vec![(vec![p(), Value::Int(1000)], false)],
        Op::Scan => vec![(vec![p(), Value::Bool(true)], false)],
        Op::Symlink => vec![(vec![p(), Value::str("plain.txt")], true)],
        Op::Open => vec![
            (vec![p(), how("ToRead")], false),
            (vec![p(), how("ToWrite")], false),
            (vec![p(), how("ToAppend")], false),
            (vec![p(), how("ToCreate")], true),
        ],
        Op::Rename => vec![
            (vec![p(), Value::str("moved")], true),
            (vec![Value::str("plain.txt"), p()], true),
        ],
        Op::Copy => vec![
            (vec![p(), Value::str("copied")], false),
            (vec![Value::str("plain.txt"), p()], false),
        ],
        Op::Link => vec![
            (vec![p(), Value::str("plain.txt")], true),
            (vec![Value::str("named"), p()], true),
        ],
        // An open file is named by the descriptor `open` answered, and takes no path.
        Op::ReadChunk | Op::WriteChunk | Op::Close => Vec::new(),
    }
}

#[test]
fn no_operation_reaches_through_a_link_out_of_the_root() {
    let registered: Vec<String> = {
        let dir = root();
        registrations(&rooted(dir.path()))
            .iter()
            .map(|(declaration, _)| declaration.op.as_str().to_string())
            .collect()
    };
    for name in &registered {
        let op = *Op::ALL
            .iter()
            .find(|op| op.name() == name)
            .unwrap_or_else(|| panic!("`{name}` is registered and is no `Op`"));
        for way in Way::ALL {
            for (args, itself) in given(op, way.path()) {
                let (inside, outside) = linked_out();
                let fs = rooted(inside.path());
                let before = held(outside.path());
                let answer = perform(&fs, op, "cache", &args);
                let said = format!("`{name}` given `{}` ({way:?})", way.path());
                if itself && way.names_the_link() {
                    assert!(answer.is_ok(), "{said} acts on the link itself");
                } else {
                    let refused = answer
                        .err()
                        .unwrap_or_else(|| panic!("{said} was not refused"));
                    assert_eq!(refused.code, codes::FS_PATH_ESCAPES_ROOT, "{said}");
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
        let path = || Value::str(name);

        assert_eq!(ctor_name(&on(&fs, Op::Kind, &[path()])), "std.fs.Symlink");
        assert!(boolean(&on(&fs, Op::Exists, &[path()])));
        assert_eq!(
            stat(&fs, name).map(|found| found.kind),
            Some("std.fs.Symlink".to_string())
        );
        assert_eq!(
            maybe_str(&on(&fs, Op::ReadLink, &[path()])),
            target.to_str().map(str::to_string)
        );
        assert_eq!(
            opened(&fs, "cache", name, "ToCreate"),
            Err("std.fs.Exists".into())
        );
        assert_eq!(linked(&fs, "named", name), Err("std.fs.NotAFile".into()));
        assert_eq!(linked(&fs, name, "plain.txt"), Err("std.fs.Exists".into()));
        assert!(!boolean(&on(
            &fs,
            Op::Symlink,
            &[path(), Value::str("plain.txt")]
        )));

        assert!(boolean(&on(
            &fs,
            Op::Rename,
            &[path(), Value::str("moved")]
        )));
        assert_eq!(std::fs::read_link(here.join("moved")).unwrap(), target);
        std::fs::write(here.join("new.txt"), b"new").unwrap();
        assert!(boolean(&on(
            &fs,
            Op::Rename,
            &[Value::str("new.txt"), Value::str("moved")]
        )));
        assert_eq!(std::fs::read(here.join("moved")).unwrap(), b"new");

        std::os::unix::fs::symlink(&target, here.join("again")).unwrap();
        let remove = if name == "out" {
            Op::RemoveTree
        } else {
            Op::Remove
        };
        assert!(boolean(&on(&fs, remove, &[Value::str("again")])));
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
    let read = |path: &str| maybe_bytes(&on(&fs, Op::ReadFile, &[Value::str(path)]));
    let write = |path: &str| {
        boolean(&on(
            &fs,
            Op::WriteFile,
            &[Value::str(path), Value::bytes(b"w")],
        ))
    };

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
            opened(&fs, "cache", nowhere, "ToWrite"),
            Err("std.fs.NotFound".into()),
            "{nowhere}"
        );
        assert_eq!(
            ctor_name(&on(&fs, Op::Resolved, &[Value::str(nowhere)])),
            "std.fs.Missing",
            "{nowhere}"
        );
    }
    assert_eq!(std::fs::read(real.join("d/a.txt")).unwrap(), b"a");
    assert_eq!(
        ctor_name(&on(&fs, Op::Kind, &[Value::str("round")])),
        "std.fs.Symlink"
    );

    // A link's target that steps back out of a name that is not there names nothing, as the
    // system answers it.
    std::os::unix::fs::symlink("absent/../d/a.txt", real.join("back")).unwrap();
    assert_eq!(read("back"), None);
    assert!(!write("back"));
}
