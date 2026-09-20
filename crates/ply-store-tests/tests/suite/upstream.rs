use ply_span::Symbol;
use ply_store::{
    CachedCertificate, CachedEvidence, CachedObligation, CachedRule, Outcome, Store, Upstream,
};
use ply_ty::DefHash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ply-upstream-{}-{tag}-{n}", std::process::id()));
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
            rules: vec![CachedRule::LinearArithmetic],
            steps: 3,
            guard_satisfiable: true,
            sorts: vec![Symbol::new("a")],
        }),
    }
}

fn failure() -> Outcome {
    Outcome::Fail {
        message: "not this".to_string(),
        diagnostic: None,
    }
}

#[test]
fn a_pass_recorded_in_one_checkout_is_a_pass_in_another_through_the_upstream() {
    let shared = TempRoot::new("shared");
    let first = TempRoot::new("first");
    let second = TempRoot::new("second");
    {
        let mut store = Store::open(first.path())
            .unwrap()
            .with_upstream(Some(Upstream::at(shared.path(), true)));
        store.put(key(1), Outcome::Pass);
        store.put(key(2), failure());
        store.put_obligation(key(3), proof());
        store.flush().unwrap();
        assert!(store.take_warnings().is_empty());
    }
    let mut store = Store::open(second.path())
        .unwrap()
        .with_upstream(Some(Upstream::at(shared.path(), true)));
    assert_eq!(store.get(key(1)), Some(Outcome::Pass));
    assert_eq!(store.get(key(2)), None, "a failure is never published");
    assert_eq!(store.obligation(key(3)), Some(proof()));
    assert_eq!(store.obligation(key(4)), None);

    // What was read in is the local cache's own from then on.
    store.flush().unwrap();
    let plain = Store::open(second.path()).unwrap();
    assert_eq!(plain.get(key(1)), Some(Outcome::Pass));
}

#[test]
fn a_read_only_upstream_is_read_and_never_written() {
    let shared = TempRoot::new("readonly");
    let mine = TempRoot::new("mine");
    let mut store = Store::open(mine.path())
        .unwrap()
        .with_upstream(Some(Upstream::at(shared.path(), false)));
    store.put(key(5), Outcome::Pass);
    store.flush().unwrap();
    assert!(store.take_warnings().is_empty());
    assert_eq!(
        std::fs::read_dir(shared.path())
            .map(|d| d.count())
            .unwrap_or(0),
        0,
        "nothing lands in a read-only upstream"
    );
}

#[test]
fn an_upstream_that_cannot_be_written_is_a_warning_and_not_a_failure() {
    let mine = TempRoot::new("unwritable");
    let file = mine.path().join("not-a-directory");
    std::fs::write(&file, b"").unwrap();
    let mut store = Store::open(mine.path())
        .unwrap()
        .with_upstream(Some(Upstream::at(&file, true)));
    store.put(key(6), Outcome::Pass);
    store.flush().unwrap();
    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].message.contains("upstream"),
        "{}",
        warnings[0].message
    );
    assert_eq!(
        Store::open(mine.path()).unwrap().get(key(6)),
        Some(Outcome::Pass)
    );
}
