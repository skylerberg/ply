//! The obligation cache, coverage, and the review baseline.

use ply_core::{CheckOutput, DefInfo, Footprint, LawBinder, Scheme, Type};
use ply_hash::{DefHash, HashOutput};
use ply_prove::key::prove_key;
use ply_prove::{
    CaseReport, Certificate, Counterexample, Discharge, Evidence, Gap, Obligation, ObligationKind,
    ProvePlan, Rule, Tier, Vacuity, VacuityKind,
};
use ply_span::{Span, Symbol};
use ply_store::{CachedCases, CachedEvidence, CachedObligation, ReviewRecord, Store};
use ply_syntax::ast::ModuleName;
use ply_test::obligation::{self, Discharger, Laws, Moved, Reason};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// --- Fixtures ---------------------------------------------------------------

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

fn check_of(names: &[&str]) -> CheckOutput {
    let mut check = CheckOutput::default();
    for name in names {
        let (module, simple) = name.rsplit_once('.').unwrap_or(("m", name));
        check.defs.insert(
            Symbol::new(*name),
            DefInfo {
                name: Symbol::new(*name),
                module: ModuleName::from_dotted(module),
                simple_name: Symbol::new(simple),
                scheme: Scheme::mono(Type::int()),
                footprint: Footprint::empty(),
                performed: Footprint::empty(),
                row_aliases: Vec::new(),
                constraints: Vec::new(),
                spec: Vec::new(),
                internally_effectful: true,
                span: Span::DUMMY,
            },
        );
    }
    check
}

fn ensures(key: u8, owner: &str, index: usize) -> Obligation {
    Obligation {
        key: hash(key),
        owner: Symbol::new(owner),
        kind: ObligationKind::Ensures { index },
        span: Span::DUMMY,
        frame: ply_prove::Frame::Pure,
        binders: vec![LawBinder {
            name: Symbol::new("x"),
            ty: Type::int(),
            span: Span::DUMMY,
        }],
        guarded: false,
        host: false,
        footprint: Footprint::empty(),
    }
}

fn law(key: u8, label: &str) -> Obligation {
    Obligation {
        key: hash(key),
        owner: Symbol::new(label),
        kind: ObligationKind::Law,
        span: Span::DUMMY,
        frame: ply_prove::Frame::Pure,
        binders: Vec::new(),
        guarded: false,
        host: false,
        footprint: Footprint::empty(),
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

fn sampled(kept: u32) -> Discharge {
    Discharge::Held(Evidence::Cases(CaseReport {
        generated: kept.max(200),
        kept,
        rejected: kept.max(200) - kept,
        roots: vec![0],
        instantiations: Vec::new(),
    }))
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
    Discharge::Unattempted(Gap::UnhandledEffect(Footprint::empty()))
}

/// A prover whose answers are written down in advance, and which records every obligation it was
/// actually asked about — which is what a cache test is measuring.
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

    /// Sorted, because `obligation::prove` discharges over a `par_iter` and the arrival order is
    /// the thread pool's rather than the program's.
    fn asked(&self) -> Vec<DefHash> {
        let mut asked = self.asked.lock().unwrap().clone();
        asked.sort();
        asked
    }
}

impl Discharger for Scripted {
    fn discharge(&self, obligation: &Obligation, _plan: &ProvePlan) -> Discharge {
        self.asked.lock().unwrap().push(obligation.key);
        match self.answers.get(&obligation.key) {
            Some(Discharge::Held(e)) => Discharge::Held(e.clone()),
            Some(Discharge::Refuted(_)) => refuted(),
            Some(Discharge::Vacuous(_)) => vacuous(),
            _ => unattempted(),
        }
    }
}

fn run(
    obligations: Vec<Obligation>,
    check: &CheckOutput,
    laws: &Laws,
    store: &mut Store,
    plan: &ProvePlan,
    discharger: &Scripted,
) -> ply_prove::ProveReport {
    obligation::prove(obligations, check, laws, store, plan, true, discharger).report
}

// --- The cache --------------------------------------------------------------

#[test]
fn an_obligation_discharges_once_and_stays_discharged() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();

    let first = Scripted::new([(hash(1), proved())]);
    let mut store = dir.store();
    let report = run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        &first,
    );
    assert_eq!(
        first.asked(),
        vec![hash(1)],
        "the first run has to do the work"
    );
    assert_eq!(report.count(Tier::Proved), 1);
    assert_eq!(report.cached, 0);
    store.flush().unwrap();

    let second = Scripted::new([]);
    let mut store = dir.store();
    let report = run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        &second,
    );
    assert!(
        second.asked().is_empty(),
        "a discharged obligation must not be attempted again"
    );
    assert_eq!(report.cached, 1);
    assert_eq!(report.count(Tier::Proved), 1);
}

