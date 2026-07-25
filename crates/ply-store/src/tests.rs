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
