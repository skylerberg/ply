use super::*;
use ply_span::{Severity, Span, codes as span_codes};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique directory under the system temp dir, removed on drop. Avoids a
/// dev-dependency for something this small.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ply-store-{}-{tag}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempRoot(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn cache_file(&self) -> PathBuf {
        self.0.join(CACHE_DIR_NAME).join("results.json")
    }

    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hash(n: u8) -> DefHash {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    bytes[31] = n.wrapping_mul(7);
    DefHash(bytes)
}

fn failure() -> Outcome {
    Outcome::Fail {
        message: "assertion failed: expected 0, found -5".to_string(),
        diagnostic: Some(
            Diagnostic::error(span_codes::ASSERTION_FAILED, "assertion failed")
                .primary(
                    Span::new(ply_span::SourceId(3), 88, 97),
                    "expected 0, found -5",
                )
                .note("suspects: apply_debit"),
        ),
    }
}

fn temp_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "tmp"))
        .collect()
}

#[test]
fn round_trips_pass_and_failure_through_disk() {
    let root = TempRoot::new("round-trip");

    let mut store = root.open();
    assert!(store.is_empty());
    store.put(hash(1), Outcome::Pass);
    store.put(hash(2), failure());
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.len(), 2);
    assert!(reopened.warnings().is_empty());
    assert!(reopened.get(hash(1)).unwrap().is_pass());
    assert!(reopened.contains(hash(2)));
    assert!(reopened.get(hash(3)).is_none());

    let Some(Outcome::Fail {
        message,
        diagnostic,
    }) = reopened.get(hash(2))
    else {
        panic!("hash(2) should have come back as a failure");
    };
    assert_eq!(message, "assertion failed: expected 0, found -5");
    let d = diagnostic.expect("the diagnostic must survive the round trip");
    assert_eq!(d.code, span_codes::ASSERTION_FAILED);
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.notes, vec!["suspects: apply_debit".to_string()]);
    assert_eq!(d.labels[0].span, Span::new(ply_span::SourceId(3), 88, 97));
    assert!(d.labels[0].primary);
}

#[test]
fn a_later_put_overwrites_an_earlier_one() {
    let root = TempRoot::new("overwrite");
    let mut store = root.open();
    store.put(hash(1), failure());
    store.put(hash(1), Outcome::Pass);
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.len(), 1);
    assert!(reopened.get(hash(1)).unwrap().is_pass());
}

#[test]
fn the_file_is_hand_readable_and_keyed_by_hex_hash() {
    let root = TempRoot::new("readable");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.flush().unwrap();

    let text = fs::read_to_string(root.cache_file()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["runtime_version"], RUNTIME_VERSION);
    assert_eq!(json["format"], 1);
    assert_eq!(json["results"][hash(1).to_hex()]["outcome"], "pass");
    assert!(
        text.contains('\n'),
        "the cache is pretty-printed on purpose"
    );
}

#[test]
fn an_abandoned_temp_file_does_not_disturb_the_cache() {
    let root = TempRoot::new("interrupt");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put(hash(2), Outcome::Pass);
    store.flush().unwrap();
    assert!(
        temp_files(store.dir()).is_empty(),
        "a completed flush leaves no temp file"
    );

    // What an interrupted `ply test` leaves behind: a half-written temp file
    // that was never renamed.
    let abandoned = store.dir().join("results.999999.0.0.tmp");
    fs::write(&abandoned, "{\"format\": 1, \"runtime_ver").unwrap();

    let mut reopened = root.open();
    assert_eq!(reopened.len(), 2, "the previous cache is still whole");
    assert!(reopened.warnings().is_empty());
    assert!(
        abandoned.exists(),
        "a temp file this young may belong to a live writer"
    );

    reopened.put(hash(3), Outcome::Pass);
    reopened.flush().unwrap();
    assert_eq!(root.open().len(), 3);
}

#[test]
fn stale_temp_files_are_swept_on_open() {
    let root = TempRoot::new("sweep");
    let store = root.open();
    let stale = store.dir().join("results.1.0.0.tmp");
    let unrelated = store.dir().join("notes.txt");
    fs::write(&stale, "garbage").unwrap();
    fs::write(&unrelated, "keep me").unwrap();

    disk::sweep_temps(store.dir(), None);
    assert!(!stale.exists());
    assert!(unrelated.exists(), "only this crate's temp files are swept");
}

#[test]
fn a_truncated_cache_degrades_to_empty_and_repairs_itself() {
    let root = TempRoot::new("truncated");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put(hash(2), failure());
    store.flush().unwrap();

    let text = fs::read_to_string(root.cache_file()).unwrap();
    fs::write(root.cache_file(), &text[..text.len() / 2]).unwrap();

    let mut damaged = root.open();
    assert!(damaged.is_empty());
    assert_eq!(damaged.warnings().len(), 1);
    assert_eq!(damaged.warnings()[0].code, codes::CACHE_CORRUPT);
    assert_eq!(damaged.warnings()[0].severity, Severity::Warning);
    assert!(
        damaged.get(hash(1)).is_none(),
        "a damaged cache must not answer"
    );

    damaged.put(hash(9), Outcome::Pass);
    damaged.flush().unwrap();

    let repaired = root.open();
    assert!(repaired.warnings().is_empty());
    assert_eq!(repaired.len(), 1);
    assert!(repaired.contains(hash(9)));
}

#[test]
fn every_shape_of_unreadable_file_degrades_rather_than_crashes() {
    for (tag, contents) in [
        ("empty", ""),
        ("not-json", "\u{0}\u{1}garbage\u{ff}"),
        ("wrong-root-type", "[]"),
        (
            "missing-results",
            r#"{"format":1,"runtime_version":"0.1.0"}"#,
        ),
        (
            "bad-hash-key",
            r#"{"format":1,"runtime_version":"0.1.0","results":{"zz":"pass"}}"#,
        ),
        (
            "bad-outcome",
            r#"{"format":1,"runtime_version":"0.1.0","results":{"aa":{"outcome":"maybe"}}}"#,
        ),
        (
            "future-format",
            r#"{"format":99,"runtime_version":"0.1.0","results":{}}"#,
        ),
    ] {
        let root = TempRoot::new(tag);
        let dir = root.path().join(CACHE_DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("results.json"), contents).unwrap();

        let store = root.open();
        assert!(
            store.is_empty(),
            "{tag} should have degraded to an empty cache"
        );
        assert_eq!(store.warnings().len(), 1, "{tag} must warn");
        assert!(
            store.warnings()[0].message.contains("results.json"),
            "{tag} must name the offending file"
        );
        assert!(
            !store.warnings()[0].notes.is_empty(),
            "{tag} must say what happens next"
        );
    }
}

