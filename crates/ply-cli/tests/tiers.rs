//! What a tier label claims, checked against what actually happened.
//!
//! **A tier label is a truth claim.** Every prior milestone could produce a
//! wrong answer; only this one can produce a wrong answer wearing a certificate,
//! and a reviewer told an obligation is `proved` stops reading. So this file's
//! job is not to demonstrate reach — it is to go and look for a lie.
//!
//! Two audits do the real work. The **certificate audit** checks that every
//! proof a corpus produces names only fragment rules and established its guard.
//! The **differential tier audit** re-runs every `proved` obligation at the
//! sampled tier, at a plan far wider than the one that ran, and treats a
//! refutation as a defect in Ply. It is the direct analogue of `--engine both`
//! and exists for the same reason: a claim that two mechanisms agree is only
//! worth what the comparison costs.

use ply_cli::engine::Prover;
use ply_cli::load::load;
use ply_cli::obligations;
use ply_prove::{
    Certificate, Discharge, Evidence, Gap, Obligation, ObligationKind, ProvePlan, Rule, Tier,
    UNFOLD_DEPTH,
};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

struct Run {
    results: Vec<(Obligation, Discharge)>,
}

impl Run {
    fn of(path: &Path) -> Run {
        Run::with(path, &ProvePlan::default())
    }

    fn with(path: &Path, plan: &ProvePlan) -> Run {
        let loaded = match load(path) {
            Ok(loaded) => loaded,
            Err(e) => panic!(
                "`{}` did not compile: {:?}",
                path.display(),
                e.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
            ),
        };
        let hashes = loaded.hashes.clone();
        let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
        assert!(
            collected.warnings.is_empty(),
            "an obligation was not collected: {:?}",
            collected
                .warnings
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        );
        let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
        let results = collected
            .obligations
            .into_iter()
            .map(|o| {
                let discharge = prover.discharge_with(&o, plan);
                (o, discharge)
            })
            .collect();
        Run { results }
    }

    fn find(&self, needle: &str) -> &(Obligation, Discharge) {
        self.results
            .iter()
            .find(|(o, _)| o.owner.as_str().contains(needle))
            .unwrap_or_else(|| {
                panic!(
                    "no obligation named `{needle}` among {:?}",
                    self.results
                        .iter()
                        .map(|(o, _)| o.owner.as_str())
                        .collect::<Vec<_>>()
                )
            })
    }

    fn tier(&self, needle: &str) -> Option<Tier> {
        self.find(needle).1.tier()
    }

    fn certificate(&self, needle: &str) -> &Certificate {
        match &self.find(needle).1 {
            Discharge::Held(Evidence::Proof(c)) => c,
            other => panic!("`{needle}` is {other:?} rather than a proof"),
        }
    }
}

/// The corpora every audit below runs over: the examples a reader is meant to
/// learn from, and the fixtures written to be wrong on purpose.
fn corpus() -> Vec<PathBuf> {
    let mut paths = vec![repo("examples")];
    for fixture in [
        "refuted_law.ply",
        "vacuous_law.ply",
        "obligation_not_discharged.ply",
        "concurrency_law_binder.ply",
    ] {
        paths.push(repo("tests/fixtures").join(fixture));
    }
    paths
}

// --- The two audits ---------------------------------------------------------

/// Every rule a certificate names is a fragment rule, every certificate
/// established its guard, and no unfolding went past the declared depth.
///
/// `Rule` being a closed enum means a prover that grew a rule nobody sanctioned
/// stops compiling before it reaches here — this catches the rest.
#[test]
fn the_certificate_audit() {
    let plan = ProvePlan::default();
    let mut proofs = 0;
    for path in corpus() {
        let run = Run::of(&path);
        for (obligation, discharge) in &run.results {
            let Discharge::Held(Evidence::Proof(certificate)) = discharge else {
                continue;
            };
            proofs += 1;
            assert!(
                certificate.guard_satisfiable,
                "`{}` holds a certificate that did not establish its guard",
                obligation.owner
            );
            assert!(
                !certificate.rules.is_empty(),
                "`{}` is proved by no rule at all",
                obligation.owner
            );
            for rule in &certificate.rules {
                match rule {
                    Rule::Unfold { def, depth } => assert!(
                        *depth <= UNFOLD_DEPTH,
                        "`{}` unfolded `{def}` to depth {depth}",
                        obligation.owner
                    ),
                    Rule::ExhaustiveEnumeration { points, .. } => assert!(
                        *points <= ply_prove::ENUMERATION_BOUND,
                        "`{}` claims to have enumerated {points} points",
                        obligation.owner
                    ),
                    Rule::ExhaustiveInterleaving { interleavings } => {
                        assert!(*interleavings > 0);
                        ply_prove::concurrency::audit_interleaving_proof(obligation, certificate)
                            .unwrap_or_else(|why| {
                                panic!("`{}`: {why}", obligation.owner);
                            });
                    }
                    Rule::GroundEvaluation
                    | Rule::LinearArithmetic
                    | Rule::Propositional
                    | Rule::CaseSplit { .. }
                    | Rule::Congruence
                    | Rule::Injectivity => {}
                }
            }
            assert!(
                certificate.steps <= plan.prove_budget.max(ply_prove::ENUMERATION_BOUND as u32),
                "`{}` spent {} steps",
                obligation.owner,
                certificate.steps
            );
        }
    }
    assert!(proofs >= 10, "the corpus produced only {proofs} proofs");
}

