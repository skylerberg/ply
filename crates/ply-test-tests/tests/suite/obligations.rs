use ply_eval::DefHash;
use ply_prove::{CaseReport, Certificate, Evidence, Rule, Tier};
use ply_store::{CachedCases, CachedEvidence, CachedObligation, Store};
use ply_test::obligation::{from_cached, to_cached};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ply-obligation-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempRoot(dir)
    }

    fn store(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hash(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

fn certificate() -> Certificate {
    Certificate {
        rules: vec![Rule::LinearArithmetic],
        steps: 12,
        guard_satisfiable: true,
        sorts: Vec::new(),
    }
}

fn cases(kept: u32) -> CaseReport {
    CaseReport {
        generated: kept.max(200),
        kept,
        rejected: kept.max(200) - kept,
        roots: vec![0],
        instantiations: Vec::new(),
    }
}

#[test]
fn every_tier_survives_a_reload_as_itself() {
    let dir = TempRoot::new();
    let mut store = dir.store();
    let evidence = [
        Evidence::Proof(certificate()),
        Evidence::Cases(cases(200)),
        Evidence::Cases(cases(7)),
    ];
    for (i, e) in evidence.iter().enumerate() {
        store.put_obligation(hash(i as u8), to_cached(e));
    }
    store.flush().unwrap();

    let store = dir.store();
    let tiers: Vec<Tier> = (0..evidence.len())
        .map(|i| {
            from_cached(&store.obligation(hash(i as u8)).expect("filed"))
                .expect("readable")
                .tier()
        })
        .collect();
    assert_eq!(tiers, vec![Tier::Proved, Tier::Property, Tier::Example]);
}

#[test]
fn an_entry_whose_label_disagrees_with_its_evidence_cannot_be_read() {
    let entry = CachedObligation {
        tier: "proved".to_string(),
        evidence: CachedEvidence::Cases(CachedCases {
            generated: 200,
            kept: 200,
            rejected: 0,
            roots: vec![0],
            instantiations: Vec::new(),
        }),
    };
    assert!(from_cached(&entry).is_err());
}

#[test]
fn a_proof_that_did_not_establish_its_guard_cannot_be_read() {
    let mut entry = to_cached(&Evidence::Proof(certificate()));
    if let CachedEvidence::Proof(c) = &mut entry.evidence {
        c.guard_satisfiable = false;
    }
    assert!(from_cached(&entry).is_err());
}