#[test]
fn a_different_runtime_version_invalidates_everything() {
    let root = TempRoot::new("version");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.flush().unwrap();

    let text = fs::read_to_string(root.cache_file()).unwrap();
    let stale = text.replace(
        &format!("\"{RUNTIME_VERSION}\""),
        "\"0.0.1-some-older-runtime\"",
    );
    assert_ne!(stale, text, "the version must actually appear in the file");
    fs::write(root.cache_file(), stale).unwrap();

    let mut store = root.open();
    assert!(store.is_empty());
    assert!(store.get(hash(1)).is_none());
    assert_eq!(store.warnings().len(), 1);
    assert_eq!(store.warnings()[0].code, codes::CACHE_VERSION_CHANGED);
    assert!(
        store.warnings()[0]
            .message
            .contains("0.0.1-some-older-runtime")
    );
    assert!(store.warnings()[0].message.contains(RUNTIME_VERSION));

    // The old entries must not come back through the merge in `flush`.
    store.put(hash(2), Outcome::Pass);
    store.flush().unwrap();
    let reopened = root.open();
    assert_eq!(reopened.len(), 1);
    assert!(reopened.contains(hash(2)));
}

#[test]
fn clear_empties_memory_and_disk() {
    let root = TempRoot::new("clear");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put(hash(2), Outcome::Pass);
    store.flush().unwrap();
    let leftover = store.dir().join("results.1.0.0.tmp");
    fs::write(&leftover, "half written").unwrap();

    store.clear().unwrap();
    assert_eq!(store.len(), 0);
    assert!(store.get(hash(1)).is_none());
    assert!(
        !root.cache_file().exists(),
        "the cache file is gone from disk"
    );
    assert!(!leftover.exists(), "clear also drops abandoned temp files");

    let reopened = root.open();
    assert_eq!(reopened.len(), 0);
    assert!(reopened.warnings().is_empty());

    store.put(hash(3), Outcome::Pass);
    store.flush().unwrap();
    assert_eq!(root.open().len(), 1, "a cleared store must still be usable");
}

#[test]
fn clear_on_a_cache_that_was_never_written_is_not_an_error() {
    let root = TempRoot::new("clear-empty");
    let mut store = root.open();
    store.clear().unwrap();
    store.clear().unwrap();
    assert_eq!(store.len(), 0);
}

#[test]
fn flush_without_changes_writes_nothing() {
    let root = TempRoot::new("no-op-flush");
    let mut store = root.open();
    store.flush().unwrap();
    assert!(!root.cache_file().exists());
    assert!(temp_files(store.dir()).is_empty());
}

#[test]
fn concurrent_stores_do_not_discard_each_others_results() {
    let root = TempRoot::new("merge");
    let mut first = root.open();
    let mut second = root.open();

    first.put(hash(1), Outcome::Pass);
    first.flush().unwrap();

    second.put(hash(2), Outcome::Pass);
    second.flush().unwrap();

    assert_eq!(second.len(), 2, "flush adopts what it merged");
    let reopened = root.open();
    assert_eq!(reopened.len(), 2);
    assert!(reopened.contains(hash(1)));
    assert!(reopened.contains(hash(2)));
}

#[test]
fn a_reader_never_observes_a_partial_file_while_writers_run() {
    let root = TempRoot::new("torn");
    let mut seed = root.open();
    seed.put(hash(1), Outcome::Pass);
    seed.flush().unwrap();

    const WRITERS: u8 = 3;
    const WRITES: u8 = 30;
    let root = &root;
    let finished = AtomicU64::new(0);
    let finished = &finished;

    std::thread::scope(|scope| {
        for w in 0..WRITERS {
            scope.spawn(move || {
                for i in 0..WRITES {
                    let mut store = root.open();
                    store.put(hash(10 + w * WRITES + i), failure());
                    store.flush().unwrap();
                }
                finished.fetch_add(1, Ordering::Release);
            });
        }
        scope.spawn(move || {
            while finished.load(Ordering::Acquire) < WRITERS as u64 {
                let store = root.open();
                assert!(
                    store.warnings().is_empty(),
                    "a reader saw a torn cache: {:?}",
                    store.warnings()
                );
                assert!(store.contains(hash(1)), "an entry vanished mid-write");
            }
        });
    });

    let final_store = root.open();
    assert!(final_store.warnings().is_empty());
    assert_eq!(final_store.len(), 1 + (WRITERS * WRITES) as usize);
}

#[test]
fn a_lock_excludes_a_second_holder_and_is_released_on_drop() {
    let root = TempRoot::new("lock");
    let store = root.open();
    let dir = store.dir();

    let first = disk::Lock::acquire(dir);
    assert!(first.held);

    let blocked = disk::Lock::acquire_within(dir, std::time::Duration::from_millis(20));
    assert!(!blocked.held, "a second holder must not get the lock");
    drop(blocked);

    drop(first);
    let after = disk::Lock::acquire(dir);
    assert!(after.held, "dropping the holder releases the lock");
}

#[test]
fn a_lock_left_by_a_dead_process_never_blocks_a_flush() {
    let root = TempRoot::new("lock-abandoned");
    let mut store = root.open();
    fs::write(store.dir().join("lock"), "").unwrap();

    let started = std::time::Instant::now();
    store.put(hash(1), Outcome::Pass);
    store.flush().unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(root.open().len(), 1);
}

#[test]
fn staleness_is_measured_from_mtime() {
    let root = TempRoot::new("mtime");
    let file = root.path().join("marker");
    fs::write(&file, "x").unwrap();
    assert!(!disk::is_older_than(
        &file,
        std::time::Duration::from_secs(30)
    ));

    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(disk::is_older_than(
        &file,
        std::time::Duration::from_millis(1)
    ));
    assert!(
        !disk::is_older_than(&root.path().join("absent"), std::time::Duration::ZERO),
        "a file that cannot be stat'd is never treated as stale"
    );
}

#[test]
fn outcome_json_is_tagged_and_stable() {
    let pass = serde_json::to_value(Outcome::Pass).unwrap();
    assert_eq!(pass, serde_json::json!({"outcome": "pass"}));

    let fail = serde_json::to_value(Outcome::Fail {
        message: "boom".to_string(),
        diagnostic: None,
    })
    .unwrap();
    assert_eq!(
        fail,
        serde_json::json!({"outcome": "fail", "message": "boom"})
    );

    let back: Outcome = serde_json::from_value(fail).unwrap();
    assert!(!back.is_pass());
}

// ---------------------------------------------------------------------------
// Observed definitions
// ---------------------------------------------------------------------------

