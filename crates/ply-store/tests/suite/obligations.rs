//! The two files M8 adds, and the one property that makes them affordable: neither is read at
//! [`Store::open`].

use ply_hash::DefHash;
use ply_span::Symbol;
use ply_store::{
    CachedCases, CachedCertificate, CachedEvidence, CachedObligation, CachedRule, PROVER_VERSION,
    ReviewRecord, Store,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique directory under the system temp dir, removed on drop — the same device this crate's
/// unit tests use, and for the same reason: a dev-dependency for one `mkdir` is not worth the build
/// time.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ply-obligations-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempRoot(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

fn proof() -> CachedObligation {
    CachedObligation {
        tier: "proved".to_string(),
        evidence: CachedEvidence::Proof(CachedCertificate {
            rules: vec![
                CachedRule::LinearArithmetic,
                CachedRule::Unfold {
                    def: Symbol::new("ledger.fee"),
                    depth: 2,
                },
            ],
            steps: 41,
            guard_satisfiable: true,
            sorts: vec![Symbol::new("a")],
        }),
    }
}

fn sample(kept: u32) -> CachedObligation {
    CachedObligation {
        tier: if kept >= 25 { "property" } else { "example" }.to_string(),
        evidence: CachedEvidence::Cases(CachedCases {
            generated: 200,
            kept,
            rejected: 200 - kept,
            roots: vec![0],
            instantiations: Vec::new(),
        }),
    }
}

fn obligations_path(root: &Path) -> std::path::PathBuf {
    root.join(".ply-cache").join("obligations.json")
}

fn reviews_path(root: &Path) -> std::path::PathBuf {
    root.join(".ply-cache").join("reviews.json")
}

#[test]
fn an_obligation_survives_a_flush_and_a_reopen_with_its_evidence_intact() {
    let dir = TempRoot::new("t");
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put_obligation(key(1), proof());
        store.put_obligation(key(2), sample(200));
        store.flush().unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.obligation(key(1)), Some(&proof()));
    assert_eq!(store.obligation(key(2)), Some(&sample(200)));
    assert_eq!(store.obligation(key(3)), None);
    assert_eq!(store.obligations_len(), 2);
}

/// The recorded tier is written down so that a disagreement with the evidence is *detectable*.
#[test]
fn the_recorded_tier_survives_the_round_trip() {
    let dir = TempRoot::new("t");
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put_obligation(key(1), proof());
        store.put_obligation(key(2), sample(200));
        store.put_obligation(key(3), sample(7));
        store.flush().unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.obligation(key(1)).unwrap().tier, "proved");
    assert_eq!(store.obligation(key(2)).unwrap().tier, "property");
    assert_eq!(store.obligation(key(3)).unwrap().tier, "example");
}

#[test]
fn a_file_written_by_another_prover_is_discarded_whole() {
    let dir = TempRoot::new("t");
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put_obligation(key(1), proof());
        store.flush().unwrap();
    }
    let text = std::fs::read_to_string(obligations_path(dir.path())).unwrap();
    assert!(text.contains(PROVER_VERSION));
    std::fs::write(
        obligations_path(dir.path()),
        text.replace(PROVER_VERSION, "0.0.1-other"),
    )
    .unwrap();

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.obligation(key(1)), None);
    assert!(
        store
            .warnings()
            .iter()
            .any(|w| w.code == ply_span::codes::CACHE_VERSION_CHANGED),
        "a prover-version mismatch has to be reported, not silently re-proved"
    );
}

/// The budget `Store::open` has predates all three lazily-read files.
#[test]
fn neither_file_is_read_at_open() {
    let dir = TempRoot::new("t");
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put_obligation(key(1), proof());
        store.put_review_record(Symbol::new("m.f"), ReviewRecord::new(key(9), [key(8)]));
        store.flush().unwrap();
    }
    std::fs::write(obligations_path(dir.path()), "{ not json").unwrap();
    std::fs::write(reviews_path(dir.path()), "{ not json").unwrap();

    let store = Store::open(dir.path()).unwrap();
    assert!(
        store.warnings().is_empty(),
        "opening the store must not read either file: {:?}",
        store.warnings()
    );

    assert_eq!(store.obligation(key(1)), None);
    assert_eq!(store.review_record(&Symbol::new("m.f")), None);
    assert_eq!(
        store.warnings().len(),
        2,
        "each file reports once it is read"
    );
}