/// A cached `proved` and a cached `property` are different claims.
#[test]
fn every_tier_survives_a_reload_as_itself() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f", "m.g", "m.h"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();
    let obligations = || {
        vec![
            ensures(1, "m.f", 0),
            ensures(2, "m.g", 0),
            ensures(3, "m.h", 0),
        ]
    };

    let first = Scripted::new([
        (hash(1), proved()),
        (hash(2), sampled(200)),
        (hash(3), sampled(7)),
    ]);
    let mut store = dir.store();
    let before = run(obligations(), &check, &laws, &mut store, &plan, &first);
    store.flush().unwrap();

    let second = Scripted::new([]);
    let mut store = dir.store();
    let after = run(obligations(), &check, &laws, &mut store, &plan, &second);

    assert!(second.asked().is_empty());
    let tiers = |report: &ply_prove::ProveReport| -> Vec<Option<Tier>> {
        report.obligations.iter().map(|(_, d)| d.tier()).collect()
    };
    assert_eq!(
        tiers(&after),
        vec![
            Some(Tier::Proved),
            Some(Tier::Property),
            Some(Tier::Example)
        ]
    );
    assert_eq!(tiers(&before), tiers(&after));
}

/// The asymmetry the whole operational value of `proved` rests on.
#[test]
fn widening_the_plan_re_runs_the_samples_and_none_of_the_proofs() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f", "m.g"]);
    let laws = Laws::default();
    let narrow = ProvePlan::default();
    let wide = ProvePlan {
        cases: narrow.cases * 4,
        ..narrow.clone()
    };
    let obligations = || vec![ensures(1, "m.f", 0), ensures(2, "m.g", 0)];

    let first = Scripted::new([(hash(1), proved()), (hash(2), sampled(200))]);
    let mut store = dir.store();
    run(obligations(), &check, &laws, &mut store, &narrow, &first);
    store.flush().unwrap();

    let second = Scripted::new([(hash(2), sampled(800))]);
    let mut store = dir.store();
    let report = run(obligations(), &check, &laws, &mut store, &wide, &second);
    assert_eq!(
        second.asked(),
        vec![hash(2)],
        "widening must re-open the sample and leave the proof alone"
    );
    assert_eq!(report.cached, 1);
    assert_eq!(report.count(Tier::Proved), 1);
    assert_eq!(report.count(Tier::Property), 1);
}

/// The rule whose absence is silent: a sampled discharge under the bare key would let
/// `--prove-cases 10` satisfy a run that asked for a thousand.
#[test]
fn a_sample_is_never_written_under_the_bare_key() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();

    let mut store = dir.store();
    let scripted = Scripted::new([(hash(1), sampled(200))]);
    run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        &scripted,
    );

    assert!(
        store.obligation(hash(1)).is_none(),
        "the bare key belongs to proofs alone"
    );
    assert!(store.obligation(prove_key(hash(1), &plan)).is_some());
}

