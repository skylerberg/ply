use super::*;
use ply_span::{Severity, Span, codes as span_codes};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique directory under the system temp dir, removed on drop.
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

    fn passes_file(&self) -> PathBuf {
        self.0.join(CACHE_DIR_NAME).join("passes.json")
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
    assert_eq!(json["format"], 2);
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

    // What an interrupted `ply test` leaves behind: a half-written temp file that was never
    // renamed.
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
    let declined = AtomicU64::new(0);
    let declined = &declined;

    std::thread::scope(|scope| {
        for w in 0..WRITERS {
            scope.spawn(move || {
                for i in 0..WRITES {
                    let mut store = root.open();
                    store.put(hash(10 + w * WRITES + i), failure());
                    store.flush().unwrap();
                    if lock_declined(&store) {
                        declined.fetch_add(1, Ordering::Release);
                    }
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
    // `flush` waits `LOCK_WAIT` for the cache lock and, if it does not get it, warns and writes
    // nothing (`lib.rs`'s `flush`, `disk.rs`'s "a caller that proceeds unlocked risks losing a
    // concurrent writer's entries, which costs a re-run, but still cannot produce a torn file").
    let lost = declined.load(Ordering::Acquire) as usize;
    assert_eq!(
        final_store.len() + lost,
        1 + (WRITERS * WRITES) as usize,
        "{} entries on disk and {lost} flushes that declined the lock do not \
         account for the {} written",
        final_store.len(),
        1 + (WRITERS * WRITES) as usize
    );
}

/// Whether the last `flush` on this store declined the cache lock and wrote nothing.
fn lock_declined(store: &Store) -> bool {
    store
        .warnings()
        .iter()
        .any(|w| w.message.contains("holding the cache lock"))
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

/// A writer that cannot take the lock now writes nothing, so a lock a killed process left behind
/// would block every write forever if it were not broken by age.
#[test]
fn a_lock_left_by_a_dead_process_never_blocks_a_flush() {
    let root = TempRoot::new("lock-abandoned");
    let mut store = root.open();
    let lock = store.dir().join("lock");
    fs::write(&lock, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();

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

    // The two records answer different questions and must never leak into each other: a definition
    // is not a test that passed.
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

fn body(bytes: &[u8]) -> DefBody {
    DefBody::new(BODY_ENCODING, bytes.to_vec())
}

fn def() -> CachedDef {
    CachedDef::new(scheme(), footprint())
}

impl TempRoot {
    fn index_file(&self) -> PathBuf {
        self.0.join(CACHE_DIR_NAME).join("frontend.idx")
    }

    fn data_file(&self) -> PathBuf {
        self.0.join(CACHE_DIR_NAME).join("frontend.dat")
    }
}

/// The header field offsets the format fixes, named here so a test that damages one says which one
/// it damaged.
mod at {
    pub(super) const MAGIC: usize = 0;
    pub(super) const SCHEMA: usize = 16;
    pub(super) const NONCE: usize = 48;
    pub(super) const DATA_LEN: usize = 56;
    pub(super) const VERSION: usize = 64;
    pub(super) const CHECKSUM: usize = 100;
}

fn patch(path: &Path, offset: usize, bytes: &[u8]) {
    let mut file = fs::read(path).unwrap();
    file[offset..offset + bytes.len()].copy_from_slice(bytes);
    fs::write(path, file).unwrap();
}

/// Every frame in a data file, as `(offset, kind, payload length)`, by walking it the way nothing
/// in the store ever does — a frame is only ever reached through an index record that already
/// claims where it is.
fn frames(path: &Path) -> Vec<(usize, u8, usize)> {
    let bytes = fs::read(path).unwrap();
    let mut out = Vec::new();
    let mut at = 56;
    while at + 13 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap()) as usize;
        if at + 13 + len > bytes.len() {
            break;
        }
        out.push((at, bytes[at], len));
        at += 13 + len;
    }
    out
}

fn frame_checksum(kind: u8, payload: &[u8]) -> [u8; 8] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[kind]);
    hasher.update(&(payload.len() as u32).to_le_bytes());
    hasher.update(payload);
    let mut out = [0u8; 8];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    out
}

/// Rewrites one frame's payload and repairs its checksum, which is what an encoding change looks
/// like from the outside: bytes that verify but no longer mean what the decoder expects.
fn rewrite_payload(path: &Path, frame: usize, edit: impl FnOnce(&mut Vec<u8>)) {
    let (offset, kind, len) = frames(path)[frame];
    let mut bytes = fs::read(path).unwrap();
    let mut payload = bytes[offset + 13..offset + 13 + len].to_vec();
    edit(&mut payload);
    assert_eq!(payload.len(), len, "this helper cannot resize a frame");
    bytes[offset + 5..offset + 13].copy_from_slice(&frame_checksum(kind, &payload));
    bytes[offset + 13..offset + 13 + len].copy_from_slice(&payload);
    fs::write(path, bytes).unwrap();
}

#[test]
fn a_fingerprint_an_interface_and_a_body_survive_a_round_trip_through_disk() {
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
    store.put_body(hash(1), body(b"\x00\x01\xfe\xff normalized"));
    store.flush().unwrap();

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert_eq!(reopened.decls_len(), 1);
    assert_eq!(reopened.bodies_len(), 1);

    assert_eq!(
        reopened.fingerprint(&file).as_deref(),
        Some(&fingerprint(1)),
        "keyed by its path under the root"
    );

    let cached = reopened.def(hash(1)).unwrap();
    assert_eq!(cached.scheme, scheme());
    assert_eq!(cached.footprint, footprint());
    assert_eq!(cached.names, vec![NameRef::new("Row", hash(9))]);

    let decl = reopened.decl(hash(9)).unwrap();
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
    assert_eq!(
        reopened.body(hash(1)).unwrap().as_bytes(),
        b"\x00\x01\xfe\xff normalized",
        "arbitrary bytes must survive, not just text"
    );
    assert!(reopened.body(hash(2)).is_none());
}

/// The index is rewritten whole on every flush, so a run that re-derives exactly what is already
/// stored must not flush at all.
#[test]
fn re_storing_identical_entries_leaves_the_cache_clean() {
    let root = TempRoot::new("frontend-idempotent");
    let file = root.path().join("src/user.ply");
    let witnessed =
        || CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("m.f", hash(1))]);

    let mut store = root.open();
    store.put_source(&file, fingerprint(1));
    store.put_def(hash(1), witnessed());
    store.flush().unwrap();
    let index = fs::read(root.index_file()).unwrap();
    let data = fs::read(root.data_file()).unwrap();

    let mut store = root.open();
    store.put_source(&file, fingerprint(1));
    store.put_def(hash(1), witnessed());
    assert!(
        !store.frontend_is_dirty(),
        "identical entries must not dirty the cache"
    );
    store.flush().unwrap();
    assert_eq!(fs::read(root.index_file()).unwrap(), index);
    assert_eq!(
        fs::read(root.data_file()).unwrap(),
        data,
        "an append-only file must not grow for an entry it already holds"
    );

    store.put_source(&file, fingerprint(2));
    assert!(store.frontend_is_dirty());
    store.flush().unwrap();
    assert_eq!(
        root.open().fingerprint(&file).as_deref(),
        Some(&fingerprint(2))
    );
}

/// Two definitions in different modules with the same `DefHash` and different schemes.
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
    assert_eq!(reopened.def_of(shared, &alpha).unwrap().scheme, scheme());
    assert_eq!(reopened.def_of(shared, &beta).unwrap().scheme, other);
    assert_eq!(
        reopened.def_of(shared, &ply_span::Symbol::new("gamma.h")),
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
    assert_eq!(
        reopened.def_of(shared, &alpha).unwrap().footprint,
        footprint()
    );
    assert_eq!(reopened.def_of(shared, &beta).unwrap().scheme, other);
}

#[test]
fn a_source_key_is_relative_so_the_cache_survives_the_checkout_moving() {
    let root = TempRoot::new("frontend-relative");
    let mut store = root.open();
    let file = root.path().join("src/user.ply");
    store.put_source(&file, fingerprint(1));
    store.flush().unwrap();

    let index = fs::read(root.index_file()).unwrap();
    let text = String::from_utf8_lossy(&index).to_string();
    assert!(
        text.contains("src/user.ply"),
        "the key must be root-relative with `/` separators"
    );
    assert!(
        !text.contains(root.path().to_str().unwrap()),
        "no absolute path may reach the cache"
    );
}

#[test]
fn a_path_that_cannot_be_keyed_is_refused_rather_than_mis_keyed() {
    let root = TempRoot::new("frontend-unkeyable");
    let mut store = root.open();

    // Escapes the root, so it would collide with a sibling checkout's file of the same name.
    let outside = root.path().join("../elsewhere.ply");
    assert!(!store.put_source(&outside, fingerprint(1)));
    assert!(store.fingerprint(&outside).is_none());
    assert_eq!(store.sources_len(), 0);
}

fn seeded(tag: &str) -> TempRoot {
    let root = TempRoot::new(tag);
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put_source(&root.path().join("a.ply"), fingerprint(1));
    store.put_def(hash(1), def());
    store.put_body(hash(1), body(b"one"));
    store.flush().unwrap();
    root
}

#[test]
fn the_two_caches_are_versioned_and_invalidated_independently() {
    let root = seeded("frontend-version");
    patch(&root.index_file(), at::VERSION, &[0xab; 32]);

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

/// The gate that exists because a binary encoding cannot fail loudly on its own: a stored shape
/// that changed without a version bump would otherwise decode into a plausible wrong `Footprint`,
/// and footprints decide which tests may run concurrently.
#[test]
fn a_cache_written_against_other_shapes_is_refused() {
    for (tag, file) in [
        (
            "frontend-schema-idx",
            TempRoot::index_file as fn(&TempRoot) -> PathBuf,
        ),
        ("frontend-schema-dat", TempRoot::data_file),
    ] {
        let root = seeded(tag);
        patch(&file(&root), at::SCHEMA, &[0x5a; 32]);

        let store = root.open();
        assert!(store.frontend_is_empty(), "{tag}");
        assert_eq!(store.warnings().len(), 1, "{tag}");
        assert_eq!(
            store.warnings()[0].code,
            codes::CACHE_VERSION_CHANGED,
            "{tag}"
        );
        assert_eq!(store.len(), 1, "{tag}: the result cache is untouched");
    }
}

#[test]
fn a_corrupt_header_degrades_to_an_empty_cache() {
    for (tag, file) in [
        (
            "frontend-magic-idx",
            TempRoot::index_file as fn(&TempRoot) -> PathBuf,
        ),
        ("frontend-magic-dat", TempRoot::data_file),
    ] {
        let root = seeded(tag);
        patch(&file(&root), at::MAGIC, b"NOTAPLY!");

        let store = root.open();
        assert!(store.frontend_is_empty(), "{tag}");
        assert_eq!(store.warnings().len(), 1, "{tag}");
        assert_eq!(store.warnings()[0].code, codes::CACHE_CORRUPT, "{tag}");
        assert_eq!(store.warnings()[0].severity, Severity::Warning, "{tag}");
    }
}

#[test]
fn an_index_that_does_not_match_its_own_checksum_is_refused() {
    let root = seeded("frontend-index-checksum");
    patch(&root.index_file(), at::CHECKSUM, &[0u8; 32]);

    let mut store = root.open();
    assert!(store.frontend_is_empty());
    assert_eq!(store.warnings()[0].code, codes::CACHE_CORRUPT);

    store.put_def(hash(2), def());
    store.flush().unwrap();
    let repaired = root.open();
    assert!(repaired.warnings().is_empty(), "it must repair itself");
    assert_eq!(repaired.defs_len(), 1);
    assert_eq!(
        repaired.sources_len(),
        0,
        "nothing from a file it refused to read may come back"
    );
}

/// The two files carry a shared nonce, so an index cannot be read against a data file it was not
/// written for — the case where one of the two is deleted, or restored from a backup.
#[test]
fn an_index_and_a_data_file_that_were_not_written_together_are_refused() {
    let root = seeded("frontend-unpaired");
    let other = seeded("frontend-unpaired-other");
    fs::copy(other.data_file(), root.data_file()).unwrap();

    let store = root.open();
    assert!(store.frontend_is_empty());
    assert_eq!(store.warnings().len(), 1);
    assert_eq!(store.warnings()[0].code, codes::CACHE_CORRUPT);
    assert!(store.warnings()[0].message.contains("data file"));
}

#[test]
fn an_index_without_its_data_file_is_refused_rather_than_answered_from() {
    let root = seeded("frontend-no-data");
    fs::remove_file(root.data_file()).unwrap();

    let store = root.open();
    assert!(store.frontend_is_empty());
    assert_eq!(store.warnings().len(), 1);
    assert_eq!(store.warnings()[0].code, codes::CACHE_CORRUPT);
}

/// The bound that makes an offset safe to follow.
#[test]
fn an_index_entry_pointing_past_the_end_of_the_data_file_is_refused() {
    let root = seeded("frontend-past-end");
    let data_len = u64::from_le_bytes(
        fs::read(root.index_file()).unwrap()[at::DATA_LEN..at::DATA_LEN + 8]
            .try_into()
            .unwrap(),
    );
    patch(
        &root.index_file(),
        at::DATA_LEN,
        &(data_len - 16).to_le_bytes(),
    );

    let store = root.open();
    assert!(store.frontend_is_empty());
    assert_eq!(store.warnings().len(), 1);
    assert_eq!(store.warnings()[0].code, codes::CACHE_CORRUPT);
    assert!(
        store.warnings()[0]
            .message
            .contains("past the end of the data file"),
        "got {}",
        store.warnings()[0].message
    );
}

/// A killed writer leaves bytes above the length the index vouches for.
#[test]
fn a_torn_append_is_invisible_and_is_recovered_by_the_next_flush() {
    let root = seeded("frontend-torn-append");
    let committed = fs::metadata(root.data_file()).unwrap().len();
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(root.data_file())
            .unwrap();
        file.write_all(&[0xcc; 4096]).unwrap();
    }

    let mut store = root.open();
    assert!(
        store.warnings().is_empty(),
        "a torn tail is not a corrupt cache: {:?}",
        store.warnings()
    );
    assert_eq!(store.defs_len(), 1);
    assert_eq!(store.sources_len(), 1);

    store.put_def(hash(2), def());
    store.flush().unwrap();
    let after = fs::metadata(root.data_file()).unwrap().len();
    assert!(
        after < committed + 4096,
        "the torn tail must be truncated, not appended after: {committed} -> {after}"
    );

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert_eq!(reopened.defs_len(), 2);
    assert!(reopened.def(hash(1)).is_some());
    assert!(reopened.def(hash(2)).is_some());
}

/// A frame whose bytes were damaged in place.
#[test]
fn an_entry_whose_checksum_fails_is_not_cached_and_is_reported() {
    let root = TempRoot::new("frontend-frame-checksum");
    let mut store = root.open();
    store.put_def(hash(1), def());
    store.put_def(hash(2), CachedDef::new(scheme(), Footprint::empty()));
    store.flush().unwrap();

    let (offset, _, len) = frames(&root.data_file())[0];
    let mut bytes = fs::read(root.data_file()).unwrap();
    bytes[offset + 13 + len / 2] ^= 0xff;
    fs::write(root.data_file(), bytes).unwrap();

    let mut store = root.open();
    assert!(
        store.warnings().is_empty(),
        "the damage is inside an entry, so opening cannot see it"
    );
    assert!(
        store.def(hash(1)).is_none(),
        "a damaged entry must not answer"
    );
    assert!(
        store.def(hash(2)).is_some(),
        "one bad frame must not cost the rest of the cache"
    );

    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, codes::CACHE_CORRUPT);
    assert!(warnings[0].message.contains("checksum"));
    assert!(!warnings[0].notes.is_empty());
}

/// The case a checksum cannot catch and a tag must: bytes that verify but were written to a
/// different shape.
#[test]
fn a_payload_whose_shape_drifted_is_refused_rather_than_misread() {
    let root = TempRoot::new("frontend-shape-drift");
    let mut store = root.open();
    store.put_def(hash(1), def());
    store.flush().unwrap();

    // The first byte of a payload is the tag that says what shape follows.
    rewrite_payload(&root.data_file(), 0, |payload| payload[0] ^= 0x01);

    let mut store = root.open();
    assert!(store.def(hash(1)).is_none());
    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, codes::CACHE_CORRUPT);
}

/// The crash window: a flush has written its temp index in full and died before the rename.
#[test]
fn an_entry_written_but_never_renamed_is_not_observable() {
    let root = seeded("frontend-crash-window");
    let committed = fs::read(root.index_file()).unwrap();

    let ahead = seeded("frontend-crash-window-source");
    let mut moved = ahead.open();
    moved.put_def(hash(2), CachedDef::new(scheme(), Footprint::empty()));
    moved.flush().unwrap();
    let unrenamed = fs::read(ahead.index_file()).unwrap();
    assert_ne!(unrenamed, committed);

    fs::write(
        root.path()
            .join(CACHE_DIR_NAME)
            .join("frontend.4242.0.0.tmp"),
        &unrenamed,
    )
    .unwrap();

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert!(
        reopened.def(hash(2)).is_none(),
        "an entry that never reached the rename must not be readable"
    );
    assert_eq!(
        fs::read(root.index_file()).unwrap(),
        committed,
        "opening the cache must not disturb it"
    );
}

/// Frames a writer appended but whose index it has not yet renamed into place are equally
/// invisible: the index on disk is the only thing that says an entry exists.
#[test]
fn an_appended_frame_no_index_names_is_not_observable() {
    let root = seeded("frontend-unindexed-frame");
    let index = fs::read(root.index_file()).unwrap();

    let source = TempRoot::new("frontend-unindexed-frame-source");
    let mut other = source.open();
    other.put_def(hash(7), CachedDef::new(scheme(), Footprint::empty()));
    other.flush().unwrap();
    let donor = fs::read(source.data_file()).unwrap();

    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(root.data_file())
            .unwrap();
        file.write_all(&donor[56..]).unwrap();
    }

    let store = root.open();
    assert!(store.warnings().is_empty());
    assert!(store.def(hash(7)).is_none());
    assert_eq!(store.defs_len(), 1);
    assert_eq!(fs::read(root.index_file()).unwrap(), index);
}