#[test]
fn an_observed_definition_survives_disk_without_becoming_a_result() {
    let root = TempRoot::new("observed");
    let mut store = root.open();
    assert!(!store.knows_definition(hash(1)));

    assert_eq!(store.observe_definitions([hash(1), hash(2)]), 2);
    store.put(hash(3), Outcome::Pass);
    store.flush().unwrap();

    let reopened = root.open();
    assert!(reopened.knows_definition(hash(1)));
    assert!(reopened.knows_definition(hash(2)));
    assert!(!reopened.knows_definition(hash(3)));
    assert_eq!(reopened.definitions_len(), 2);

    // The two records answer different questions and must never leak into each
    // other: a definition is not a test that passed.
    assert!(reopened.get(hash(1)).is_none());
    assert!(!reopened.contains(hash(1)));
    assert_eq!(reopened.len(), 1);

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.cache_file()).unwrap()).unwrap();
    assert_eq!(
        json["definitions"],
        serde_json::json!([hash(1).to_hex(), hash(2).to_hex()])
    );
}

#[test]
fn observing_nothing_new_writes_nothing() {
    let root = TempRoot::new("observe-idempotent");
    let mut store = root.open();
    assert_eq!(store.observe_definitions([hash(1)]), 1);
    store.flush().unwrap();

    let mut reopened = root.open();
    assert_eq!(reopened.observe_definitions([hash(1)]), 0);
    fs::remove_file(root.cache_file()).unwrap();
    reopened.flush().unwrap();
    assert!(
        !root.cache_file().exists(),
        "a flush with nothing to record must not rewrite the cache"
    );
}

#[test]
fn a_cache_written_before_definitions_existed_still_loads() {
    let root = TempRoot::new("legacy");
    let dir = root.path().join(CACHE_DIR_NAME);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("results.json"),
        format!(
            r#"{{"format":1,"runtime_version":"{RUNTIME_VERSION}","results":{{"{}":{{"outcome":"pass"}}}}}}"#,
            hash(1).to_hex()
        ),
    )
    .unwrap();

    let store = root.open();
    assert!(store.warnings().is_empty());
    assert!(store.contains(hash(1)));
    assert_eq!(store.definitions_len(), 0);
}

#[test]
fn clear_forgets_the_definitions_as_well_as_the_results() {
    let root = TempRoot::new("clear-definitions");
    let mut store = root.open();
    store.observe_definitions([hash(1)]);
    store.put(hash(2), Outcome::Pass);
    store.flush().unwrap();

    store.clear().unwrap();
    assert!(!store.knows_definition(hash(1)));
    assert_eq!(store.definitions_len(), 0);
    assert!(!root.open().knows_definition(hash(1)));
}

#[test]
fn a_different_runtime_version_forgets_the_definitions_too() {
    let root = TempRoot::new("version-definitions");
    let mut store = root.open();
    store.observe_definitions([hash(1)]);
    store.flush().unwrap();

    let text = fs::read_to_string(root.cache_file()).unwrap();
    fs::write(
        root.cache_file(),
        text.replace(&format!("\"{RUNTIME_VERSION}\""), "\"0.0.1-older\""),
    )
    .unwrap();

    let store = root.open();
    assert!(!store.knows_definition(hash(1)));
    assert_eq!(store.warnings()[0].code, codes::CACHE_VERSION_CHANGED);
}

#[test]
fn concurrent_stores_union_their_definitions() {
    let root = TempRoot::new("merge-definitions");
    let mut first = root.open();
    let mut second = root.open();

    first.observe_definitions([hash(1)]);
    first.flush().unwrap();

    second.observe_definitions([hash(2)]);
    second.flush().unwrap();

    assert!(
        second.knows_definition(hash(1)),
        "flush adopts what it merged"
    );
    let reopened = root.open();
    assert!(reopened.knows_definition(hash(1)));
    assert!(reopened.knows_definition(hash(2)));
}

// ---------------------------------------------------------------------------
// Front-end cache
// ---------------------------------------------------------------------------

use ply_core::{EffectAtom, Footprint, Resource, Row, Scheme, TyVar, Type};
use ply_syntax::ast::Mode;

fn content(n: u8) -> ContentHash {
    ContentHash::of(&[n, n.wrapping_add(1), n.wrapping_mul(3)])
}

fn scheme() -> Scheme {
    Scheme {
        ty_vars: vec![TyVar(0)],
        row_vars: vec![],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(0))],
            ret: Box::new(Type::int()),
            effects: Row::empty(),
        },
    }
}

fn footprint() -> Footprint {
    Footprint::from_atoms([EffectAtom::new(
        "db",
        Resource::Named(ply_span::Symbol::new("users")),
        Mode::Read,
    )])
}

fn fingerprint(n: u8) -> SourceFingerprint {
    let mut fp = SourceFingerprint::new(content(n));
    fp.defs.push(DefEntry {
        name: ply_span::Symbol::new("active_users"),
        hash: hash(n),
        span: FileSpan { start: 10, end: 42 },
        kind: DefKind::Fn,
        members: Vec::new(),
        deps: Vec::new(),
    });
    fp.tests.push(CachedTest {
        name: "active_users excludes inactive".to_string(),
        hash: hash(n.wrapping_add(100)),
        nondet: false,
        footprint: footprint(),
        span: FileSpan {
            start: 50,
            end: 120,
        },
        name_span: FileSpan { start: 55, end: 60 },
        deps: Vec::new(),
    });
    fp
}

/// The cache file is rewritten whole, so a run that re-derives exactly what is
/// already stored must not flush at all. This is the difference between a warm
/// run costing nothing and costing a full serialization of the project.
#[test]
fn re_storing_identical_entries_leaves_the_cache_clean() {
    let root = TempRoot::new("frontend-idempotent");
    let file = root.path().join("src/user.ply");
    let def = || CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("m.f", hash(1))]);

    let mut store = root.open();
    store.put_source(&file, fingerprint(1));
    store.put_def(hash(1), def());
    store.flush().unwrap();
    let written = fs::metadata(store.frontend_path()).unwrap().len();

    let mut store = root.open();
    store.put_source(&file, fingerprint(1));
    store.put_def(hash(1), def());
    assert!(!store.frontend_is_dirty(), "identical entries must not dirty the cache");
    store.flush().unwrap();
    assert_eq!(fs::metadata(store.frontend_path()).unwrap().len(), written);

    // A real change still lands.
    store.put_source(&file, fingerprint(2));
    assert!(store.frontend_is_dirty());
    store.flush().unwrap();
    assert_eq!(root.open().source(&file), Some(&fingerprint(2)));
}