#[test]
fn a_refutation_a_vacuity_and_a_gap_are_never_cached() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f", "m.g", "m.h"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();
    let obligations = || {
        vec![
            ensures(1, "m.f", 0),
            ensures(2, "m.g", 0),
            ensures(3, "m.h", 0),
        ]
    };

    let first = Scripted::new([
        (hash(1), refuted()),
        (hash(2), vacuous()),
        (hash(3), unattempted()),
    ]);
    let mut store = dir.store();
    run(obligations(), &check, &laws, &mut store, &plan, &first);
    store.flush().unwrap();
    assert_eq!(store.obligations_len(), 0);

    let second = Scripted::new([]);
    let mut store = dir.store();
    run(obligations(), &check, &laws, &mut store, &plan, &second);
    assert_eq!(
        second.asked(),
        vec![hash(1), hash(2), hash(3)],
        "a red obligation re-runs until it goes green"
    );
}

/// The permissive-direction failure, and the one that must not ship: an obligation's key covers its
/// owner's hash, so rewriting the implementation moves the key and the discharged claim does not
/// follow it.
#[test]
fn editing_the_implementation_moves_the_key_and_re_opens_the_obligation() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();

    let first = Scripted::new([(hash(1), proved())]);
    let mut store = dir.store();
    run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        &first,
    );
    store.flush().unwrap();

    // The same clause on the same definition, after the body changed: the key is a function of the
    // owner's hash, so it is a different key.
    let second = Scripted::new([(hash(2), sampled(200))]);
    let mut store = dir.store();
    let report = run(
        vec![ensures(2, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        &second,
    );
    assert_eq!(second.asked(), vec![hash(2)]);
    assert_eq!(report.cached, 0);
    assert_eq!(report.count(Tier::Proved), 0);
}

/// A file whose label and whose evidence tell different stories is not evidence of either.
#[test]
fn an_entry_whose_label_disagrees_with_its_evidence_is_refused() {
    let dir = TempRoot::new();
    let plan = ProvePlan::default();
    let mut store = dir.store();
    store.put_obligation(
        hash(1),
        CachedObligation {
            tier: "proved".to_string(),
            evidence: CachedEvidence::Cases(CachedCases {
                generated: 200,
                kept: 200,
                rejected: 0,
                roots: vec![0],
                instantiations: Vec::new(),
            }),
        },
    );

    let answer = obligation::lookup(&store, hash(1), &plan);
    assert_eq!(answer.reason, Reason::Refused);
    assert!(answer.evidence.is_none());
    assert!(answer.warning.is_some());
}

#[test]
fn a_proof_written_under_a_plan_key_is_refused_rather_than_believed() {
    let dir = TempRoot::new();
    let plan = ProvePlan::default();
    let mut store = dir.store();
    store.put_obligation(
        prove_key(hash(1), &plan),
        obligation::to_cached(&Evidence::Proof(certificate())),
    );

    let answer = obligation::lookup(&store, hash(1), &plan);
    assert_eq!(answer.reason, Reason::Refused);
    assert!(answer.evidence.is_none());
}

/// A certificate that did not establish its guard has a domain it cannot vouch for.
#[test]
fn a_proof_that_did_not_establish_its_guard_is_refused() {
    let dir = TempRoot::new();
    let plan = ProvePlan::default();
    let mut store = dir.store();
    let mut entry = obligation::to_cached(&Evidence::Proof(certificate()));
    if let CachedEvidence::Proof(c) = &mut entry.evidence {
        c.guard_satisfiable = false;
    }
    store.put_obligation(hash(1), entry);

    assert_eq!(
        obligation::lookup(&store, hash(1), &plan).reason,
        Reason::Refused
    );
}

/// A refusal is a warning and a re-discharge, never a silent read: a cache that quietly declines to
/// answer looks exactly like a prover that is slow for no reason, and nobody investigates that.
#[test]
fn a_refused_entry_reaches_the_caller_and_the_obligation_is_attempted_again() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();

    let mut store = dir.store();
    store.put_obligation(
        hash(1),
        CachedObligation {
            tier: "proved".to_string(),
            evidence: CachedEvidence::Cases(CachedCases {
                generated: 200,
                kept: 200,
                rejected: 0,
                roots: vec![0],
                instantiations: Vec::new(),
            }),
        },
    );

    let scripted = Scripted::new([(hash(1), sampled(200))]);
    let proved = obligation::prove(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        true,
        &scripted,
    );
    assert_eq!(scripted.asked(), vec![hash(1)]);
    assert_eq!(proved.reasons, vec![Reason::Refused]);
    assert_eq!(proved.report.cached, 0);
    assert!(
        proved
            .warnings
            .iter()
            .any(|w| w.code == ply_span::codes::CACHE_CORRUPT),
        "a refusal has to be reported: {:?}",
        proved.warnings
    );
    assert_eq!(proved.report.count(Tier::Property), 1);
}