#[test]
fn clearing_the_cache_discards_types_as_well_as_results() {
    let root = seeded("frontend-clear");
    let mut store = root.open();
    assert!(store.frontend_path().exists());

    store.clear().unwrap();
    assert_eq!(store.len(), 0);
    assert!(store.frontend_is_empty());
    assert!(!store.frontend_path().exists());
    assert!(!store.frontend_data_path().exists());

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
    assert!(!store.frontend_data_path().exists());
    assert!(temp_files(store.dir()).is_empty());
}

#[test]
fn interfaces_merge_across_processes_because_they_are_content_keyed() {
    let root = TempRoot::new("frontend-merge");
    let mut first = root.open();
    let mut second = root.open();

    first.put_def(hash(1), def());
    first.flush().unwrap();

    second.put_def(hash(2), def());
    second.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 2);
    assert!(reopened.def(hash(1)).is_some());
    assert!(reopened.def(hash(2)).is_some());
}

/// A writer that cannot take the lock keeps its work in memory and says so.
#[test]
fn a_writer_that_cannot_take_the_lock_writes_nothing() {
    let root = TempRoot::new("frontend-lock-refused");
    let mut store = root.open();
    let held = disk::Lock::acquire(store.dir());
    assert!(held.held);

    store.put_def(hash(1), def());
    store.flush().unwrap();
    assert!(
        !store.frontend_path().exists(),
        "nothing may be written without the lock"
    );
    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("lock"));
    assert!(store.frontend_is_dirty(), "the work is still pending");

    drop(held);
    store.flush().unwrap();
    assert_eq!(root.open().defs_len(), 1);
}