/// Two definitions in different modules with the same `DefHash` and different
/// schemes. Sharing a hash is the design, so neither may evict the other — with
/// one slot per hash the loser is rechecked on every run forever.
#[test]
fn two_definitions_sharing_a_hash_each_keep_their_own_interface() {
    let root = TempRoot::new("frontend-shared-hash");
    let shared = hash(1);
    let alpha = ply_span::Symbol::new("alpha.f");
    let beta = ply_span::Symbol::new("beta.g");
    let other = Scheme {
        ty_vars: vec![],
        row_vars: vec![],
        ty: Type::int(),
    };

    let mut store = root.open();
    store.put_def(
        shared,
        CachedDef::new(scheme(), footprint())
            .witnessed_by(vec![NameRef::new(alpha.clone(), shared)]),
    );
    store.put_def(
        shared,
        CachedDef::new(other.clone(), Footprint::empty())
            .witnessed_by(vec![NameRef::new(beta.clone(), shared)]),
    );
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 2);
    assert_eq!(reopened.cached_def_of(shared, &alpha).unwrap().scheme, scheme());
    assert_eq!(reopened.cached_def_of(shared, &beta).unwrap().scheme, other);
    assert_eq!(
        reopened.cached_def_of(shared, &ply_span::Symbol::new("gamma.h")),
        None,
        "a third definition must miss rather than borrow someone else's scheme"
    );

    // Re-storing one replaces its own slot and leaves the other alone.
    let mut store = root.open();
    store.put_def(
        shared,
        CachedDef::new(other.clone(), footprint())
            .witnessed_by(vec![NameRef::new(alpha.clone(), shared)]),
    );
    store.flush().unwrap();
    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 2);
    assert_eq!(reopened.cached_def_of(shared, &alpha).unwrap().footprint, footprint());
    assert_eq!(reopened.cached_def_of(shared, &beta).unwrap().scheme, other);
}

#[test]
fn a_fingerprint_and_an_interface_survive_a_round_trip_through_disk() {
    let root = TempRoot::new("frontend-round-trip");
    let file = root.path().join("src/user.ply");

    let mut store = root.open();
    assert!(store.frontend_is_empty());
    assert!(store.put_source(&file, fingerprint(1)));
    store.put_def(
        hash(1),
        CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("Row", hash(9))]),
    );
    store.put_decl(
        hash(9),
        CachedDecl::new(DeclBody::Effect {
            nondet: false,
            ops: vec![CachedOp {
                name: ply_span::Symbol::new("op"),
                mode: Mode::Read,
                resource_param: true,
                params: vec![Type::int()],
                ret: Type::int(),
            }],
        }),
    );
    store.flush().unwrap();

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert_eq!(reopened.decls_len(), 1);

    let fp = reopened
        .source(&file)
        .expect("keyed by its path under the root");
    assert_eq!(fp, &fingerprint(1));

    let def = reopened.cached_def(hash(1)).unwrap();
    assert_eq!(def.scheme, scheme());
    assert_eq!(def.footprint, footprint());
    assert_eq!(def.names, vec![NameRef::new("Row", hash(9))]);

    let decl = reopened.cached_decl(hash(9)).unwrap();
    assert_eq!(
        decl.body,
        DeclBody::Effect {
            nondet: false,
            ops: vec![CachedOp {
                name: ply_span::Symbol::new("op"),
                mode: Mode::Read,
                resource_param: true,
                params: vec![Type::int()],
                ret: Type::int(),
            }],
        }
    );
}

#[test]
fn a_source_key_is_relative_so_the_cache_survives_the_checkout_moving() {
    let root = TempRoot::new("frontend-relative");
    let mut store = root.open();
    let file = root.path().join("src/user.ply");
    store.put_source(&file, fingerprint(1));
    store.flush().unwrap();

    let text = fs::read_to_string(store.frontend_path()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["frontend_version"], FRONTEND_VERSION);
    assert_eq!(json["format"], 2);
    assert!(
        json["sources"]["src/user.ply"].is_object(),
        "the key must be root-relative with `/` separators, got {}",
        json["sources"]
    );
    assert!(
        !text.contains(root.path().to_str().unwrap()),
        "no absolute path may reach the cache file"
    );
}

#[test]
fn a_path_that_cannot_be_keyed_is_refused_rather_than_mis_keyed() {
    let root = TempRoot::new("frontend-unkeyable");
    let mut store = root.open();

    // Escapes the root, so it would collide with a sibling checkout's file of
    // the same name.
    let outside = root.path().join("../elsewhere.ply");
    assert!(!store.put_source(&outside, fingerprint(1)));
    assert!(store.source(&outside).is_none());
    assert_eq!(store.sources_len(), 0);
}

#[test]
fn the_two_caches_are_versioned_and_invalidated_independently() {
    let root = TempRoot::new("frontend-version");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put_source(&root.path().join("a.ply"), fingerprint(1));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.flush().unwrap();

    let text = fs::read_to_string(store.frontend_path()).unwrap();
    let stale = text.replace(
        &format!("\"{FRONTEND_VERSION}\""),
        "\"0.0.1-some-older-front-end\"",
    );
    assert_ne!(stale, text, "the version must actually appear in the file");
    fs::write(store.frontend_path(), stale).unwrap();

    let reopened = root.open();
    assert_eq!(reopened.sources_len(), 0);
    assert_eq!(reopened.defs_len(), 0);
    assert_eq!(
        reopened.len(),
        1,
        "a stale front end must not discard a result that is still valid"
    );
    assert_eq!(reopened.warnings().len(), 1);
    assert_eq!(reopened.warnings()[0].code, codes::CACHE_VERSION_CHANGED);
    assert!(reopened.warnings()[0].message.contains("front end"));
}

#[test]
fn a_corrupt_front_end_cache_degrades_to_empty_and_repairs_itself() {
    let root = TempRoot::new("frontend-corrupt");
    let mut store = root.open();
    store.put_source(&root.path().join("a.ply"), fingerprint(1));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.flush().unwrap();

    let text = fs::read_to_string(store.frontend_path()).unwrap();
    fs::write(store.frontend_path(), &text[..text.len() / 2]).unwrap();

    let mut damaged = root.open();
    assert!(damaged.frontend_is_empty());
    assert_eq!(damaged.warnings().len(), 1);
    assert_eq!(damaged.warnings()[0].code, codes::CACHE_CORRUPT);
    assert_eq!(damaged.warnings()[0].severity, Severity::Warning);
    assert!(
        damaged.source(&root.path().join("a.ply")).is_none(),
        "a damaged cache must not answer"
    );

    damaged.put_def(hash(2), CachedDef::new(scheme(), footprint()));
    damaged.flush().unwrap();

    let repaired = root.open();
    assert!(repaired.warnings().is_empty());
    assert_eq!(repaired.defs_len(), 1);
    assert!(repaired.cached_def(hash(2)).is_some());
}