/// The plan key is an identity, not an ordering: a sample taken under a *wider* plan is not read by
/// a narrower run either.
#[test]
fn narrowing_the_plan_re_opens_a_sample_and_leaves_a_proof_alone() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f", "m.g"]);
    let laws = Laws::default();
    let wide = ProvePlan {
        cases: 800,
        roots: vec![0, 1, 2, 3],
        ..ProvePlan::default()
    };
    let narrow = ProvePlan::default();
    let obligations = || vec![ensures(1, "m.f", 0), ensures(2, "m.g", 0)];

    let first = Scripted::new([(hash(1), proved()), (hash(2), sampled(800))]);
    let mut store = dir.store();
    run(obligations(), &check, &laws, &mut store, &wide, &first);
    store.flush().unwrap();

    let second = Scripted::new([(hash(2), sampled(200))]);
    let mut store = dir.store();
    let report = run(obligations(), &check, &laws, &mut store, &narrow, &second);
    assert_eq!(
        second.asked(),
        vec![hash(2)],
        "a sample keyed by another plan is not this run's evidence"
    );
    assert_eq!(report.count(Tier::Proved), 1);
}

/// The root set is part of the plan, so re-spelling it is one key and widening it is another.
#[test]
fn a_sample_is_keyed_by_the_root_set_as_well_as_the_case_count() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f"]);
    let laws = Laws::default();
    let one_root = ProvePlan::default();
    let respelled = ProvePlan {
        roots: vec![0, 0],
        ..ProvePlan::default()
    };
    let two_roots = ProvePlan {
        roots: vec![0, 1],
        ..ProvePlan::default()
    };

    let first = Scripted::new([(hash(1), sampled(200))]);
    let mut store = dir.store();
    run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &one_root,
        &first,
    );
    store.flush().unwrap();

    let same = Scripted::new([]);
    let mut store = dir.store();
    run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &respelled,
        &same,
    );
    assert!(same.asked().is_empty(), "two spellings of one plan are one");

    let wider = Scripted::new([(hash(1), sampled(400))]);
    let mut store = dir.store();
    run(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &two_roots,
        &wider,
    );
    assert_eq!(wider.asked(), vec![hash(1)]);
}

/// The discharger that decides nothing.
#[test]
fn a_run_that_decides_nothing_claims_nothing_and_caches_nothing() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f", "m.g"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();

    let mut store = dir.store();
    let proved = obligation::prove(
        vec![ensures(1, "m.f", 0), ensures(2, "m.g", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        true,
        &ply_test::obligation::Undecided,
    );
    assert_eq!(proved.report.unattempted(), 2);
    assert_eq!(proved.report.count(Tier::Proved), 0);
    assert_eq!(proved.report.coverage.covered, 0);
    assert_eq!(proved.report.coverage.uncovered.len(), 2);
    assert!(!proved.report.failed(), "a gap is not a failure");
    store.flush().unwrap();
    assert_eq!(store.obligations_len(), 0);
}

#[test]
fn no_cache_reads_nothing_and_writes_nothing() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f"]);
    let laws = Laws::default();
    let plan = ProvePlan::default();

    let mut store = dir.store();
    let scripted = Scripted::new([(hash(1), proved())]);
    let report = obligation::prove(
        vec![ensures(1, "m.f", 0)],
        &check,
        &laws,
        &mut store,
        &plan,
        false,
        &scripted,
    )
    .report;
    assert_eq!(scripted.asked(), vec![hash(1)]);
    assert_eq!(report.cached, 0);
    assert_eq!(store.obligations_len(), 0);
}