#[test]
fn pruning_drops_dead_files_and_the_interfaces_only_they_referred_to() {
    let root = TempRoot::new("frontend-prune");
    let kept = root.path().join("kept.ply");
    let deleted = root.path().join("deleted.ply");

    let mut store = root.open();
    store.put_source(&kept, fingerprint(1));
    store.put_source(&deleted, fingerprint(2));
    store.put_def(hash(1), def());
    store.put_def(hash(2), def());
    store.put_decl(
        hash(2),
        CachedDecl::new(DeclBody::Type {
            arity: 0,
            ctors: vec![],
        }),
    );
    store.put_body(hash(1), body(b"kept"));
    store.put_body(hash(2), body(b"dead"));
    store.flush().unwrap();

    let pruned = store.prune(std::slice::from_ref(&kept));
    assert_eq!(
        pruned,
        Pruned {
            sources: 1,
            defs: 1,
            decls: 1,
            bodies: 1,
        }
    );
    assert!(store.has_body(hash(1)));
    assert!(!store.has_body(hash(2)));
    assert!(store.fingerprint(&deleted).is_none());
    assert!(store.fingerprint(&kept).is_some());
    assert!(store.def(hash(1)).is_some());
    store.flush().unwrap();

    // A merging flush would have resurrected everything pruning removed.
    let reopened = root.open();
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert_eq!(reopened.decls_len(), 0);
    assert_eq!(reopened.bodies_len(), 1);
    assert!(reopened.fingerprint(&deleted).is_none());
}

fn pass_record(test: u8, closure: &[(&str, u8)]) -> PassRecord {
    PassRecord {
        test_hash: hash(test),
        closure: closure
            .iter()
            .map(|(name, h)| (Symbol::new(*name), hash(*h)))
            .collect(),
        decls: Default::default(),
    }
}

#[test]
fn a_pass_record_survives_a_flush_and_a_reopen() {
    let root = TempRoot::new("pass-record-roundtrip");
    let key = Symbol::new("ledger.balances");
    let mut store = root.open();
    assert!(store.pass_record(&key).is_none());

    store.put_pass_record(key.clone(), pass_record(1, &[("ledger.post", 2)]));
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(
        reopened.pass_record(&key),
        Some(&pass_record(1, &[("ledger.post", 2)]))
    );
    assert_eq!(reopened.pass_records_len(), 1);
}