#[test]
fn clearing_the_cache_discards_types_as_well_as_results() {
    let root = TempRoot::new("frontend-clear");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put_source(&root.path().join("a.ply"), fingerprint(1));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.put_decl(
        hash(2),
        CachedDecl::new(DeclBody::Type {
            arity: 0,
            ctors: vec![],
        }),
    );
    store.flush().unwrap();
    assert!(store.frontend_path().exists());

    store.clear().unwrap();
    assert_eq!(store.len(), 0);
    assert!(store.frontend_is_empty());
    assert!(!store.frontend_path().exists());

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert!(reopened.frontend_is_empty());
    assert_eq!(reopened.len(), 0);
}

#[test]
fn flush_without_changes_writes_no_front_end_cache_either() {
    let root = TempRoot::new("frontend-no-op");
    let mut store = root.open();
    store.flush().unwrap();
    assert!(!store.frontend_path().exists());
    assert!(temp_files(store.dir()).is_empty());
}

#[test]
fn interfaces_merge_across_processes_because_they_are_content_keyed() {
    let root = TempRoot::new("frontend-merge");
    let mut first = root.open();
    let mut second = root.open();

    first.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    first.flush().unwrap();

    second.put_def(hash(2), CachedDef::new(scheme(), footprint()));
    second.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 2);
    assert!(reopened.cached_def(hash(1)).is_some());
    assert!(reopened.cached_def(hash(2)).is_some());
}

#[test]
fn pruning_drops_dead_files_and_the_interfaces_only_they_referred_to() {
    let root = TempRoot::new("frontend-prune");
    let kept = root.path().join("kept.ply");
    let deleted = root.path().join("deleted.ply");

    let mut store = root.open();
    store.put_source(&kept, fingerprint(1));
    store.put_source(&deleted, fingerprint(2));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.put_def(hash(2), CachedDef::new(scheme(), footprint()));
    store.put_decl(
        hash(2),
        CachedDecl::new(DeclBody::Type {
            arity: 0,
            ctors: vec![],
        }),
    );
    store.flush().unwrap();

    let pruned = store.prune(std::slice::from_ref(&kept));
    assert_eq!(
        pruned,
        Pruned {
            sources: 1,
            defs: 1,
            decls: 1
        }
    );
    assert!(store.source(&deleted).is_none());
    assert!(store.source(&kept).is_some());
    assert!(store.cached_def(hash(1)).is_some());
    store.flush().unwrap();

    // A merging flush would have resurrected everything pruning removed.
    let reopened = root.open();
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert_eq!(reopened.decls_len(), 0);
    assert!(reopened.source(&deleted).is_none());
}

#[test]
fn pruning_to_the_same_file_set_changes_nothing() {
    let root = TempRoot::new("frontend-prune-noop");
    let a = root.path().join("a.ply");
    let mut store = root.open();
    store.put_source(&a, fingerprint(1));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));

    assert_eq!(store.prune(std::slice::from_ref(&a)), Pruned::default());
    assert_eq!(store.sources_len(), 1);
    assert_eq!(store.defs_len(), 1);
}

#[test]
fn source_paths_round_trip_back_to_the_paths_that_were_stored() {
    let root = TempRoot::new("frontend-paths");
    let mut store = root.open();
    let nested = root.path().join("src/nested/user.ply");
    let flat = root.path().join("main.ply");
    store.put_source(&nested, fingerprint(1));
    store.put_source(&flat, fingerprint(2));

    let mut paths = store.source_paths();
    paths.sort();
    assert_eq!(paths, vec![flat, nested]);
}

#[test]
fn a_witness_holds_only_while_every_name_still_denotes_what_it_did() {
    let def = CachedDef::new(scheme(), footprint()).witnessed_by(vec![
        NameRef::new("Row", hash(1)),
        NameRef::new("db", hash(2)),
    ]);

    let resolved = |name: &ply_span::Symbol| match name.as_str() {
        "Row" => Some(hash(1)),
        "db" => Some(hash(2)),
        _ => None,
    };
    assert!(def.witness_holds(resolved));

    // `Row` was edited: same name, different definition.
    let edited = |name: &ply_span::Symbol| match name.as_str() {
        "Row" => Some(hash(7)),
        "db" => Some(hash(2)),
        _ => None,
    };
    assert!(!def.witness_holds(edited));

    // `Row` was renamed away, so the scheme's `Row` no longer denotes anything.
    let renamed = |name: &ply_span::Symbol| match name.as_str() {
        "db" => Some(hash(2)),
        _ => None,
    };
    assert!(!def.witness_holds(renamed));

    assert!(CachedDef::new(scheme(), footprint()).witness_holds(renamed));
}

#[test]
fn an_exports_digest_ignores_order_and_notices_every_change() {
    let a = NameRef::new("a", hash(1));
    let b = NameRef::new("b", hash(2));

    let forward = exports_digest(&[a.clone(), b.clone()]);
    let backward = exports_digest(&[b.clone(), a.clone()]);
    assert_eq!(
        forward, backward,
        "reordering items must not invalidate an importer"
    );

    assert_ne!(
        forward,
        exports_digest(std::slice::from_ref(&a)),
        "a removed export shows"
    );
    assert_ne!(
        forward,
        exports_digest(&[a.clone(), b.clone(), NameRef::new("c", hash(3))]),
        "an added export shows"
    );
    assert_ne!(
        forward,
        exports_digest(&[a.clone(), NameRef::new("b", hash(9))]),
        "an edited export shows"
    );
    // Renaming is the whole point of content addressing: it must show here,
    // because an importer names what it imports.
    assert_ne!(
        forward,
        exports_digest(&[a.clone(), NameRef::new("bb", hash(2))]),
        "a renamed export shows"
    );
}

#[test]
fn a_digest_cannot_be_forged_by_running_two_names_together() {
    let split = exports_digest(&[NameRef::new("ab", hash(1)), NameRef::new("c", hash(2))]);
    let joined = exports_digest(&[NameRef::new("a", hash(1)), NameRef::new("bc", hash(2))]);
    assert_ne!(
        split, joined,
        "names must be length-prefixed, not concatenated"
    );
}

#[test]
fn a_file_span_survives_the_source_ids_of_the_next_run() {
    let first = ply_span::SourceId(3);
    let later = ply_span::SourceId(0);
    let span = Span::new(first, 88, 97);

    let stored = FileSpan::of(span);
    assert_eq!(stored, FileSpan { start: 88, end: 97 });
    assert_eq!(stored.rebase(later), Span::new(later, 88, 97));

    assert_eq!(FileSpan::of(Span::DUMMY), FileSpan { start: 0, end: 0 });
}