// --- Coverage ---------------------------------------------------------------

fn coverage_of(
    names: &[&str],
    laws: &Laws,
    results: Vec<(Obligation, Discharge)>,
) -> ply_prove::Coverage {
    obligation::coverage(&check_of(names), laws, &results)
}

#[test]
fn only_an_obligation_that_holds_covers_its_definition() {
    let coverage = coverage_of(
        &["m.f", "m.g", "m.h", "m.i"],
        &Laws::default(),
        vec![
            (ensures(1, "m.f", 0), proved()),
            (ensures(2, "m.g", 0), refuted()),
            (ensures(3, "m.h", 0), unattempted()),
        ],
    );
    assert_eq!(coverage.definitions, 4);
    assert_eq!(coverage.covered, 1);
    assert_eq!(
        coverage.uncovered,
        vec![Symbol::new("m.g"), Symbol::new("m.h"), Symbol::new("m.i")],
        "a claim the machine could not establish is not evidence"
    );
}

/// A definition is covered at the strongest tier that holds of it, and the per-tier counts add up
/// to the covered count — otherwise the two numbers on the coverage line are measuring different
/// things.
#[test]
fn coverage_counts_a_definition_at_its_strongest_holding_tier() {
    let coverage = coverage_of(
        &["m.f", "m.g"],
        &Laws::default(),
        vec![
            (ensures(1, "m.f", 0), sampled(7)),
            (ensures(2, "m.f", 1), proved()),
            (ensures(3, "m.g", 0), sampled(200)),
        ],
    );
    assert_eq!(coverage.covered, 2);
    assert_eq!(coverage.by_tier.get(&Tier::Proved), Some(&1));
    assert_eq!(coverage.by_tier.get(&Tier::Property), Some(&1));
    assert_eq!(coverage.by_tier.get(&Tier::Example), None);
    assert_eq!(
        coverage.by_tier.values().sum::<usize>(),
        coverage.covered,
        "every covered definition is counted at exactly one tier"
    );
}

/// A law covers what it names and nothing else.
#[test]
fn a_law_covers_the_definitions_it_names_directly_and_no_others() {
    let mut laws = Laws::default();
    laws.insert(
        Symbol::new("m.cancel"),
        hash(9),
        hash(9),
        [Symbol::new("m.credited"), Symbol::new("m.debited")],
    );
    let coverage = coverage_of(
        &["m.credited", "m.debited", "m.helper"],
        &laws,
        vec![(law(9, "m.cancel"), proved())],
    );
    assert_eq!(coverage.covered, 2);
    assert_eq!(coverage.uncovered, vec![Symbol::new("m.helper")]);
}

#[test]
fn a_law_that_does_not_hold_covers_nothing() {
    let mut laws = Laws::default();
    laws.insert(
        Symbol::new("m.cancel"),
        hash(9),
        hash(9),
        [Symbol::new("m.credited")],
    );
    let coverage = coverage_of(
        &["m.credited"],
        &laws,
        vec![(law(9, "m.cancel"), refuted())],
    );
    assert_eq!(coverage.covered, 0);
    assert_eq!(coverage.uncovered, vec![Symbol::new("m.credited")]);
}