/// The audit that would catch a lying prover: every obligation the corpus
/// reports `proved` is re-run at the sampled tier, at 1,000 cases across 8
/// roots. A `proved` obligation a sampled run **refutes or raises at** is a
/// defect in Ply.
///
/// A raise is half the audit and it is the half that was missing. Ply's
/// arithmetic is `checked_*` and its recursion is bounded, so a claim that is
/// valid over ℤ and over total function symbols but wrong about Ply can only
/// ever surface as `Gap::Raised` — `x + 1 > x` does not come back false at
/// `i64::MAX`, it comes back `E0502`. An audit that treated only
/// `Discharge::Refuted` as a defect could not fail on the one thing ADR 0007
/// §5.1(a) named it to catch.
#[test]
fn the_differential_tier_audit() {
    let wide = ProvePlan {
        cases: 1_000,
        roots: (0..8).collect(),
        ..ProvePlan::default()
    };
    let mut audited = 0;
    for path in corpus() {
        let loaded = load(&path).expect("the corpus compiles");
        let hashes = loaded.hashes.clone();
        let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
        let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
        for obligation in &collected.obligations {
            if prover.discharge_with(obligation, &ProvePlan::default()).tier() != Some(Tier::Proved)
            {
                continue;
            }
            audited += 1;
            // A vacuity is not a defect: it is a claim about the *sample*, not
            // about the proof, because a guard the prover showed valid can still
            // reject every drawn tuple and the proved path establishes its own
            // guard.
            if let Some(defect) = disagreement(&prover.resample(obligation, &wide)) {
                panic!("`{}` is reported `proved` and a sampled run {defect} — a defect in Ply",
                    obligation.owner);
            }
        }
    }
    assert!(audited >= 10, "only {audited} proofs were audited");
}

// --- What each rule is for --------------------------------------------------

/// `forall (b: Bool) { b || !b }` is decided by covering its domain, for two
/// evaluations rather than two hundred draws.
#[test]
fn a_finite_domain_is_proved_by_covering_it() {
    let dir = project(
        r#"
law "excluded middle"
  forall (b: Bool) {
    b || !b
  }
"#,
    );
    let run = Run::of(dir.path());
    assert_eq!(run.tier("excluded middle"), Some(Tier::Proved));
    let certificate = run.certificate("excluded middle");
    assert!(
        certificate.rules.iter().any(|r| matches!(
            r,
            Rule::ExhaustiveEnumeration { points: 2, .. } | Rule::Propositional
        )),
        "{certificate:?}"
    );
}

/// A ground claim is the degenerate finite domain: one point, and evaluating it
/// is a decision procedure for it. Reporting `example` here would be reporting
/// the weakest label for the strongest possible evidence.
#[test]
fn a_ground_law_is_proved_rather_than_exemplified() {
    let dir = project(
        r#"
fn stock() -> List<Int> = [3, 1, 2]

law "the stock is three deep" {
  len(stock()) == 3
}
"#,
    );
    let run = Run::of(dir.path());
    assert_eq!(run.tier("the stock is three deep"), Some(Tier::Proved));
    assert!(
        run.certificate("the stock is three deep")
            .rules
            .contains(&Rule::GroundEvaluation)
    );
}

/// A recursive definition is never unfolded, because stopping the unfolding at
/// a general statement is what induction is for and there is none here. So this
/// is `property`, and it should be.
#[test]
fn a_recursive_definition_is_never_unfolded() {
    let dir = project(
        r#"
fn rev_onto(xs: List<Int>, acc: List<Int>) -> List<Int> =
  match xs {
    [x, ..rest] -> rev_onto(rest, push(acc, x)),
    _ -> acc,
  }

fn rev(xs: List<Int>) -> List<Int> = rev_onto(xs, [])

law "reverse is an involution"
  forall (xs: List<Int>) {
    rev(rev(xs)) == xs
  }
"#,
    );
    let run = Run::of(dir.path());
    assert_eq!(
        run.tier("reverse is an involution"),
        Some(Tier::Property),
        "a claim over unbounded data cannot be decided without induction"
    );
}