#[test]
fn a_content_hash_round_trips_through_hex_and_is_not_a_def_hash() {
    let h = ContentHash::of(b"fn f() -> Int = 1\n");
    assert_eq!(h.to_hex().len(), 64);
    assert_eq!(ContentHash::from_hex(&h.to_hex()), Some(h));
    assert_eq!(h.short(), h.to_hex()[..12]);
    assert_eq!(h.to_string(), h.short());

    assert_ne!(
        h,
        ContentHash::of(b"fn f() -> Int = 1"),
        "a trailing newline is content"
    );
    assert_eq!(ContentHash::from_hex("nonsense"), None);
    assert_eq!(ContentHash::from_hex(&"z".repeat(64)), None);

    let json = serde_json::to_string(&h).unwrap();
    assert_eq!(json, format!("\"{}\"", h.to_hex()));
    assert_eq!(serde_json::from_str::<ContentHash>(&json).unwrap(), h);
    assert!(serde_json::from_str::<ContentHash>("\"zz\"").is_err());
}

#[test]
fn a_stale_result_cache_leaves_the_front_end_alone() {
    let root = TempRoot::new("frontend-survives-runtime-bump");
    let file = root.path().join("a.ply");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put_source(&file, fingerprint(1));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.flush().unwrap();

    let text = fs::read_to_string(root.cache_file()).unwrap();
    fs::write(
        root.cache_file(),
        text.replace(&format!("\"{RUNTIME_VERSION}\""), "\"0.0.1-older-runtime\""),
    )
    .unwrap();

    let reopened = root.open();
    assert!(reopened.is_empty(), "the results are gone");
    assert_eq!(
        reopened.sources_len(),
        1,
        "a type is not invalidated by a change to the evaluator"
    );
    assert_eq!(reopened.defs_len(), 1);
    assert!(reopened.source(&file).is_some());
    assert_eq!(reopened.warnings().len(), 1);
    assert_eq!(reopened.warnings()[0].code, codes::CACHE_VERSION_CHANGED);
}

#[test]
fn every_shape_of_unreadable_front_end_file_degrades_rather_than_crashes() {
    let good = format!(r#""format":2,"frontend_version":"{FRONTEND_VERSION}""#);
    for (tag, contents) in [
        ("fe-empty", String::new()),
        ("fe-not-json", "\u{0}\u{1}garbage\u{ff}".to_string()),
        ("fe-wrong-root-type", "[]".to_string()),
        (
            "fe-future-format",
            r#"{"format":99,"sources":{}}"#.to_string(),
        ),
        ("fe-no-version", r#"{"format":2,"sources":{}}"#.to_string()),
        (
            "fe-bad-hash-key",
            format!(r#"{{{good},"defs":{{"zz":{{"scheme":null}}}}}}"#),
        ),
        (
            "fe-bad-scheme",
            format!(
                r#"{{{good},"defs":{{"{}":{{"scheme":{{"ty_vars":"all of them"}}}}}}}}"#,
                hash(1).to_hex()
            ),
        ),
        (
            "fe-unknown-decl-tag",
            format!(
                r#"{{{good},"decls":{{"{}":{{"body":{{"decl":"module"}}}}}}}}"#,
                hash(1).to_hex()
            ),
        ),
        (
            "fe-truncated-fingerprint",
            format!(r#"{{{good},"sources":{{"a.ply":{{}}}}}}"#),
        ),
    ] {
        let root = TempRoot::new(tag);
        let dir = root.path().join(CACHE_DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("frontend.json"), &contents).unwrap();

        let mut store = root.open();
        assert!(
            store.frontend_is_empty(),
            "{tag} should have degraded to an empty front-end cache"
        );
        assert_eq!(store.warnings().len(), 1, "{tag} must warn");
        assert!(
            store.warnings()[0].message.contains("frontend.json"),
            "{tag} must name the offending file"
        );
        assert!(
            !store.warnings()[0].notes.is_empty(),
            "{tag} must say what happens next"
        );

        store.put_def(hash(5), CachedDef::new(scheme(), footprint()));
        store.flush().unwrap();
        let repaired = root.open();
        assert!(repaired.warnings().is_empty(), "{tag} must repair itself");
        assert_eq!(repaired.defs_len(), 1, "{tag} must keep what it just wrote");
        assert_eq!(
            repaired.sources_len(),
            0,
            "{tag} must not resurrect part of a file it refused to read"
        );
    }
}

/// The crash window: a flush has written its temp file in full and died before
/// the rename. Nothing in it may be visible, and the previous cache must be
/// exactly as it was.
#[test]
fn an_entry_written_but_never_renamed_is_not_observable() {
    let root = TempRoot::new("frontend-crash-window");
    let file = root.path().join("a.ply");

    let mut store = root.open();
    store.put_source(&file, fingerprint(1));
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.flush().unwrap();
    let committed = fs::read_to_string(store.frontend_path()).unwrap();

    // Real bytes for a *later* state of the same cache, produced the way a real
    // flush would produce them.
    let next = TempRoot::new("frontend-crash-window-source");
    let mut ahead = next.open();
    ahead.put_source(&next.path().join("a.ply"), fingerprint(2));
    ahead.put_def(hash(2), CachedDef::new(scheme(), footprint()));
    ahead.flush().unwrap();
    let unrenamed = fs::read_to_string(ahead.frontend_path()).unwrap();
    assert_ne!(unrenamed, committed);

    fs::write(store.dir().join("frontend.4242.0.0.tmp"), &unrenamed).unwrap();

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert!(
        reopened.cached_def(hash(2)).is_none(),
        "an entry that never reached the rename must not be readable"
    );
    assert_eq!(
        reopened.source(&file).unwrap().content_hash,
        fingerprint(1).content_hash,
        "the fingerprint that was committed is the one that answers"
    );
    assert_eq!(
        fs::read_to_string(root.open().frontend_path()).unwrap(),
        committed,
        "opening the cache must not disturb it"
    );
}

#[test]
fn a_reader_never_observes_a_partial_front_end_file_while_writers_run() {
    let root = TempRoot::new("frontend-torn");
    let seeded = root.path().join("seed.ply");
    let mut seed = root.open();
    seed.put_source(&seeded, fingerprint(1));
    seed.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    seed.flush().unwrap();

    const WRITERS: u8 = 3;
    const WRITES: u8 = 20;
    let root = &root;
    let finished = AtomicU64::new(0);
    let finished = &finished;

    std::thread::scope(|scope| {
        for w in 0..WRITERS {
            scope.spawn(move || {
                for i in 0..WRITES {
                    let mut store = root.open();
                    let n = 10 + w * WRITES + i;
                    store.put_source(&root.path().join(format!("f{n}.ply")), fingerprint(n));
                    store.put_def(hash(n), CachedDef::new(scheme(), footprint()));
                    store.flush().unwrap();
                }
                finished.fetch_add(1, Ordering::Release);
            });
        }
        scope.spawn(move || {
            while finished.load(Ordering::Acquire) < WRITERS as u64 {
                let store = root.open();
                assert!(
                    store.warnings().is_empty(),
                    "a reader saw a torn front-end cache: {:?}",
                    store.warnings()
                );
                let seen = store
                    .source(&seeded)
                    .expect("a fingerprint vanished mid-write");
                assert_eq!(seen.content_hash, fingerprint(1).content_hash);
                assert!(store.cached_def(hash(1)).is_some());
            }
        });
    });

    let final_store = root.open();
    assert!(final_store.warnings().is_empty());
    assert_eq!(final_store.defs_len(), 1 + (WRITERS * WRITES) as usize);
    assert_eq!(final_store.sources_len(), 1 + (WRITERS * WRITES) as usize);
}

#[test]
fn a_fingerprint_is_only_believed_against_the_bytes_that_produced_it() {
    let bytes = b"fn f() -> Int = 1\n";
    let fp = SourceFingerprint::new(ContentHash::of(bytes));
    assert!(fp.matches_bytes(bytes));
    assert!(!fp.matches_bytes(b"fn f() -> Int = 2\n"));
    assert!(
        !fp.matches_bytes(b"fn f() -> Int = 1"),
        "reformatting is a content change; gate 1 is conservative on purpose"
    );
}

// ---------------------------------------------------------------------------
// Canonical form
// ---------------------------------------------------------------------------

/// `fn f<a, e>(a) -> a / e`, numbered the way a run's global counter would leave
/// it rather than from zero.
fn counted_scheme(a: u32, e: u32) -> Scheme {
    Scheme {
        ty_vars: vec![TyVar(a)],
        row_vars: vec![ply_core::RowVar(e)],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(a))],
            ret: Box::new(Type::Var(TyVar(a))),
            effects: Row::open(ply_core::RowVar(e)),
        },
    }
}

#[test]
fn a_scheme_is_canonical_by_the_time_it_reaches_the_disk() {
    let root = TempRoot::new("frontend-canonical");
    let mut store = root.open();
    store.put_def(
        hash(1),
        CachedDef::new(counted_scheme(412, 87), footprint()).witnessed_by(vec![
            NameRef::new("Row", hash(9)),
            NameRef::new("db", hash(8)),
        ]),
    );
    store.flush().unwrap();

    let expected = canonicalize_scheme(&counted_scheme(0, 0));
    assert_eq!(
        store.cached_def(hash(1)).unwrap().scheme,
        expected,
        "the counter's numbers must not survive the put"
    );

    let reopened = root.open();
    assert_eq!(reopened.cached_def(hash(1)).unwrap().scheme, expected);
    assert_eq!(
        reopened.cached_def(hash(1)).unwrap().names,
        vec![NameRef::new("Row", hash(9)), NameRef::new("db", hash(8))],
        "a witness is sorted by name so two callers write the same bytes"
    );

    // What the equivalence test does: the same definition checked under a
    // different global counter has to land on byte-identical bytes.
    let other = TempRoot::new("frontend-canonical-other");
    let mut other_store = other.open();
    other_store.put_def(hash(1), CachedDef::new(counted_scheme(3, 1), footprint()));
    other_store.flush().unwrap();
    let mine: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(store.frontend_path()).unwrap()).unwrap();
    let theirs: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(other_store.frontend_path()).unwrap()).unwrap();
    assert_eq!(
        mine["defs"][hash(1).to_hex()]["scheme"],
        theirs["defs"][hash(1).to_hex()]["scheme"]
    );
}