/// A `requires` is a filter on the domain of the `ensures` clauses beside it, not a claim about
/// behaviour — so it is not an obligation at all, and a definition carrying only preconditions is
/// one a reviewer still has to read.
#[test]
fn a_definition_with_no_obligation_is_never_covered() {
    let coverage = coverage_of(&["m.f"], &Laws::default(), Vec::new());
    assert_eq!(coverage.covered, 0);
    assert_eq!(coverage.uncovered, vec![Symbol::new("m.f")]);
    assert_eq!(coverage.uncovered_count(), 1);
}

// --- Review -----------------------------------------------------------------

/// `specs` and `laws` are the claims *as written*.
fn hashes_of(defs: &[(&str, u8)], specs: &[(&str, Vec<u8>)], laws: &[u8]) -> HashOutput {
    let mut out = HashOutput::default();
    for (name, byte) in defs {
        out.defs.insert(Symbol::new(*name), hash(*byte));
    }
    for (name, bytes) in specs {
        let hashes: Vec<_> = bytes.iter().copied().map(hash).collect();
        out.specs.insert(Symbol::new(*name), hashes.clone());
        out.spec_texts.insert(Symbol::new(*name), hashes);
    }
    out.laws = laws.iter().copied().map(hash).collect();
    out.law_texts = out.laws.clone();
    out
}

fn review_after_accept(
    before: &HashOutput,
    after: &HashOutput,
    names: &[&str],
    laws_before: &Laws,
    laws_after: &Laws,
    results: Vec<(Obligation, Discharge)>,
) -> ply_test::obligation::ReviewReport {
    let dir = TempRoot::new();
    let check = check_of(names);
    let mut store = dir.store();
    obligation::accept(&check, before, laws_before, &mut store);

    let report = ply_prove::ProveReport {
        coverage: obligation::coverage(&check, laws_after, &results),
        obligations: results,
        plan: ProvePlan::default(),
        cached: 0,
        duration: std::time::Duration::ZERO,
    };
    obligation::review(&check, after, laws_after, &store, &report)
}

/// The cheapest review in the system, and the row the milestone exists for: the claim is fixed and
/// still holds, so the diff is an implementation detail.
#[test]
fn a_changed_body_under_an_unchanged_spec_reports_the_obligations() {
    let before = hashes_of(&[("m.f", 1)], &[("m.f", vec![7])], &[]);
    let after = hashes_of(&[("m.f", 2)], &[("m.f", vec![7])], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.f"],
        &Laws::default(),
        &Laws::default(),
        vec![(ensures(7, "m.f", 0), proved())],
    );

    assert_eq!(review.changed.len(), 1);
    let entry = &review.changed[0];
    assert_eq!(entry.implementation, Moved::Changed);
    assert_eq!(entry.spec, Moved::Unchanged);
    assert!(entry.specified());
    assert_eq!(review.broken, 0);
    assert_eq!(review.unspecified(), 0);
    assert!(
        review.headline().contains("no specified behaviour changed"),
        "{}",
        review.headline()
    );
}

#[test]
fn an_unchanged_body_under_a_changed_spec_reports_the_spec_diff() {
    let before = hashes_of(&[("m.f", 1)], &[("m.f", vec![7])], &[]);
    let after = hashes_of(&[("m.f", 1)], &[("m.f", vec![8])], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.f"],
        &Laws::default(),
        &Laws::default(),
        vec![(ensures(8, "m.f", 0), sampled(200))],
    );
    let entry = &review.changed[0];
    assert_eq!(entry.implementation, Moved::Unchanged);
    assert_eq!(entry.spec, Moved::Changed);
}

#[test]
fn an_unchanged_definition_is_not_reported_at_all() {
    let hashes = hashes_of(&[("m.f", 1)], &[("m.f", vec![7])], &[]);
    let review = review_after_accept(
        &hashes,
        &hashes,
        &["m.f"],
        &Laws::default(),
        &Laws::default(),
        vec![(ensures(7, "m.f", 0), proved())],
    );
    assert!(review.changed.is_empty());
    assert_eq!(review.reviewed, 1);
    assert_eq!(
        review.headline(),
        "no definition changed since the last accepted review"
    );
}