#[test]
fn a_review_record_is_keyed_by_name_and_sorts_its_specs() {
    let dir = TempRoot::new("t");
    let record = ReviewRecord::new(key(1), [key(9), key(3), key(9)]);
    assert_eq!(record.specs, vec![key(3), key(9)]);
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put_review_record(Symbol::new("ledger.withdraw"), record.clone());
        store.flush().unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store.review_record(&Symbol::new("ledger.withdraw")),
        Some(&record)
    );
    assert_eq!(store.review_record(&Symbol::new("ledger.deposit")), None);
    assert_eq!(store.review_records_len(), 1);
}

/// `ply cache clear` means "prove everything again".
#[test]
fn clearing_the_cache_discards_the_obligations_and_keeps_the_review_baseline() {
    let dir = TempRoot::new("t");
    let mut store = Store::open(dir.path()).unwrap();
    store.put_obligation(key(1), proof());
    store.put_review_record(Symbol::new("m.f"), ReviewRecord::new(key(9), [key(8)]));
    store.flush().unwrap();

    store.clear().unwrap();
    assert_eq!(store.obligation(key(1)), None);
    assert!(!obligations_path(dir.path()).exists());

    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(reopened.obligation(key(1)), None);
    assert_eq!(
        reopened.review_record(&Symbol::new("m.f")),
        Some(&ReviewRecord::new(key(9), [key(8)]))
    );
}

/// Two runs must not discard each other's work, so a flush merges rather than replaces — the same
/// rule the result cache follows, under the same lock.
#[test]
fn a_flush_merges_with_what_another_run_wrote() {
    let dir = TempRoot::new("t");
    let mut first = Store::open(dir.path()).unwrap();
    first.put_obligation(key(1), proof());

    let mut second = Store::open(dir.path()).unwrap();
    second.put_obligation(key(2), sample(200));
    second.flush().unwrap();

    first.flush().unwrap();

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.obligation(key(1)), Some(&proof()));
    assert_eq!(store.obligation(key(2)), Some(&sample(200)));
}

/// The reason there are two version constants: a prover that learns a new rule must upgrade a tier
/// without invalidating a single test result, and a change to evaluation must invalidate results
/// without touching a proof that never ran a program.
#[test]
fn discarding_the_obligations_leaves_every_test_result_where_it_was() {
    let dir = TempRoot::new("t");
    {
        let mut store = Store::open(dir.path()).unwrap();
        store.put(key(5), ply_store::Outcome::Pass);
        store.observe_definitions([key(6)]);
        store.put_obligation(key(1), proof());
        store.flush().unwrap();
    }
    let text = std::fs::read_to_string(obligations_path(dir.path())).unwrap();
    std::fs::write(
        obligations_path(dir.path()),
        text.replace(PROVER_VERSION, "9.9.9"),
    )
    .unwrap();

    let store = Store::open(dir.path()).unwrap();
    assert!(store.obligation(key(1)).is_none(), "the prover moved");
    assert!(
        store.get(key(5)).is_some_and(|o| o.is_pass()),
        "no test may re-run because the prover moved"
    );
    assert!(store.knows_definition(key(6)));
}

/// A run that answered every question from the cache has nothing to write, and must not rewrite the
/// file to say so.
#[test]
fn re_recording_what_is_already_stored_is_not_a_write() {
    let dir = TempRoot::new("t");
    let mut store = Store::open(dir.path()).unwrap();
    store.put_obligation(key(1), proof());
    store.flush().unwrap();
    let written = std::fs::metadata(obligations_path(dir.path()))
        .unwrap()
        .modified()
        .unwrap();

    let mut again = Store::open(dir.path()).unwrap();
    again.put_obligation(key(1), proof());
    again.flush().unwrap();
    assert_eq!(
        std::fs::metadata(obligations_path(dir.path()))
            .unwrap()
            .modified()
            .unwrap(),
        written
    );
}
