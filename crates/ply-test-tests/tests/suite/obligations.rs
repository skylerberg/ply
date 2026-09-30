use ply_eval::{DefHash, Span, Symbol};
use ply_prove::{
    Binder, CaseReport, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation,
    ObligationKind, ProvePlan, Rule, Sort, Tier, Vacuity, VacuityKind,
};
use ply_store::{CachedCases, CachedEvidence, CachedObligation, Store};
use ply_test::obligation::{self, Choice, Discharger, from_cached, to_cached};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
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

fn ensures(key: u8, owner: &str, index: usize) -> Obligation {
    Obligation {
        key: hash(key),
        owner: Symbol::new(owner),
        kind: ObligationKind::Ensures { index },
        span: Span::DUMMY,
        binders: vec![Binder::new("x", Sort::int())],
        guarded: false,
        host: false,
        footprint: None,
    }
}

fn certificate() -> Certificate {
    Certificate {
        rules: vec![Rule::LinearArithmetic],
        steps: 12,
        guard_satisfiable: true,
        sorts: Vec::new(),
    }
}

fn proved() -> Discharge {
    Discharge::Held(Evidence::Proof(certificate()))
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

fn refuted() -> Discharge {
    Discharge::Refuted(Counterexample {
        bindings: Vec::new(),
        original: Vec::new(),
        shrinks: 0,
        root: 0,
        case: 0,
        race: None,
        sim_seed: None,
    })
}

fn vacuous() -> Discharge {
    Discharge::Vacuous(Vacuity {
        guard: Span::DUMMY,
        kind: VacuityKind::NoCaseKept { generated: 200 },
    })
}

fn unattempted() -> Discharge {
    Discharge::Unattempted(Gap::UnhandledEffect(None))
}

/// A prover with scripted answers that records every obligation it was asked about.
struct Scripted {
    answers: BTreeMap<DefHash, Discharge>,
    asked: Mutex<Vec<DefHash>>,
}

impl Scripted {
    fn new(answers: impl IntoIterator<Item = (DefHash, Discharge)>) -> Scripted {
        Scripted {
            answers: answers.into_iter().collect(),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// Sorted: a discharge runs over a `par_iter`, so arrival order is the pool's.
    fn asked(&self) -> Vec<DefHash> {
        let mut asked = self.asked.lock().unwrap().clone();
        asked.sort();
        asked
    }
}

impl Discharger for Scripted {
    fn discharge(
        &self,
        obligation: &Obligation,
        _plan: &ProvePlan,
        _domain: Option<&ply_test::obligation::Domain>,
    ) -> Discharge {
        self.asked.lock().unwrap().push(obligation.key);
        match self.answers.get(&obligation.key) {
            Some(Discharge::Held(e)) => Discharge::Held(e.clone()),
            Some(Discharge::Refuted(_)) => refuted(),
            Some(Discharge::Vacuous(_)) => vacuous(),
            _ => unattempted(),
        }
    }
}

/// The program's decision, carried out: every answered obligation's evidence is read back from the
/// key the program named, and only the rest are discharged.
fn carried_out(
    obligations: Vec<Obligation>,
    store: &Store,
    read: Vec<(usize, DefHash)>,
    to_discharge: Vec<usize>,
    discharger: &Scripted,
) -> ply_prove::ProveReport {
    let choice = Choice {
        claims: (0..obligations.len()).collect(),
        domains: Vec::new(),
        to_discharge,
        read,
    };
    obligation::Asked::chosen(obligations, &choice, store, &ProvePlan::default())
        .discharge(discharger)
}

#[test]
fn evidence_is_read_back_from_the_key_the_program_named_and_only_the_rest_is_discharged() {
    let dir = TempRoot::new();
    let mut store = dir.store();
    // A sample, filed under a key of the program's choosing: nothing here encodes one.
    store.put_obligation(hash(40), to_cached(&Evidence::Cases(cases(200))));
    store.flush().unwrap();

    let store = dir.store();
    let scripted = Scripted::new([(hash(2), proved())]);
    let report = carried_out(
        vec![ensures(1, "m.f", 0), ensures(2, "m.g", 0)],
        &store,
        vec![(0, hash(40))],
        vec![1],
        &scripted,
    );
    assert_eq!(
        scripted.asked(),
        vec![hash(2)],
        "an answered obligation is not attempted"
    );
    let tiers: Vec<Option<Tier>> = report.obligations.iter().map(|(_, d)| d.tier()).collect();
    assert_eq!(tiers, vec![Some(Tier::Property), Some(Tier::Proved)]);
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

#[test]
fn only_what_was_asked_about_is_discharged_and_every_outcome_comes_back() {
    let dir = TempRoot::new();
    let store = dir.store();
    let scripted = Scripted::new([
        (hash(1), refuted()),
        (hash(2), vacuous()),
        (hash(3), unattempted()),
    ]);
    let report = carried_out(
        vec![
            ensures(1, "m.f", 0),
            ensures(2, "m.g", 0),
            ensures(3, "m.h", 0),
        ],
        &store,
        Vec::new(),
        vec![0, 1, 2],
        &scripted,
    );
    assert_eq!(scripted.asked(), vec![hash(1), hash(2), hash(3)]);
    assert_eq!(report.refuted(), 1);
    assert_eq!(report.vacuous(), 1);
    assert_eq!(report.unattempted(), 1);
    assert!(
        report.failed(),
        "a refutation and a vacuity fail a run, and a gap does not"
    );
}