#[test]
fn a_declarations_signatures_are_canonical_on_the_disk_too() {
    let root = TempRoot::new("frontend-canonical-decl");
    let body = |a: u32| DeclBody::Effect {
        nondet: false,
        ops: vec![CachedOp {
            name: ply_span::Symbol::new("op"),
            mode: Mode::Read,
            resource_param: true,
            params: vec![Type::int()],
            ret: Type::Var(TyVar(a)),
        }],
    };

    let mut store = root.open();
    store.put_decl(hash(1), CachedDecl::new(body(77)));
    store.flush().unwrap();

    assert_eq!(
        root.open().cached_decl(hash(1)).unwrap().body,
        canonicalize_decl_body(&body(0))
    );
}

#[test]
fn an_abandoned_front_end_temp_file_does_not_disturb_the_cache() {
    let root = TempRoot::new("frontend-interrupt");
    let mut store = root.open();
    store.put_def(hash(1), CachedDef::new(scheme(), footprint()));
    store.flush().unwrap();
    assert!(
        temp_files(store.dir()).is_empty(),
        "a completed flush leaves no temp file"
    );

    let abandoned = store.dir().join("frontend.999999.0.0.tmp");
    fs::write(&abandoned, "{\"format\": 1, \"frontend_ver").unwrap();

    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 1, "the previous cache is still whole");
    assert!(reopened.warnings().is_empty());

    disk::sweep_temps(store.dir(), None);
    assert!(!abandoned.exists(), "a stale sweep covers both cache files");
}

// ---------------------------------------------------------------------------
// The on-disk schema
// ---------------------------------------------------------------------------

/// Both caches are discarded whole when the version constant they were written
/// under does not match this build's, so a shape change that is *not* paired
/// with a bump is read back as though it were the old shape — either as a parse
/// failure, which costs a whole project's work, or silently as the wrong type,
/// which is worse. Nothing about a struct definition makes that visible at the
/// point of the edit, so these two tests are the notice.
const BUMP: &str = "the on-disk schema changed. Bump the version constant this \
                    cache is keyed on, then update this pin — a build that reads \
                    an entry written under the old shape has no other way to know";

fn pin_fingerprint() -> SourceFingerprint {
    SourceFingerprint {
        content_hash: content(1),
        imports: vec![ImportEdge {
            module: ply_span::Symbol::new("store.db"),
            exports: content(2),
        }],
        deps: vec![NameRef::new("store.db.get", hash(7))],
        defs: vec![
            DefEntry {
                name: ply_span::Symbol::new("user.active_users"),
                hash: hash(2),
                span: FileSpan { start: 10, end: 42 },
                kind: DefKind::Fn,
                members: vec![],
                deps: vec![ply_span::Symbol::new("store.db.get")],
            },
            DefEntry {
                name: ply_span::Symbol::new("user.User"),
                hash: hash(3),
                span: FileSpan { start: 50, end: 80 },
                kind: DefKind::Type,
                members: vec![Member {
                    name: ply_span::Symbol::new("user.Active"),
                    span: FileSpan { start: 60, end: 66 },
                }],
                deps: vec![],
            },
        ],
        tests: vec![CachedTest {
            name: "active_users excludes inactive".to_string(),
            hash: hash(5),
            nondet: true,
            footprint: footprint(),
            span: FileSpan {
                start: 90,
                end: 140,
            },
            name_span: FileSpan {
                start: 95,
                end: 100,
            },
            deps: vec![ply_span::Symbol::new("user.active_users")],
        }],
    }
}