/// One record per test, so the baseline is always the *last* configuration it passed at rather than
/// the first.
#[test]
fn a_later_pass_replaces_the_baseline_rather_than_accumulating() {
    let root = TempRoot::new("pass-record-overwrite");
    let key = Symbol::new("ledger.balances");
    let mut store = root.open();
    store.put_pass_record(key.clone(), pass_record(1, &[("ledger.post", 2)]));
    store.flush().unwrap();
    store.put_pass_record(key.clone(), pass_record(3, &[("ledger.post", 4)]));
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.pass_records_len(), 1);
    assert_eq!(
        reopened.pass_record(&key),
        Some(&pass_record(3, &[("ledger.post", 4)]))
    );
}

/// The records are read on the first question rather than at `open`, so a corrupt file produces no
/// warning until something asks — and asking must not answer "never passed" without saying why it
/// cannot tell.
#[test]
fn corrupt_pass_records_warn_and_do_not_claim_the_test_never_passed() {
    let root = TempRoot::new("pass-record-corrupt");
    let key = Symbol::new("ledger.balances");
    let mut store = root.open();
    store.put_pass_record(key.clone(), pass_record(1, &[("ledger.post", 2)]));
    store.flush().unwrap();
    fs::write(root.passes_file(), "{ not json at all").unwrap();

    let mut store = root.open();
    assert!(
        store.take_warnings().is_empty(),
        "`open` must not read them"
    );
    assert!(store.pass_record(&key).is_none());

    let warnings = store.warnings();
    let warning = warnings
        .first()
        .expect("an unreadable baseline has to be reported");
    assert_eq!(warning.code, crate::codes::CACHE_CORRUPT);
    assert!(
        warning.message.contains("pass records"),
        "the failing file has to be named: {}",
        warning.message
    );
    assert!(
        warning.notes.iter().any(|n| n.contains("no test re-runs")),
        "losing a baseline re-runs nothing, and the note must not imply it does: {:?}",
        warning.notes
    );
}

/// Re-recording an identical baseline must not dirty the cache, or every warm run over a green
/// project rewrites the result cache for nothing.
#[test]
fn re_recording_the_same_baseline_writes_nothing() {
    let root = TempRoot::new("pass-record-clean");
    let key = Symbol::new("ledger.balances");
    let mut store = root.open();
    store.put_pass_record(key.clone(), pass_record(1, &[("ledger.post", 2)]));
    store.flush().unwrap();

    let written = fs::metadata(root.passes_file())
        .unwrap()
        .modified()
        .unwrap();
    let mut store = root.open();
    store.put_pass_record(key, pass_record(1, &[("ledger.post", 2)]));
    store.flush().unwrap();
    assert_eq!(
        fs::metadata(root.passes_file())
            .unwrap()
            .modified()
            .unwrap(),
        written,
        "an unchanged baseline rewrote the pass records"
    );
    assert!(
        !root.cache_file().exists(),
        "recording a baseline rewrote the result cache, which holds no baselines"
    );
}

/// ADR 0004's silent-wrongness path: prune deletes the baselines, and every later failure degrades
/// to `no_bodies` with no error to explain it.
#[test]
fn pruning_keeps_the_bodies_a_baseline_names_even_when_no_file_declares_them() {
    let root = TempRoot::new("prune-keeps-baselines");
    let kept = root.path().join("kept.ply");
    let deleted = root.path().join("deleted.ply");

    let mut store = root.open();
    store.put_source(&kept, fingerprint(1));
    store.put_source(&deleted, fingerprint(2));
    store.put_def(hash(1), def());
    store.put_def(hash(2), def());
    store.put_body(hash(1), body(b"kept"));
    store.put_body(hash(2), body(b"the baseline's"));
    store.put_pass_record(
        Symbol::new("ledger.balances"),
        pass_record(9, &[("ledger.gone", 2)]),
    );
    store.flush().unwrap();

    store.prune(std::slice::from_ref(&kept));
    assert!(
        store.has_body(hash(2)),
        "pruning deleted the body a baseline named"
    );
    assert!(store.fingerprint(&deleted).is_none(), "the file still went");
    store.flush().unwrap();
    assert!(root.open().has_body(hash(2)));
}

/// The retention is not unconditional: a body no surviving file *and* no baseline names is still
/// garbage, or `prune` would stop reclaiming anything once a single test had ever passed.
#[test]
fn a_baseline_retains_only_the_hashes_it_actually_names() {
    let root = TempRoot::new("prune-baseline-scope");
    let kept = root.path().join("kept.ply");
    let deleted = root.path().join("deleted.ply");

    let mut store = root.open();
    store.put_source(&kept, fingerprint(1));
    store.put_source(&deleted, fingerprint(2));
    store.put_def(hash(2), def());
    store.put_body(hash(2), body(b"nobody's"));
    store.put_pass_record(
        Symbol::new("ledger.balances"),
        pass_record(9, &[("ledger.elsewhere", 7)]),
    );
    store.flush().unwrap();

    store.prune(std::slice::from_ref(&kept));
    assert!(!store.has_body(hash(2)));
}

#[test]
fn clearing_discards_the_baselines_with_everything_else() {
    let root = TempRoot::new("pass-record-clear");
    let key = Symbol::new("ledger.balances");
    let mut store = root.open();
    store.put_pass_record(key.clone(), pass_record(1, &[("ledger.post", 2)]));
    store.flush().unwrap();

    store.clear().unwrap();
    assert!(store.pass_record(&key).is_none());
    assert!(root.open().pass_record(&key).is_none());
}

#[test]
fn pruning_to_the_same_file_set_changes_nothing() {
    let root = TempRoot::new("frontend-prune-noop");
    let a = root.path().join("a.ply");
    let mut store = root.open();
    store.put_source(&a, fingerprint(1));
    store.put_def(hash(1), def());

    assert_eq!(store.prune(std::slice::from_ref(&a)), Pruned::default());
    assert_eq!(store.sources_len(), 1);
    assert_eq!(store.defs_len(), 1);
}

/// A prune that drops nothing must leave the cache clean, or every run over an unchanged project
/// rewrites the index it just read.
#[test]
fn pruning_a_project_that_did_not_change_leaves_the_cache_clean() {
    let root = TempRoot::new("frontend-prune-clean");
    let a = root.path().join("a.ply");
    let mut store = root.open();
    store.put_source(&a, fingerprint(1));
    store.put_def(hash(1), def());
    store.flush().unwrap();

    let mut store = root.open();
    assert_eq!(store.prune(std::slice::from_ref(&a)), Pruned::default());
    assert!(!store.frontend_is_dirty());
}

#[test]
fn forgetting_a_source_survives_the_flush_that_follows_it() {
    let root = TempRoot::new("frontend-forget");
    let a = root.path().join("a.ply");
    let b = root.path().join("b.ply");
    let mut store = root.open();
    store.put_source(&a, fingerprint(1));
    store.put_source(&b, fingerprint(2));
    store.flush().unwrap();

    let mut store = root.open();
    assert!(store.forget_source(&a));
    assert!(!store.forget_source(&a), "it is already gone");
    assert!(store.fingerprint(&a).is_none());
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.sources_len(), 1);
    assert!(reopened.fingerprint(&a).is_none());
    assert!(reopened.fingerprint(&b).is_some());
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
    let cached = CachedDef::new(scheme(), footprint()).witnessed_by(vec![
        NameRef::new("Row", hash(1)),
        NameRef::new("db", hash(2)),
    ]);

    let resolved = |name: &ply_span::Symbol| match name.as_str() {
        "Row" => Some(hash(1)),
        "db" => Some(hash(2)),
        _ => None,
    };
    assert!(cached.witness_holds(resolved));

    // `Row` was edited: same name, different definition.
    let edited = |name: &ply_span::Symbol| match name.as_str() {
        "Row" => Some(hash(7)),
        "db" => Some(hash(2)),
        _ => None,
    };
    assert!(!cached.witness_holds(edited));

    // `Row` was renamed away, so the scheme's `Row` no longer denotes anything.
    let renamed = |name: &ply_span::Symbol| match name.as_str() {
        "db" => Some(hash(2)),
        _ => None,
    };
    assert!(!cached.witness_holds(renamed));

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
    // Renaming is the whole point of content addressing: it must show here, because an importer
    // names what it imports.
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
    let root = seeded("frontend-survives-runtime-bump");
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
    assert!(reopened.fingerprint(&root.path().join("a.ply")).is_some());
    assert_eq!(reopened.warnings().len(), 1);
    assert_eq!(reopened.warnings()[0].code, codes::CACHE_VERSION_CHANGED);
}