/// A law is part of the specification of every definition it names, so editing one has to read as a
/// spec change on each of them.
#[test]
fn editing_a_law_is_a_spec_change_on_every_definition_it_names() {
    let mut before_laws = Laws::default();
    before_laws.insert(
        Symbol::new("m.cancel"),
        hash(9),
        hash(9),
        [Symbol::new("m.f")],
    );
    let mut after_laws = Laws::default();
    after_laws.insert(
        Symbol::new("m.cancel"),
        hash(10),
        hash(10),
        [Symbol::new("m.f")],
    );

    let hashes = hashes_of(&[("m.f", 1)], &[], &[9]);
    let after = hashes_of(&[("m.f", 1)], &[], &[10]);
    let review = review_after_accept(
        &hashes,
        &after,
        &["m.f"],
        &before_laws,
        &after_laws,
        vec![(law(10, "m.cancel"), proved())],
    );
    assert_eq!(review.changed.len(), 1);
    assert_eq!(review.changed[0].implementation, Moved::Unchanged);
    assert_eq!(review.changed[0].spec, Moved::Changed);
}

/// Renaming a definition loses its baseline, which costs one re-read and never a false "unchanged".
#[test]
fn a_definition_with_no_baseline_is_unreviewed_rather_than_unchanged() {
    let before = hashes_of(&[("m.old", 1)], &[], &[]);
    let after = hashes_of(&[("m.new", 1)], &[], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.new"],
        &Laws::default(),
        &Laws::default(),
        Vec::new(),
    );
    assert_eq!(review.changed.len(), 1);
    assert_eq!(review.changed[0].implementation, Moved::Never);
    assert_eq!(review.reviewed, 0);
}

/// The sentence this command must not get wrong.
#[test]
fn the_headline_never_claims_more_than_the_specifications_cover() {
    let before = hashes_of(&[("m.f", 1), ("m.g", 3)], &[("m.f", vec![7])], &[]);
    let after = hashes_of(&[("m.f", 2), ("m.g", 4)], &[("m.f", vec![7])], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.f", "m.g"],
        &Laws::default(),
        &Laws::default(),
        vec![(ensures(7, "m.f", 0), proved())],
    );

    assert_eq!(review.changed.len(), 2);
    assert_eq!(review.specified(), 1);
    assert_eq!(review.unspecified(), 1);
    let headline = review.headline();
    assert!(
        headline.contains("no specified behaviour changed"),
        "{headline}"
    );
    assert!(
        headline.contains("1 of 2 carry no obligation"),
        "the limit has to be visible at the point of use: {headline}"
    );
    assert!(
        !headline.contains("nothing changed"),
        "a changed unspecified definition did change: {headline}"
    );
}

#[test]
fn a_changed_definition_whose_obligation_broke_says_so() {
    let before = hashes_of(&[("m.f", 1)], &[("m.f", vec![7])], &[]);
    let after = hashes_of(&[("m.f", 2)], &[("m.f", vec![7])], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.f"],
        &Laws::default(),
        &Laws::default(),
        vec![(ensures(7, "m.f", 0), refuted())],
    );
    assert_eq!(review.broken, 1);
    assert!(
        review.headline().contains("no longer hold"),
        "{}",
        review.headline()
    );
}