/// `/` and `%` are outside the fragment entirely, so `x / 2 * 2 == x` — which is
/// false — is not proved. It is the exact defect this milestone must not ship.
#[test]
fn a_term_outside_the_fragment_is_never_proved() {
    let dir = project(
        r#"
law "halving and doubling cancel"
  forall (x: Int) {
    x / 2 * 2 == x
  }
"#,
    );
    let run = Run::of(dir.path());
    assert_ne!(
        run.tier("halving and doubling cancel"),
        Some(Tier::Proved),
        "an uninterpreted `/` must not be reasoned about"
    );
    assert!(matches!(
        run.find("halving and doubling cancel").1,
        Discharge::Refuted(_)
    ));
}

/// An equality that holds for an arbitrary `f` holds for every actual `f`, so
/// this is a genuinely universal proof over an uninterpreted symbol.
#[test]
fn an_uninterpreted_function_closes_under_congruence() {
    let dir = project(
        r#"
law "a pure function is a function"
  forall (f: (Int) -> Int, x: Int) {
    f(x) == f(x)
  }
"#,
    );
    let run = Run::of(dir.path());
    assert_eq!(run.tier("a pure function is a function"), Some(Tier::Proved));
    assert!(
        run.certificate("a pure function is a function")
            .rules
            .contains(&Rule::Congruence)
    );
}

/// The prover treats a type variable as an uninterpreted sort, so a proved
/// polymorphic law is genuinely polymorphic and the certificate says which
/// variables stayed uninterpreted.
#[test]
fn a_proved_polymorphic_law_records_its_sorts() {
    let dir = project(
        r#"
fn pair<a>(x: a) -> a = x

law "identity is identity"
  forall (x: a) {
    pair(x) == x
  }
"#,
    );
    let run = Run::of(dir.path());
    assert_eq!(run.tier("identity is identity"), Some(Tier::Proved));
    assert!(
        !run.certificate("identity is identity").sorts.is_empty(),
        "a proof over a type variable must name the sort it left uninterpreted"
    );
}

/// A spent budget is inconclusive, and inconclusive reports `property` — never
/// `proved`, and never `refuted`.
///
/// Over `Int`, so the enumeration tier cannot step in and decide it honestly:
/// what is under test is what happens when *nothing* decided it. Nothing here
/// does arithmetic on a drawn value either, so the sampled tier reports what it
/// found rather than a raised evaluation at `i64::MAX`.
///
/// The law has to be one the budget can actually starve, which not every true
/// law is: `(x + y) - y == x` is settled by interning both sides as the same
/// linear combination, before a single inference step is charged, so it is
/// `proved` at a budget of one and correctly so. The `steps` assertion below is
/// what keeps this test honest about that — a law the prover learns to settle
/// for free fails it loudly rather than passing for the wrong reason.
#[test]
fn a_spent_budget_reports_the_weaker_tier() {
    const LAW: &str = r#"
law "one is below, equal to, or above the other"
  forall (x: Int, y: Int) {
    (x < y) || (x == y) || (x > y)
  }
"#;
    const LABEL: &str = "below, equal to, or above";
    let dir = project(LAW);
    let decided = Run::of(dir.path());
    assert_eq!(
        decided.tier(LABEL),
        Some(Tier::Proved),
        "case analysis over the comparisons decides this when the budget allows"
    );
    assert!(
        decided.certificate(LABEL).steps > 1,
        "a law a budget of one already settles cannot test what a spent one does"
    );

    let starved = ProvePlan {
        prove_budget: 1,
        ..ProvePlan::default()
    };
    let run = Run::with(dir.path(), &starved);
    assert_eq!(
        run.tier(LABEL),
        Some(Tier::Property),
        "a spent budget is inconclusive, and inconclusive is the weaker tier"
    );
    assert!(
        !matches!(run.find(LABEL).1, Discharge::Refuted(_)),
        "an inconclusive attempt is not a refutation"
    );
}

// --- The outcomes that are not tiers ----------------------------------------