#[test]
fn every_shape_of_unreadable_front_end_file_degrades_rather_than_crashes() {
    for (tag, damage) in [
        (
            "fe-empty",
            &(|f: &Path| fs::write(f, []).unwrap()) as &dyn Fn(&Path),
        ),
        ("fe-garbage", &|f| {
            fs::write(f, b"\x00\x01garbage\xff".repeat(40)).unwrap()
        }),
        ("fe-truncated", &|f| {
            let bytes = fs::read(f).unwrap();
            fs::write(f, &bytes[..bytes.len() / 2]).unwrap();
        }),
        ("fe-header-only", &|f| {
            let bytes = fs::read(f).unwrap();
            fs::write(f, &bytes[..132]).unwrap();
        }),
        ("fe-future-format", &|f| patch(f, 8, &99u32.to_le_bytes())),
        ("fe-nonce", &|f| patch(f, at::NONCE, &[0x11; 8])),
        ("fe-section-past-end", &|f| {
            // The first descriptor's offset, pushed beyond the file.
            patch(f, 132 + 8, &u64::MAX.to_le_bytes());
        }),
    ] {
        let root = seeded(tag);
        damage(&root.index_file());

        let mut store = root.open();
        assert!(
            store.frontend_is_empty(),
            "{tag} should have degraded to an empty front-end cache"
        );
        assert_eq!(store.warnings().len(), 1, "{tag} must warn");
        assert!(
            store.warnings()[0].message.contains("frontend.idx"),
            "{tag} must name the offending file: {}",
            store.warnings()[0].message
        );
        assert!(
            !store.warnings()[0].notes.is_empty(),
            "{tag} must say what happens next"
        );
        assert_eq!(store.len(), 1, "{tag} must not touch the result cache");

        store.put_def(hash(5), def());
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

/// Readers take no lock, which is only sound because an append never moves a byte another process
/// has already mapped.
#[test]
fn a_reader_never_observes_a_partial_front_end_cache_while_writers_run() {
    let root = TempRoot::new("frontend-torn");
    let seeded_file = root.path().join("seed.ply");
    let mut seed = root.open();
    seed.put_source(&seeded_file, fingerprint(1));
    seed.put_def(hash(1), def());
    seed.flush().unwrap();

    const WRITERS: u8 = 3;
    const WRITES: u8 = 20;
    let root = &root;
    let finished = AtomicU64::new(0);
    let finished = &finished;
    let declined = AtomicU64::new(0);
    let declined = &declined;

    std::thread::scope(|scope| {
        for w in 0..WRITERS {
            scope.spawn(move || {
                for i in 0..WRITES {
                    let mut store = root.open();
                    let n = 10 + w * WRITES + i;
                    store.put_source(&root.path().join(format!("f{n}.ply")), fingerprint(n));
                    store.put_def(hash(n), def());
                    store.flush().unwrap();
                    if lock_declined(&store) {
                        declined.fetch_add(1, Ordering::Release);
                    }
                }
                finished.fetch_add(1, Ordering::Release);
            });
        }
        scope.spawn(move || {
            while finished.load(Ordering::Acquire) < WRITERS as u64 {
                let store = root.open();
                let seen = store
                    .fingerprint(&seeded_file)
                    .expect("a fingerprint vanished mid-write");
                assert_eq!(seen.content_hash, fingerprint(1).content_hash);
                assert!(store.def(hash(1)).is_some());
                assert!(
                    store.warnings().is_empty(),
                    "a reader saw a torn front-end cache: {:?}",
                    store.warnings()
                );
            }
        });
    });

    let final_store = root.open();
    assert!(final_store.warnings().is_empty());
    // `flush` waits `LOCK_WAIT` for the cache lock and, if it does not get it, warns and writes
    // nothing (`lib.rs`'s `flush`, `disk.rs`'s "a caller that proceeds unlocked risks losing a
    // concurrent writer's entries, which costs a re-run, but still cannot produce a torn file").
    let lost = declined.load(Ordering::Acquire) as usize;
    assert_eq!(
        final_store.defs_len() + lost,
        1 + (WRITERS * WRITES) as usize,
        "{} defs and {lost} declined flushes do not account for the {} written",
        final_store.defs_len(),
        1 + (WRITERS * WRITES) as usize
    );
    assert_eq!(
        final_store.sources_len() + lost,
        1 + (WRITERS * WRITES) as usize,
        "{} sources and {lost} declined flushes do not account for the {} written",
        final_store.sources_len(),
        1 + (WRITERS * WRITES) as usize
    );
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

/// `fn f<a, e>(a) -> a / e`, numbered the way a run's global counter would leave it rather than
/// from zero.
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
        store.def(hash(1)).unwrap().scheme,
        expected,
        "the counter's numbers must not survive the put"
    );

    let reopened = root.open();
    assert_eq!(reopened.def(hash(1)).unwrap().scheme, expected);
    assert_eq!(
        reopened.def(hash(1)).unwrap().names,
        vec![NameRef::new("Row", hash(9)), NameRef::new("db", hash(8))],
        "a witness is sorted by name so two callers write the same bytes"
    );

    // What the equivalence test does: the same definition checked under a different global counter
    // has to land on byte-identical bytes.
    let other = TempRoot::new("frontend-canonical-other");
    let mut other_store = other.open();
    other_store.put_def(
        hash(1),
        CachedDef::new(counted_scheme(3, 1), footprint()).witnessed_by(vec![
            NameRef::new("db", hash(8)),
            NameRef::new("Row", hash(9)),
        ]),
    );
    other_store.flush().unwrap();
    let mine = fs::read(root.data_file()).unwrap();
    let theirs = fs::read(other.data_file()).unwrap();
    assert_eq!(
        mine[56..],
        theirs[56..],
        "one definition must reach the disk as one sequence of bytes"
    );
}

#[test]
fn a_declarations_signatures_are_canonical_on_the_disk_too() {
    let root = TempRoot::new("frontend-canonical-decl");
    let declared = |a: u32| DeclBody::Effect {
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
    store.put_decl(hash(1), CachedDecl::new(declared(77)));
    store.flush().unwrap();

    assert_eq!(
        root.open().decl(hash(1)).unwrap().body,
        canonicalize_decl_body(&declared(0))
    );
}

#[test]
fn an_abandoned_front_end_temp_file_does_not_disturb_the_cache() {
    let root = TempRoot::new("frontend-interrupt");
    let mut store = root.open();
    store.put_def(hash(1), def());
    store.flush().unwrap();
    assert!(
        temp_files(store.dir()).is_empty(),
        "a completed flush leaves no temp file"
    );

    let abandoned = store.dir().join("frontend.999999.0.0.tmp");
    fs::write(&abandoned, "half an index").unwrap();

    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 1, "the previous cache is still whole");
    assert!(reopened.warnings().is_empty());

    disk::sweep_temps(store.dir(), None);
    assert!(!abandoned.exists(), "a stale sweep covers both cache files");
}

/// Both caches are discarded whole when the version constant they were written under does not match
/// this build's, so a shape change that is *not* paired with a bump is read back as though it were
/// the old shape — either as a parse failure, which costs a whole project's work, or silently as
/// the wrong type, which is worse.
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

/// `performed` is deliberately narrower than `footprint` and `row_aliases` is deliberately
/// non-empty: a pin over a value that happened to take the default would not move when the field's
/// encoding did.
fn pin_def() -> CachedDef {
    CachedDef::new(counted_scheme(9, 4), footprint())
        .performing(Footprint::empty())
        .written_as(vec![ply_span::Symbol::new("Web")])
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

fn digest(bytes: &[u8]) -> String {
    ContentHash::of(bytes).to_hex()
}

/// The encoder is the schema, so the pin is over what it emits.
#[test]
fn the_front_end_entry_encoding_is_pinned() {
    let found: Vec<(&str, String)> = vec![
        (
            "fingerprint",
            digest(&crate::codec::encode_fingerprint(&pin_fingerprint())),
        ),
        (
            "def",
            digest(&crate::codec::encode_def(&pin_def().canonicalized())),
        ),
        (
            "type declaration",
            digest(&crate::codec::encode_decl(&pin_type_decl().canonicalized())),
        ),
        (
            "effect declaration",
            digest(&crate::codec::encode_decl(
                &pin_effect_decl().canonicalized(),
            )),
        ),
        (
            "body",
            digest(&crate::codec::encode_body(&body(&[0x20, 0xca, 0xfe]))),
        ),
    ];
    let pinned = [
        ("fingerprint", PINNED_FINGERPRINT),
        ("def", PINNED_DEF),
        ("type declaration", PINNED_TYPE_DECL),
        ("effect declaration", PINNED_EFFECT_DECL),
        ("body", PINNED_BODY),
    ];
    let found: Vec<(&str, &str)> = found.iter().map(|(w, d)| (*w, d.as_str())).collect();
    assert_eq!(found, pinned.to_vec(), "{BUMP}");
}

const PINNED_FINGERPRINT: &str = "02e7e6340261171838cb49303958e371ae61d01db729090dec71f8fa7896a003";
const PINNED_DEF: &str = "edbd2fa35344f8a5fd38f3e745727cd96ca1d7c57255560c0064004b249c6aab";
const PINNED_TYPE_DECL: &str = "563d17593d11975f979c1714dbf0845f19433439fd5517b15d8d7750dd2d6d91";
const PINNED_EFFECT_DECL: &str = "0b5bc11329b83fd823d762923323c2373dfb1e9e985756570dd709013e1a004d";
const PINNED_BODY: &str = "adf0f67e207566df6efe0eb0ac42e091e3f554a4d7b36ec34cd37b8306f21900";

/// The other direction, which the forward pin cannot show: bytes written by an earlier run of this
/// version still decode to the same values, through the framing and the index rather than through
/// the encoder alone.
#[test]
fn a_front_end_cache_in_the_pinned_shape_loads_back_unchanged() {
    let root = TempRoot::new("pin-frontend-read");
    let mut store = root.open();
    store.put_source(&root.path().join("src/user.ply"), pin_fingerprint());
    store.put_def(hash(2), pin_def());
    store.put_decl(hash(3), pin_type_decl());
    store.put_decl(hash(4), pin_effect_decl());
    store.put_body(hash(2), body(&[0x20, 0xca, 0xfe]));
    store.flush().unwrap();

    let store = root.open();
    assert!(store.warnings().is_empty(), "{BUMP}");
    assert_eq!(
        store
            .fingerprint(&root.path().join("src/user.ply"))
            .as_deref(),
        Some(&pin_fingerprint())
    );
    assert_eq!(
        store.def(hash(2)).as_deref(),
        Some(&pin_def().canonicalized()),
        "a scheme is stored canonical, so it comes back canonical"
    );
    assert_eq!(
        store.decl(hash(3)).as_deref(),
        Some(&pin_type_decl().canonicalized())
    );
    assert_eq!(
        store.decl(hash(4)).as_deref(),
        Some(&pin_effect_decl().canonicalized())
    );
    assert_eq!(
        store.body(hash(2)).as_deref(),
        Some(&body(&[0x20, 0xca, 0xfe]))
    );
}

/// The encoding version is checked per entry as well as by the schema fingerprint, so an entry that
/// somehow outlived a bump still cannot be handed to a decoder that would read it as this build's
/// shape.
#[test]
fn a_body_written_under_another_encoding_is_not_handed_back() {
    let root = TempRoot::new("body-encoding");
    let mut store = root.open();
    store.put_body(hash(1), DefBody::new(BODY_ENCODING + 1, vec![1, 2, 3]));
    assert!(store.body(hash(1)).is_none());
    assert!(!store.has_body(hash(1)));
    assert_eq!(store.bodies_len(), 1, "it is stored, just not readable");
}

/// A body is keyed by a hash of itself, so two different bodies under one hash means the encoding
/// depends on something the hash does not cover.
#[test]
fn two_different_bodies_for_one_hash_are_refused_and_reported() {
    let root = TempRoot::new("body-conflict");
    let mut store = root.open();
    store.put_body(hash(1), body(b"first"));
    store.put_body(hash(1), body(b"first"));
    assert!(
        store.warnings().is_empty(),
        "re-storing the same body is not a conflict"
    );

    store.put_body(hash(1), body(b"second"));
    assert_eq!(store.body(hash(1)).unwrap().as_bytes(), b"first");
    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, codes::CACHE_CORRUPT);
    assert_eq!(warnings[0].severity, Severity::Warning);

    // And the same across a flush, where the body it disagrees with is on disk.
    store.flush().unwrap();
    let mut reopened = root.open();
    reopened.put_body(hash(1), body(b"second"));
    assert_eq!(reopened.body(hash(1)).unwrap().as_bytes(), b"first");
    assert_eq!(reopened.take_warnings().len(), 1);
}

/// The whole point of the split: two definitions in different modules with one hash share a body,
/// because a body is name-free and therefore a function of that hash.
#[test]
fn one_body_serves_every_definition_that_shares_its_hash() {
    let root = TempRoot::new("body-shared");
    let mut store = root.open();
    store.put_def(
        hash(1),
        CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("a.f", hash(1))]),
    );
    store.put_def(
        hash(1),
        CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("b.g", hash(1))]),
    );
    store.put_body(hash(1), body(b"one computation"));
    store.flush().unwrap();

    let reopened = root.open();
    assert_eq!(reopened.defs_len(), 2, "two interfaces");
    assert_eq!(reopened.bodies_len(), 1, "one body");
    assert!(
        reopened
            .def_of(hash(1), &ply_span::Symbol::new("a.f"))
            .is_some()
    );
    assert!(
        reopened
            .def_of(hash(1), &ply_span::Symbol::new("b.g"))
            .is_some()
    );
}