fn pin_def() -> CachedDef {
    CachedDef::new(counted_scheme(9, 4), footprint())
        .witnessed_by(vec![NameRef::new("user.User", hash(3))])
}

fn pin_type_decl() -> CachedDecl {
    CachedDecl::new(DeclBody::Type {
        arity: 1,
        ctors: vec![CachedCtor {
            fields: vec![Type::Var(TyVar(6))],
            scheme: Scheme {
                ty_vars: vec![TyVar(6)],
                row_vars: vec![],
                ty: Type::Fn {
                    params: vec![Type::Var(TyVar(6))],
                    ret: Box::new(Type::Con(
                        ply_span::Symbol::new("user.User"),
                        vec![Type::Var(TyVar(6))],
                    )),
                    effects: Row::empty(),
                },
            },
        }],
    })
    .witnessed_by(vec![NameRef::new("user.User", hash(3))])
}

fn pin_effect_decl() -> CachedDecl {
    CachedDecl::new(DeclBody::Effect {
        nondet: true,
        ops: vec![CachedOp {
            name: ply_span::Symbol::new("op"),
            mode: Mode::Write,
            resource_param: true,
            params: vec![
                Type::int(),
                Type::Record(std::collections::BTreeMap::from([(
                    ply_span::Symbol::new("id"),
                    Type::int(),
                )])),
            ],
            ret: Type::unit(),
        }],
    })
}

fn pinned_frontend() -> serde_json::Value {
    serde_json::json!({
      "format": 2,
      "frontend_version": FRONTEND_VERSION,
      "sources": {
        "src/user.ply": {
          "content_hash": content(1).to_hex(),
          "imports": [{ "module": "store.db", "exports": content(2).to_hex() }],
          "deps": [{ "name": "store.db.get", "hash": hash(7).to_hex() }],
          "defs": [
            {
              "name": "user.active_users",
              "hash": hash(2).to_hex(),
              "span": { "start": 10, "end": 42 },
              "kind": "fn",
              "deps": ["store.db.get"]
            },
            {
              "name": "user.User",
              "hash": hash(3).to_hex(),
              "span": { "start": 50, "end": 80 },
              "kind": "type",
              "members": [{ "name": "user.Active", "span": { "start": 60, "end": 66 } }]
            }
          ],
          "tests": [{
            "name": "active_users excludes inactive",
            "hash": hash(5).to_hex(),
            "nondet": true,
            "footprint": [{ "effect": "db", "resource": { "Named": "users" }, "mode": "read" }],
            "span": { "start": 90, "end": 140 },
            "name_span": { "start": 95, "end": 100 },
            "deps": ["user.active_users"]
          }]
        }
      },
      "defs": {
        hash(2).to_hex(): [{
          "scheme": {
            "ty_vars": [0],
            "row_vars": [0],
            "ty": { "Fn": {
              "params": [{ "Var": 0 }],
              "ret": { "Var": 0 },
              "effects": { "atoms": [], "tail": 0 }
            }}
          },
          "footprint": [{ "effect": "db", "resource": { "Named": "users" }, "mode": "read" }],
          "names": [{ "name": "user.User", "hash": hash(3).to_hex() }]
        }]
      },
      "decls": {
        hash(3).to_hex(): [{
          "body": {
            "decl": "type",
            "arity": 1,
            "ctors": [{
              "fields": [{ "Var": 0 }],
              "scheme": {
                "ty_vars": [0],
                "row_vars": [],
                "ty": { "Fn": {
                  "params": [{ "Var": 0 }],
                  "ret": { "Con": ["user.User", [{ "Var": 0 }]] },
                  "effects": { "atoms": [], "tail": null }
                }}
              }
            }]
          },
          "names": [{ "name": "user.User", "hash": hash(3).to_hex() }]
        }],
        hash(4).to_hex(): [{
          "body": {
            "decl": "effect",
            "nondet": true,
            "ops": [{
              "name": "op",
              "mode": "write",
              "resource_param": true,
              "params": [
                { "Con": ["Int", []] },
                { "Record": { "id": { "Con": ["Int", []] } } }
              ],
              "ret": { "Con": ["Unit", []] }
            }]
          }
        }]
      }
    })
}

#[test]
fn the_front_end_on_disk_schema_is_pinned() {
    let root = TempRoot::new("pin-frontend");
    let mut store = root.open();
    store.put_source(&root.path().join("src/user.ply"), pin_fingerprint());
    store.put_def(hash(2), pin_def());
    store.put_decl(hash(3), pin_type_decl());
    store.put_decl(hash(4), pin_effect_decl());
    store.flush().unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(store.frontend_path()).unwrap()).unwrap();
    assert_eq!(written, pinned_frontend(), "{BUMP} (FRONTEND_VERSION)");
}

/// The other direction, which the forward pin cannot show: an entry written by
/// an earlier run of *this* version still loads into the same values.
#[test]
fn a_front_end_cache_in_the_pinned_shape_loads_back_unchanged() {
    let root = TempRoot::new("pin-frontend-read");
    let dir = root.path().join(CACHE_DIR_NAME);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("frontend.json"),
        serde_json::to_string_pretty(&pinned_frontend()).unwrap(),
    )
    .unwrap();

    let store = root.open();
    assert!(store.warnings().is_empty(), "{BUMP} (FRONTEND_VERSION)");
    assert_eq!(
        store.source(&root.path().join("src/user.ply")),
        Some(&pin_fingerprint())
    );
    assert_eq!(
        store.cached_def(hash(2)),
        Some(&pin_def().canonicalized()),
        "a scheme is stored canonical, so it comes back canonical"
    );
    assert_eq!(
        store.cached_decl(hash(3)),
        Some(&pin_type_decl().canonicalized())
    );
    assert_eq!(
        store.cached_decl(hash(4)),
        Some(&pin_effect_decl().canonicalized())
    );
}

#[test]
fn the_result_cache_on_disk_schema_is_pinned() {
    let root = TempRoot::new("pin-results");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put(hash(2), failure());
    store.observe_definitions([hash(3)]);
    store.flush().unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.cache_file()).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
          "format": 1,
          "runtime_version": RUNTIME_VERSION,
          "results": {
            hash(1).to_hex(): { "outcome": "pass" },
            hash(2).to_hex(): {
              "outcome": "fail",
              "message": "assertion failed: expected 0, found -5",
              "diagnostic": {
                "severity": "error",
                "code": "E0501",
                "message": "assertion failed",
                "labels": [{
                  "span": { "source": 3, "start": 88, "end": 97 },
                  "message": "expected 0, found -5",
                  "primary": true
                }],
                "notes": ["suspects: apply_debit"]
              }
            }
          },
          "definitions": [hash(3).to_hex()]
        }),
        "{BUMP} (RUNTIME_VERSION)"
    );
}