/// A guard the prover shows unsatisfiable is `Vacuous` — never `proved`. A
/// system that reported it proved would turn a typo in a guard into a proof of
/// everything.
#[test]
fn an_unsatisfiable_guard_is_vacuous_rather_than_proved() {
    let run = Run::of(&repo("tests/fixtures/vacuous_law.ply"));
    assert!(
        run.results
            .iter()
            .all(|(_, d)| matches!(d, Discharge::Vacuous(_))),
        "{:?}",
        run.results.iter().map(|(_, d)| d).collect::<Vec<_>>()
    );
    for (_, discharge) in &run.results {
        assert_eq!(discharge.tier(), None);
        assert!(!discharge.is_cacheable());
    }
}

/// Checking an `ensures` means calling the definition, and a definition that
/// performs needs a handler nothing supplies. That is a reported gap: no tier,
/// no coverage, exit 0.
#[test]
fn an_effectful_definition_is_a_gap_rather_than_a_claim() {
    let run = Run::of(&repo("tests/fixtures/obligation_not_discharged.ply"));
    let (_, discharge) = run.find("recorded");
    assert!(matches!(
        discharge,
        Discharge::Unattempted(ply_prove::Gap::UnhandledEffect(_))
    ));
    assert_eq!(discharge.tier(), None);
}

/// A spec that raises is not false, so it is neither a refutation nor a hold.
#[test]
fn an_evaluation_that_raises_is_a_gap_rather_than_a_refutation() {
    let run = Run::of(&repo("tests/fixtures/obligation_not_discharged.ply"));
    let (_, discharge) = run.find("share");
    assert!(
        matches!(discharge, Discharge::Unattempted(ply_prove::Gap::Raised { .. })),
        "{discharge:?}"
    );
}

// --- Concurrency laws -------------------------------------------------------

/// ADR 0007 §6's condition 5, which is the one an implementer drops: an
/// exhaustive interleaving search over *sampled values* proves something about
/// those values and nothing about the law.
#[test]
fn a_concurrency_law_over_a_binder_is_property_however_exhaustive_the_search() {
    let run = Run::of(&repo("tests/fixtures/concurrency_law_binder.ply"));
    let (obligation, discharge) = run.find("overdraw");
    assert!(!obligation.binders.is_empty(), "the law must have a binder");
    assert_eq!(
        discharge.tier(),
        Some(Tier::Property),
        "exhaustive over schedules says nothing about an `Int` binder"
    );
}

/// The same shape without a binder: the value domain is one point and the
/// interleaving search emptied its frontier, so both coverage claims hold and
/// the law is proved by execution.
#[test]
fn a_ground_concurrency_law_whose_search_is_exhaustive_is_proved() {
    let run = Run::of(&repo("examples"));
    let (obligation, discharge) = run.find("no interleaving of two guarded settlements");
    assert!(obligation.binders.is_empty());
    assert_eq!(discharge.tier(), Some(Tier::Proved));
    let Discharge::Held(Evidence::Proof(certificate)) = discharge else {
        unreachable!("just asserted proved");
    };
    assert!(
        certificate
            .rules
            .iter()
            .any(|r| matches!(r, Rule::ExhaustiveInterleaving { .. })),
        "an execution-derived proof names the rule that says so: {certificate:?}"
    );
}

/// Under `--sim once` there is no exhaustiveness to claim, whatever the
/// exploration reports — and one interleaving is not a coverage claim either,
/// so the honest label is the weaker of the two sampled tiers rather than
/// `property`.
#[test]
fn a_single_interleaving_never_proves_a_concurrency_law() {
    let plan = ProvePlan {
        sim: ply_eval::Plan::once(ply_eval::Seed::root(7)),
        ..ProvePlan::default()
    };
    let run = Run::with(&repo("examples"), &plan);
    assert_eq!(
        run.tier("no interleaving of two guarded settlements"),
        Some(Tier::Example),
        "one schedule is one concrete case, and `Evidence::tier` says so"
    );
}

// --- Determinism ------------------------------------------------------------

/// Two runs over one program agree on every tier, every certificate and every
/// counterexample. Today's artifact has to be diffable against yesterday's.
#[test]
fn two_runs_over_one_corpus_agree() {
    for path in corpus() {
        let first = Run::of(&path);
        let second = Run::of(&path);
        let render = |run: &Run| {
            run.results
                .iter()
                .map(|(o, d)| format!("{} {:?} {d:?}", o.owner, o.kind))
                .collect::<Vec<_>>()
        };
        assert_eq!(render(&first), render(&second), "in {}", path.display());
    }
}