#[test]
fn lookup_finds_a_definition_by_full_name_simple_name_or_hash_prefix() {
    let root = TempRoot::new("lookup");
    let file = root.path().join("src/user.ply");
    let mut store = root.open();
    let mut fp = SourceFingerprint::new(content(1));
    fp.defs.push(DefEntry {
        name: ply_span::Symbol::new("user.active_users"),
        hash: hash(9),
        span: FileSpan { start: 10, end: 42 },
        kind: DefKind::Fn,
        members: Vec::new(),
        deps: Vec::new(),
    });
    store.put_source(&file, fp);

    for query in [
        "user.active_users",
        "active_users",
        &hash(9).to_hex()[..8],
        &hash(9).to_hex(),
    ] {
        let found = store.lookup(query);
        assert_eq!(found.len(), 1, "`{query}` should have matched once");
        let Found::Def(found) = &found[0] else {
            panic!("`{query}` matched a test");
        };
        assert_eq!(found.hash, hash(9));
        assert_eq!(found.kind, DefKind::Fn);
        assert_eq!(found.path, file);
        assert_eq!(found.span, FileSpan { start: 10, end: 42 });
    }

    assert!(
        store.lookup("user").is_empty(),
        "a module is not a definition"
    );
    assert!(
        store.lookup("ab").is_empty(),
        "two hex characters are too ambiguous to be a prefix"
    );
}