/// A law is labelled per module and hashed by what it claims, so moving one between modules changes
/// no hash.
#[test]
fn a_law_moved_between_modules_leaves_every_baseline_where_it_was() {
    let mut before_laws = Laws::default();
    before_laws.insert(
        Symbol::new("a.cancel"),
        hash(9),
        hash(9),
        [Symbol::new("m.f")],
    );
    let mut after_laws = Laws::default();
    after_laws.insert(
        Symbol::new("b.cancel"),
        hash(9),
        hash(9),
        [Symbol::new("m.f")],
    );

    let hashes = hashes_of(&[("m.f", 1)], &[], &[9]);
    let review = review_after_accept(
        &hashes,
        &hashes,
        &["m.f"],
        &before_laws,
        &after_laws,
        vec![(law(9, "b.cancel"), proved())],
    );
    assert!(review.changed.is_empty(), "{:?}", review.changed);
    assert_eq!(review.coverage.covered, 1);
}

/// Deleting a law is a change to the specification of everything it spoke about, in the same way
/// editing one is — otherwise a claim could be withdrawn while every definition it constrained
/// reported "spec unchanged".
#[test]
fn deleting_a_law_reads_as_a_spec_change_on_what_it_named() {
    let mut before_laws = Laws::default();
    before_laws.insert(
        Symbol::new("m.cancel"),
        hash(9),
        hash(9),
        [Symbol::new("m.f")],
    );

    let before = hashes_of(&[("m.f", 1)], &[], &[9]);
    let after = hashes_of(&[("m.f", 1)], &[], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.f"],
        &before_laws,
        &Laws::default(),
        Vec::new(),
    );
    assert_eq!(review.changed.len(), 1);
    assert_eq!(review.changed[0].implementation, Moved::Unchanged);
    assert_eq!(review.changed[0].spec, Moved::Changed);
    assert_eq!(review.coverage.covered, 0);
}

/// A claim the machine could not attempt is not evidence about anything.
#[test]
fn a_changed_definition_whose_only_obligation_is_a_gap_gains_no_evidence() {
    let before = hashes_of(&[("m.f", 1)], &[("m.f", vec![7])], &[]);
    let after = hashes_of(&[("m.f", 2)], &[("m.f", vec![7])], &[]);
    let review = review_after_accept(
        &before,
        &after,
        &["m.f"],
        &Laws::default(),
        &Laws::default(),
        vec![(ensures(7, "m.f", 0), unattempted())],
    );

    assert_eq!(review.changed.len(), 1);
    assert_eq!(review.changed[0].implementation, Moved::Changed);
    assert_eq!(review.changed[0].spec, Moved::Unchanged);
    assert_eq!(review.coverage.covered, 0);
    assert_eq!(review.coverage.uncovered, vec![Symbol::new("m.f")]);
    assert_eq!(
        review.broken, 1,
        "an obligation nothing established must not read as one that held"
    );
    assert_eq!(review.undischarged, 1);
    assert!(
        !review.headline().contains("no specified behaviour changed"),
        "{}",
        review.headline()
    );
    assert!(
        !review.headline().contains("no longer hold"),
        "nothing established it, so nothing stopped holding: {}",
        review.headline()
    );

    // The count `ply review --changed` derives its advice from.
    assert!(
        !review.changed[0].specified(),
        "an undischarged obligation is not a specification that holds"
    );
    assert!(
        review.changed[0].claimed(),
        "it does carry a clause, and the report must not say it carries none"
    );
    assert_eq!(review.specified(), 0);
    assert_eq!(review.unspecified(), 1);
}

#[test]
fn accepting_records_one_baseline_per_definition_keyed_by_name() {
    let dir = TempRoot::new();
    let check = check_of(&["m.f", "m.g"]);
    let hashes = hashes_of(&[("m.f", 1), ("m.g", 2)], &[("m.f", vec![7])], &[]);
    let mut store = dir.store();
    assert_eq!(
        obligation::accept(&check, &hashes, &Laws::default(), &mut store),
        2
    );
    assert_eq!(
        store.review_record(&Symbol::new("m.f")),
        Some(&ReviewRecord::new(hash(1), [hash(7)]))
    );
    assert_eq!(
        store.review_record(&Symbol::new("m.g")),
        Some(&ReviewRecord::new(hash(2), []))
    );
}