/// A refutation names the input and says how far the search got, and both halves
/// are byte-identical across runs.
#[test]
fn a_refutation_shrinks_to_the_same_value_twice() {
    let run = Run::of(&repo("tests/fixtures/refuted_law.ply"));
    let (_, discharge) = run.find("settling a day's payments drops nothing");
    let Discharge::Refuted(first) = discharge else {
        panic!("the fixture exists to be refuted: {discharge:?}");
    };
    assert!(!first.bindings.is_empty());

    let again = Run::of(&repo("tests/fixtures/refuted_law.ply"));
    let Discharge::Refuted(second) = &again.find("settling a day's payments drops nothing").1 else {
        unreachable!("just refuted");
    };
    assert_eq!(
        first.bindings.iter().map(|b| b.rendered.clone()).collect::<Vec<_>>(),
        second.bindings.iter().map(|b| b.rendered.clone()).collect::<Vec<_>>()
    );
    assert_eq!(first.shrinks, second.shrinks);
}

/// The fixture above reports zero shrink steps: its first falsifying draw is
/// already `[-1, -1]`, so nothing there says the walk reduces anything. A
/// shrinker that never visibly reduces is not earning its place, so this is the
/// end-to-end evidence that it does — a law falsified only past a length the
/// generator reaches late, whose original run is long and whose elements are
/// large.
#[test]
fn a_long_counterexample_is_visibly_reduced() {
    let dir = project(
        r#"
law "a batch never holds more than six entries"
  forall (xs: List<Int>) {
    len(xs) <= 6
  }
"#,
    );
    let run = Run::of(dir.path());
    let Discharge::Refuted(counterexample) = &run.find("a batch never holds more than six").1 else {
        panic!("the law is false for every list of seven");
    };
    let width = |bindings: &[ply_prove::Binding]| -> usize {
        bindings.iter().map(|b| b.rendered.chars().count()).sum()
    };
    let (before, after) = (width(&counterexample.original), width(&counterexample.bindings));
    assert!(counterexample.shrinks > 0, "the walk accepted nothing");
    assert!(
        after * 2 < before,
        "shrank from {before} rendered characters only to {after}"
    );
    assert_eq!(
        counterexample.bindings[0].rendered,
        "[0, 0, 0, 0, 0, 0, 0]",
        "seven zeroes is the minimum: shorter satisfies the law and no element shrinks below 0"
    );
}

// --- Coverage ---------------------------------------------------------------

/// A definition carrying only `requires` makes no claim about behaviour, so it
/// is not covered and a reader still has to read it.
#[test]
fn a_precondition_alone_is_not_an_obligation() {
    let dir = project(
        r#"
fn withdraw(balance: Int, amount: Int) -> Int
  requires amount > 0
= balance - amount
"#,
    );
    let run = Run::of(dir.path());
    assert!(
        run.results.is_empty(),
        "a `requires` is a filter on a domain, not a claim to discharge"
    );
}

/// Each `ensures` is its own obligation at its own tier: a definition whose
/// first postcondition is proved and whose second is sampled is told both.
#[test]
fn each_postcondition_is_discharged_at_its_own_tier() {
    let run = Run::of(&repo("examples/ledger.ply"));
    let tiers: Vec<Option<Tier>> = run
        .results
        .iter()
        .filter(|(o, _)| {
            o.owner.as_str() == "ledger.transfer"
                && matches!(o.kind, ObligationKind::Ensures { .. })
        })
        .map(|(_, d)| d.tier())
        .collect();
    assert_eq!(
        tiers,
        vec![
            Some(Tier::Proved),
            Some(Tier::Proved),
            Some(Tier::Property)
        ],
        "one clause per obligation, or the pair would share the weaker label"
    );
}

/// How a sampled run contradicts a proof, or `None` when it does not.
///
/// A proof claims that every input satisfying the guard has an answer and that
/// the answer is `true`. A refutation denies the second; a raise denies the
/// first, and denying the first is the only shape ADR 0007 §5.1(a)'s ℤ-versus-
/// `i64` divergence can take in a language whose arithmetic never wraps.
fn disagreement(discharge: &Discharge) -> Option<String> {
    let rendered = |bindings: &[ply_prove::Binding]| {
        bindings
            .iter()
            .map(|b| format!("{} = {}", b.name, b.rendered))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match discharge {
        Discharge::Refuted(counterexample) => {
            Some(format!("refutes it at {}", rendered(&counterexample.bindings)))
        }
        Discharge::Unattempted(Gap::Raised {
            bindings,
            diagnostic,
        }) => Some(format!(
            "raises `{}` at {}",
            diagnostic.message,
            rendered(bindings)
        )),
        _ => None,
    }
}