#[test]
fn lookup_finds_a_test_by_label_and_keeps_it_distinct_from_a_definition() {
    let root = TempRoot::new("lookup-test");
    let file = root.path().join("src/user.ply");
    let mut store = root.open();
    store.put_source(&file, fingerprint(1));

    let found = store.lookup("active_users excludes inactive");
    assert_eq!(found.len(), 1);
    let Found::Test(test) = &found[0] else {
        panic!("a test label must not match as a definition");
    };
    assert_eq!(test.hash, hash(101));
    assert!(!test.nondet);
    assert_eq!(test.footprint, footprint());
}

/// One name in two modules is two answers, and the store holds no namespace that could pick between
/// them.
#[test]
fn lookup_returns_every_match_rather_than_refusing() {
    let root = TempRoot::new("lookup-ambiguous");
    let mut store = root.open();
    for (module, n) in [("a", 1u8), ("b", 2)] {
        let mut fp = SourceFingerprint::new(content(n));
        fp.defs.push(DefEntry {
            name: ply_span::Symbol::new(format!("{module}.place")),
            hash: hash(n),
            span: FileSpan { start: 0, end: 1 },
            kind: DefKind::Fn,
            members: Vec::new(),
            deps: Vec::new(),
        });
        store.put_source(&root.path().join(format!("{module}.ply")), fp);
    }

    let found = store.lookup("place");
    assert_eq!(found.len(), 2);
    let mut hashes: Vec<DefHash> = found.iter().map(Found::hash).collect();
    hashes.sort();
    assert_eq!(hashes, vec![hash(1), hash(2)]);
}

#[test]
fn stats_counts_both_caches_and_measures_what_is_on_disk() {
    let root = TempRoot::new("stats");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.observe_definitions([hash(1), hash(2)]);
    store.put_source(&root.path().join("src/user.ply"), fingerprint(1));
    store.put_def(hash(1), def());
    store.put_decl(
        hash(2),
        CachedDecl::new(DeclBody::Type {
            arity: 0,
            ctors: vec![],
        }),
    );
    store.put_body(hash(1), body(b"body"));
    store.flush().unwrap();

    let stats = root.open().stats();
    assert_eq!(stats.results, 1);
    assert_eq!(stats.definitions_seen, 2);
    assert_eq!(stats.sources, 1);
    assert_eq!(stats.defs, 1);
    assert_eq!(stats.decls, 1);
    assert_eq!(stats.bodies, 1);
    assert!(stats.results_bytes > 0);
    assert!(stats.index_bytes > 0);
    assert!(stats.data_bytes > 0);
    assert_eq!(
        stats.garbage_bytes,
        Some(0),
        "nothing has been superseded yet"
    );
}

/// The number `ply cache stats` reports and compaction reclaims: what the data file holds that no
/// index record names.
#[test]
fn garbage_is_what_no_index_record_names() {
    let root = TempRoot::new("garbage");
    let mut store = root.open();
    store.put_def(
        hash(1),
        CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("m.f", hash(1))]),
    );
    store.flush().unwrap();
    assert_eq!(store.stats().garbage_bytes, Some(0));

    let mut store = root.open();
    store.put_def(
        hash(1),
        CachedDef::new(scheme(), Footprint::empty())
            .witnessed_by(vec![NameRef::new("m.f", hash(1))]),
    );
    store.flush().unwrap();
    let garbage = store.stats().garbage_bytes.unwrap();
    assert!(garbage > 0, "the superseded interface is unreachable");

    store.compact(&[]).unwrap();
    assert_eq!(store.stats().garbage_bytes, Some(0));
}

#[test]
fn compacting_drops_what_no_surviving_file_refers_to_and_shrinks_the_cache() {
    let root = TempRoot::new("compact");
    let kept = root.path().join("kept.ply");
    let deleted = root.path().join("deleted.ply");

    let mut store = root.open();
    store.put_source(&kept, fingerprint(1));
    store.put_source(&deleted, fingerprint(2));
    store.put_def(hash(1), def());
    store.put_def(hash(2), def());
    store.put_body(hash(2), body(&[7u8; 512]));
    store.flush().unwrap();

    let mut store = root.open();
    let compaction = store.compact(std::slice::from_ref(&kept)).unwrap();
    assert_eq!(compaction.dropped.sources, 1);
    assert_eq!(compaction.dropped.defs, 1);
    assert_eq!(compaction.dropped.bodies, 1);
    assert!(
        compaction.bytes_after < compaction.bytes_before,
        "compaction has to actually reclaim the bytes: {} -> {}",
        compaction.bytes_before,
        compaction.bytes_after
    );

    let reopened = root.open();
    assert!(reopened.warnings().is_empty());
    assert_eq!(reopened.sources_len(), 1);
    assert_eq!(reopened.defs_len(), 1);
    assert_eq!(reopened.bodies_len(), 0);
    assert!(reopened.def(hash(1)).is_some());
    assert!(
        reopened.get(hash(1)).is_none() && reopened.is_empty(),
        "compaction is a front-end concern and must not touch results"
    );
}

/// `Arc` rather than `&`: an entry the store decoded on demand is owned by nothing the borrow could
/// point into, and a materialized one has to be able to outlive the lookup that produced it.
#[test]
fn a_materialized_entry_outlives_the_store_that_produced_it() {
    let root = TempRoot::new("arc-entries");
    let (cached, decl, fingerprint_of_file) = {
        let mut store = root.open();
        store.put_def(hash(1), def());
        store.put_decl(
            hash(2),
            CachedDecl::new(DeclBody::Type {
                arity: 0,
                ctors: vec![],
            }),
        );
        let file = root.path().join("src/user.ply");
        store.put_source(&file, fingerprint(1));
        store.flush().unwrap();
        (
            store.def(hash(1)).unwrap(),
            store.decl(hash(2)).unwrap(),
            store.fingerprint(&file).unwrap(),
        )
    };
    assert_eq!(cached.footprint, footprint());
    assert!(matches!(decl.body, DeclBody::Type { arity: 0, .. }));
    assert_eq!(fingerprint_of_file.content_hash, content(1));
}

/// The budget the format exists for: opening a ten-thousand-definition cache must decode nothing
/// and cost a read plus a checksum.
#[test]
fn opening_a_ten_thousand_definition_cache_is_under_the_budget() {
    let root = TempRoot::new("open-budget");
    let mut store = root.open();
    let mut defs: Vec<DefHash> = Vec::new();
    for file in 0..400u32 {
        let mut fp = SourceFingerprint::new(ContentHash::of(&file.to_le_bytes()));
        for n in 0..25u32 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&file.to_le_bytes());
            bytes[4..8].copy_from_slice(&n.to_le_bytes());
            let hash = DefHash(bytes);
            let name = ply_span::Symbol::new(format!("m{file}.d{n}"));
            fp.defs.push(DefEntry {
                name: name.clone(),
                hash,
                span: FileSpan {
                    start: n,
                    end: n + 10,
                },
                kind: DefKind::Fn,
                members: Vec::new(),
                deps: Vec::new(),
            });
            store.put_def(
                hash,
                CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new(name, hash)]),
            );
            defs.push(hash);
        }
        store.put_source(&root.path().join(format!("src/m{file}.ply")), fp);
    }
    store.flush().unwrap();
    assert_eq!(store.defs_len(), 10_000);

    let started = std::time::Instant::now();
    let reopened = root.open();
    let elapsed = started.elapsed();
    assert!(reopened.warnings().is_empty());

    let index_bytes = reopened.stats().index_bytes;
    eprintln!(
        "Store::open at 10,000 definitions: {elapsed:?} (index {index_bytes} bytes, data {} bytes)",
        reopened.stats().data_bytes
    );
    // An unoptimized BLAKE3 over half a megabyte of index dominates a debug build; the budget the
    // ADR sets is a release number.
    let budget = if cfg!(debug_assertions) {
        std::time::Duration::from_millis(250)
    } else {
        std::time::Duration::from_millis(5)
    };
    assert!(elapsed < budget, "Store::open took {elapsed:?}");

    // And it decoded nothing: an entry asked for afterwards still answers.
    assert!(reopened.def(defs[9_999]).is_some());
}

#[test]
fn the_result_cache_on_disk_schema_is_pinned() {
    let root = TempRoot::new("pin-results");
    let mut store = root.open();
    store.put(hash(1), Outcome::Pass);
    store.put(hash(2), failure());
    store.observe_definitions([hash(3)]);
    store.put_pass_record(
        Symbol::new("ledger.balance never goes negative"),
        PassRecord {
            test_hash: hash(1),
            closure: [(Symbol::new("ledger.apply_debit"), hash(4))]
                .into_iter()
                .collect(),
            decls: Default::default(),
        },
    );
    store.flush().unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.cache_file()).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
          "format": 2,
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

    let passes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.passes_file()).unwrap()).unwrap();
    assert_eq!(
        passes,
        serde_json::json!({
          "format": 2,
          "runtime_version": RUNTIME_VERSION,
          "passes": {
            "ledger.balance never goes negative": {
              "test_hash": hash(1).to_hex(),
              "closure": { "ledger.apply_debit": hash(4).to_hex() }
            }
          }
        }),
        "{BUMP} (RUNTIME_VERSION)"
    );
}

/// The one thing that must survive a format bump: a cache written before the pass records moved out
/// still answers, and the run that reads it relocates them rather than dropping them.
#[test]
fn a_format_one_result_cache_keeps_its_baselines_and_is_rewritten() {
    let root = TempRoot::new("pin-results-migrate");
    let key = Symbol::new("ledger.balance never goes negative");
    let record = pass_record(1, &[("ledger.apply_debit", 4)]);
    let legacy = serde_json::json!({
      "format": 1,
      "runtime_version": RUNTIME_VERSION,
      "results": { hash(1).to_hex(): { "outcome": "pass" } },
      "definitions": [hash(3).to_hex()],
      "passes": {
        key.to_string(): {
          "test_hash": hash(1).to_hex(),
          "closure": { "ledger.apply_debit": hash(4).to_hex() }
        }
      }
    });
    fs::create_dir_all(root.cache_file().parent().unwrap()).unwrap();
    fs::write(
        root.cache_file(),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let mut store = root.open();
    assert!(store.warnings().is_empty());
    assert_eq!(store.get(hash(1)).map(|o| o.is_pass()), Some(true));
    assert_eq!(store.pass_record(&key), Some(&record));
    store.flush().unwrap();

    let rewritten: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.cache_file()).unwrap()).unwrap();
    assert_eq!(rewritten["format"], 2);
    assert!(
        rewritten.get("passes").is_none(),
        "the inline copy outlived the migration"
    );

    let reopened = root.open();
    assert_eq!(reopened.pass_record(&key), Some(&record));
    assert_eq!(reopened.get(hash(1)).map(|o| o.is_pass()), Some(true));
    assert!(reopened.knows_definition(hash(3)));
}

/// The regression that made this split necessary: the budget covers `Store::open`, and a result
/// cache full of baselines is part of what it opens.
#[test]
fn a_baseline_for_every_test_does_not_slow_the_open() {
    let root = TempRoot::new("open-budget-passes");
    let mut store = root.open();
    for test in 0..5_000u32 {
        let closure: Vec<(String, DefHash)> = (0..12u32)
            .map(|d| {
                let mut bytes = [0u8; 32];
                bytes[0..4].copy_from_slice(&test.to_le_bytes());
                bytes[4..8].copy_from_slice(&d.to_le_bytes());
                (format!("m{test}.d{d}"), DefHash(bytes))
            })
            .collect();
        store.put_pass_record(
            Symbol::new(format!("m{test}.t{test} holds for a seeded fixture")),
            PassRecord {
                test_hash: hash(test as u8),
                closure: closure
                    .into_iter()
                    .map(|(name, hash)| (Symbol::new(name), hash))
                    .collect(),
                decls: Default::default(),
            },
        );
    }
    store.flush().unwrap();
    assert!(fs::metadata(root.passes_file()).unwrap().len() > 4_000_000);

    let started = std::time::Instant::now();
    let reopened = root.open();
    let elapsed = started.elapsed();
    let budget = if cfg!(debug_assertions) {
        std::time::Duration::from_millis(250)
    } else {
        std::time::Duration::from_millis(5)
    };
    assert!(elapsed < budget, "Store::open took {elapsed:?}");

    // And the records are still there for the one run that needs them.
    assert!(
        reopened
            .pass_record(&Symbol::new("m4999.t4999 holds for a seeded fixture"))
            .is_some()
    );
}

/// The stdlib digest survives a reopen, and survives a `RUNTIME_VERSION` change — which is exactly
/// why it is its own file rather than a field of the result cache.
#[test]
fn the_stdlib_digest_round_trips_and_is_written_only_when_it_moves() {
    let root = TempRoot::new("stdlib-digest");
    let mut store = root.open();
    assert_eq!(store.stdlib_digest(), None, "a cold cache records nothing");

    store.set_stdlib_digest("b3:aaaaaaaaaaaa".to_string());
    store.flush().unwrap();
    assert_eq!(
        root.open().stdlib_digest().as_deref(),
        Some("b3:aaaaaaaaaaaa")
    );

    // The same digest is not a write: an unchanged compiler must touch no file.
    let path = root.path().join(CACHE_DIR_NAME).join("stdlib");
    let before = fs::metadata(&path).unwrap().len();
    let mut store = root.open();
    store.set_stdlib_digest("b3:aaaaaaaaaaaa".to_string());
    store.flush().unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), before);

    let mut store = root.open();
    store.set_stdlib_digest("b3:bbbbbbbbbbbb".to_string());
    assert_eq!(
        store.stdlib_digest().as_deref(),
        Some("b3:bbbbbbbbbbbb"),
        "a reader sees this run's digest before it is flushed"
    );
    store.flush().unwrap();
    assert_eq!(
        root.open().stdlib_digest().as_deref(),
        Some("b3:bbbbbbbbbbbb")
    );

    // `ply cache clear` forgets it: after a clear there is nothing left for a moved stdlib to have
    // invalidated.
    let mut store = root.open();
    store.clear().unwrap();
    assert_eq!(store.stdlib_digest(), None);
    assert_eq!(root.open().stdlib_digest(), None);
}
