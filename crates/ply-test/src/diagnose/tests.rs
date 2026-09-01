//! The five situations a failure can be in, driven end to end through [`diagnose`].

use super::{Evidence, Options, diagnose};
use crate::bisect::{
    Baseline, ChangeKind, Classify, Confidence, DefKey, Delta, DepEdges, Hybrid, Mode, Skipped,
    Trial, TrialOutcome, Unresolved, Verdict,
};
use crate::slice::{CausalSlice, Entered, Frame, Tracing};
use ply_core::Footprint;
use ply_hash::{DefHash, HashOutput};
use ply_span::{Span, Symbol};
use std::collections::{BTreeMap, BTreeSet};

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

fn hash(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

const KEY: &str = "m.a regression";

/// A program of `n` definitions, all in the failing test's closure.
fn hashes(defs: &[(&str, u8)], test: u8) -> HashOutput {
    let mut out = HashOutput {
        tests: vec![hash(test)],
        ..HashOutput::default()
    };
    let mut closure: BTreeSet<Symbol> = BTreeSet::new();
    for (name, byte) in defs {
        out.defs.insert(sym(name), hash(*byte));
        closure.insert(sym(name));
    }
    closure.insert(sym(KEY));
    out.closure.insert(sym(KEY), closure);
    out
}

fn baseline(defs: &[(&str, u8)], test: u8) -> Baseline {
    Baseline::new(
        hash(test),
        defs.iter().map(|(n, b)| (sym(n), hash(*b))).collect(),
    )
}

/// Answers the classification questions from a table, so a test says exactly what the system was
/// told rather than deriving it.
struct Told {
    /// Names the current body re-normalizes to the baseline hash for — the `Derived` ones.
    derived: Vec<Symbol>,
    interface_moved: Vec<Symbol>,
    test_edited: bool,
    baseline: Baseline,
    test_after: DefHash,
}

impl Classify for Told {
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash> {
        if self.derived.contains(&key.name) {
            return self.baseline.hash(&key.name);
        }
        Some(hash(0xEE))
    }
    fn renormalized_test(&mut self, _: &Symbol) -> Option<DefHash> {
        if self.test_edited {
            Some(self.test_after)
        } else {
            Some(self.baseline.test_hash)
        }
    }
    fn interface_stable(&mut self, key: &DefKey, _: DefHash) -> Option<bool> {
        Some(!self.interface_moved.contains(&key.name))
    }
}

impl Told {
    fn new(baseline: Baseline, test_after: DefHash) -> Told {
        Told {
            derived: Vec::new(),
            interface_moved: Vec::new(),
            test_edited: false,
            baseline,
            test_after,
        }
    }
}

/// Fails exactly when every culprit is on the post-edit side.
struct Culprits {
    culprits: Vec<Symbol>,
    inseparable: Vec<(Symbol, Symbol)>,
    trials: usize,
}

impl Culprits {
    fn new(culprits: &[&str]) -> Culprits {
        Culprits {
            culprits: culprits.iter().map(|c| sym(c)).collect(),
            inseparable: Vec::new(),
            trials: 0,
        }
    }
}

impl Hybrid for Culprits {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
        self.trials += 1;
        let names = delta.flipped_names(flipped);
        for (a, b) in &self.inseparable {
            if names.contains(a) != names.contains(b) {
                return Trial::unresolved(Unresolved::DoesNotCheck);
            }
        }
        if self.culprits.iter().all(|c| names.contains(c)) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    }
}

/// Which of the preconditions hold.
#[derive(Clone, Copy)]
struct Situation {
    baseline: bool,
    nondet: bool,
    defect: bool,
    /// The failing run reached a host handler.
    host: bool,
    /// Which way a missing hybrid builder is reported.
    absent: Skipped,
}

impl Default for Situation {
    fn default() -> Situation {
        Situation {
            baseline: true,
            nondet: false,
            defect: false,
            host: false,
            absent: Skipped::NoBodies,
        }
    }
}

struct Case {
    defs_before: Vec<(&'static str, u8)>,
    defs_after: Vec<(&'static str, u8)>,
    edited: Vec<Symbol>,
    test_before: u8,
    test_after: u8,
}

impl Case {
    /// **Every** definition's hash moves — that is what one edit to a leaf does to a closure — and
    /// `edited` says which of them anybody actually wrote.
    fn edits(defs: &[&'static str], edited: &[&'static str]) -> Case {
        let before: Vec<(&'static str, u8)> = defs
            .iter()
            .enumerate()
            .map(|(i, n)| (*n, i as u8 + 1))
            .collect();
        let after = before.iter().map(|(n, b)| (*n, b + 100)).collect();
        Case {
            defs_before: before,
            defs_after: after,
            edited: edited.iter().map(|n| sym(n)).collect(),
            test_before: 200,
            test_after: 201,
        }
    }

    fn run(
        &self,
        options: &Options,
        told: impl FnOnce(&mut Told),
        hybrid: Option<&mut dyn Hybrid>,
        situation: Situation,
        slice: Option<CausalSlice>,
    ) -> crate::Attribution {
        let base = baseline(&self.defs_before, self.test_before);
        let hashes = hashes(&self.defs_after, self.test_after);
        let mut classify = Told::new(base.clone(), hash(self.test_after));
        classify.derived = self
            .defs_after
            .iter()
            .map(|(n, _)| sym(n))
            .filter(|n| !self.edited.contains(n))
            .collect();
        told(&mut classify);

        let suspects: Vec<Symbol> = self.defs_after.iter().map(|(n, _)| sym(n)).collect();
        let key = sym(KEY);
        let evidence = Evidence {
            key: &key,
            test_hash: Some(hash(self.test_after)),
            nondet: situation.nondet,
            defect: situation.defect,
            host: situation.host,
            suspects: &suspects,
            hashes: &hashes,
            baseline: situation.baseline.then_some(&base),
            slice,
        };
        diagnose(
            evidence,
            options,
            &DepEdges::new(),
            &mut classify,
            hybrid,
            situation.absent,
        )
    }
}

fn simple(case: &Case, culprits: &mut Culprits) -> crate::Attribution {
    case.run(
        &Options::default(),
        |_| {},
        Some(culprits),
        Situation::default(),
        None,
    )
}

/// A single-culprit regression.
#[test]
fn a_single_culprit_regression_is_named_exactly() {
    let case = Case::edits(&["a", "b", "c", "d"], &["a"]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        Some(&mut hybrid),
        Situation::default(),
        None,
    );

    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.bisection.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("a")]);
    assert_eq!(hybrid.trials, 0, "one cluster costs nothing");

    assert_eq!(out.suspects[0].name, sym("a"));
    assert!(out.suspects[0].culprit);
    assert_eq!(out.suspects[0].change, Some(ChangeKind::Edited));
    assert_eq!(out.suspects[0].before, Some(hash(1)));
    for suspect in &out.suspects[1..] {
        assert_eq!(suspect.change, Some(ChangeKind::Derived), "{suspect:?}");
        assert!(!suspect.culprit);
    }
}

/// Two edits that only break the test together.
#[test]
fn a_multi_culprit_regression_names_both() {
    let case = Case::edits(&["a", "b", "c", "d", "e"], &["a", "b", "c", "d", "e"]);
    let mut hybrid = Culprits::new(&["b", "e"]);
    let out = simple(&case, &mut hybrid);

    assert_eq!(out.bisection.verdict, Verdict::Bisected);
    assert_eq!(out.bisection.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("b"), sym("e")]);
    assert!(
        out.suspects[0].culprit && out.suspects[1].culprit,
        "the culprits rank first: {:?}",
        out.suspects
    );
}

/// The case the failure artifact calls common rather than exotic.
#[test]
fn a_hybrid_that_does_not_typecheck_is_not_evidence_and_costs_the_minimality_claim() {
    let case = Case::edits(&["a", "b", "c", "d"], &["a", "b", "c", "d"]);
    let mut hybrid = Culprits::new(&["b"]);
    hybrid.inseparable.push((sym("b"), sym("c")));
    let out = simple(&case, &mut hybrid);

    assert_eq!(out.bisection.verdict, Verdict::Bisected);
    assert!(out.bisection.search.unresolved > 0);
    assert_eq!(out.bisection.confidence, Confidence::Partial);
    assert!(
        out.culprits().contains(&sym("b")),
        "the true cause survives: {:?}",
        out.culprits()
    );
}

/// Fusing the pair up front is strictly better than letting ddmin discover it: the same answer,
/// exactly, with no unresolved trial to pay for.
#[test]
fn fusing_an_interface_change_with_its_caller_beats_discovering_it() {
    let case = Case::edits(
        &["caller", "callee", "other"],
        &["caller", "callee", "other"],
    );
    let mut hybrid = Culprits::new(&["callee"]);
    hybrid.inseparable.push((sym("callee"), sym("caller")));

    let base = baseline(&case.defs_before, case.test_before);
    let hashes = hashes(&case.defs_after, case.test_after);
    let mut classify = Told::new(base.clone(), hash(case.test_after));
    classify.interface_moved = vec![sym("callee")];

    let mut edges = DepEdges::new();
    edges.add(sym("caller"), sym("callee"));

    let suspects: Vec<Symbol> = case.defs_after.iter().map(|(n, _)| sym(n)).collect();
    let key = sym(KEY);
    let out = diagnose(
        Evidence {
            key: &key,
            test_hash: Some(hash(case.test_after)),
            nondet: false,
            defect: false,
            host: false,
            suspects: &suspects,
            hashes: &hashes,
            baseline: Some(&base),
            slice: None,
        },
        &Options::default(),
        &edges,
        &mut classify,
        Some(&mut hybrid),
        Skipped::NoBodies,
    );

    assert_eq!(out.bisection.confidence, Confidence::Fused);
    assert_eq!(
        out.bisection.search.unresolved, 0,
        "no split was ever built"
    );
    assert_eq!(
        out.bisection.groups,
        vec![vec![sym("callee"), sym("caller")]]
    );
}

/// A first-ever-red test.
#[test]
fn a_test_that_never_passed_is_not_bisected() {
    let case = Case::edits(&["a", "b"], &["a", "b"]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        Some(&mut hybrid),
        Situation {
            baseline: false,
            ..Situation::default()
        },
        None,
    );

    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NeverPassed)
    );
    assert_eq!(out.bisection.confidence, Confidence::None);
    assert_eq!(hybrid.trials, 0);
    assert!(out.culprits().is_empty());
    assert!(out.bisection.reason.contains("never passed"));
    // Without a baseline nothing can be said about what changed, and saying nothing is the point:
    // `change: None` is not `change: derived`.
    assert!(out.suspects.iter().all(|s| s.change.is_none()));
}

/// A first-ever-red test still gets the artifact's other half.
#[test]
fn a_test_that_never_passed_still_carries_its_causal_slice() {
    let case = Case::edits(&["a", "b"], &["a", "b"]);
    let slice = CausalSlice {
        traced: true,
        reproduced: true,
        entered: vec![Entered {
            name: sym("a"),
            hash: None,
            calls: 3,
        }],
        stack: vec![Frame {
            name: sym("a"),
            hash: None,
            call_site: Span::DUMMY,
        }],
        observed: Footprint::empty(),
        truncated: false,
    };
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation {
            baseline: false,
            ..Situation::default()
        },
        Some(slice),
    );

    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NeverPassed)
    );
    let slice = out.slice.as_ref().expect("the slice survives");
    assert!(slice.traced);
    assert_eq!(out.suspects[0].name, sym("a"));
    assert_eq!(out.suspects[0].ran, Some(true));
    assert_eq!(out.suspects[0].depth, Some(0));
    assert_eq!(out.suspects[1].ran, Some(false), "b never ran");
}

/// The flaky-looking case: the failure reproduces against the definitions as they were, and the
/// test was not edited, so nothing in the definition graph explains it.
#[test]
fn a_failure_no_change_explains_is_reported_rather_than_attributed() {
    let case = Case::edits(&["a", "b", "c"], &["a", "b", "c"]);
    let mut hybrid = Culprits::new(&[]);
    let out = simple(&case, &mut hybrid);

    assert_eq!(out.bisection.verdict, Verdict::NotInTheGraph);
    assert_eq!(out.bisection.confidence, Confidence::None);
    assert!(out.culprits().is_empty());
    assert!(out.bisection.reason.contains("nondet"));
    // Still annotated: an agent that reads no culprit still learns which three definitions it
    // edited.
    assert!(
        out.suspects
            .iter()
            .all(|s| s.change == Some(ChangeKind::Edited))
    );
}

/// The same shape, with the test itself edited.
#[test]
fn editing_only_the_test_body_names_the_test() {
    let case = Case::edits(&["a", "b"], &[]);
    let mut hybrid = Culprits::new(&[]);
    let out = case.run(
        &Options::default(),
        |told| told.test_edited = true,
        Some(&mut hybrid),
        Situation::default(),
        None,
    );

    assert_eq!(out.bisection.verdict, Verdict::TestChanged);
    assert_eq!(out.culprits(), vec![sym(KEY)]);
}

#[test]
fn a_nondet_test_is_not_bisected() {
    let case = Case::edits(&["a"], &["a"]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        Some(&mut hybrid),
        Situation {
            nondet: true,
            ..Situation::default()
        },
        None,
    );
    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::Nondet)
    );
    assert_eq!(hybrid.trials, 0);
}

#[test]
fn a_panic_is_not_bisected() {
    let case = Case::edits(&["a"], &["a"]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        Some(&mut hybrid),
        Situation {
            defect: true,
            ..Situation::default()
        },
        None,
    );
    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::Panicked)
    );
    assert_eq!(hybrid.trials, 0);
}

/// A host-backed failure runs no trial at all, and the suspect list survives.
#[test]
fn a_host_backed_failure_is_not_bisected_and_keeps_its_suspects() {
    let case = Case::edits(&["a", "b"], &["a"]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        Some(&mut hybrid),
        Situation {
            host: true,
            ..Situation::default()
        },
        None,
    );
    assert_eq!(out.bisection.verdict, Verdict::NotAttempted(Skipped::Host));
    assert_eq!(hybrid.trials, 0, "a trial would have re-reached the host");
    assert!(out.culprits().is_empty(), "a culprit was named anyway");
    assert_eq!(
        out.suspects
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
        [sym("a"), sym("b")],
        "the static half of the artifact is still owed to the reader"
    );
    // And `--bisect always` does not talk it out of the refusal: the reason is an action on the
    // world rather than a budget.
    let out = case.run(
        &Options {
            bisect: Mode::Always,
            ..Options::default()
        },
        |_| {},
        Some(&mut hybrid),
        Situation {
            host: true,
            ..Situation::default()
        },
        None,
    );
    assert_eq!(out.bisection.verdict, Verdict::NotAttempted(Skipped::Host));
    assert_eq!(hybrid.trials, 0);
}

#[test]
fn bisect_never_evaluates_nothing() {
    let case = Case::edits(&["a", "b"], &["a", "b"]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options {
            bisect: Mode::Never,
            trace: Tracing::Never,
            ..Options::default()
        },
        |_| {},
        Some(&mut hybrid),
        Situation::default(),
        None,
    );
    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NotRequested)
    );
    assert_eq!(hybrid.trials, 0);
}

/// One edit is decided by counting the clusters, so a cold body store costs nothing on the failure
/// the default exists for.
#[test]
fn a_single_edit_is_answered_without_a_body_store() {
    let case = Case::edits(&["a", "b", "c"], &["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation::default(),
        None,
    );

    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.culprits(), vec![sym("a")]);
    assert_eq!(out.suspects[0].name, sym("a"));
    assert_eq!(out.suspects[0].change, Some(ChangeKind::Edited));
    assert_eq!(
        out.suspects
            .iter()
            .filter(|s| s.change == Some(ChangeKind::Derived))
            .count(),
        2
    );
}

/// A search that would have had to run a mixture says `no_bodies` rather than concluding from the
/// silence, and still annotates what it could.
#[test]
fn a_pruned_body_store_refuses_the_searches_that_need_one() {
    let case = Case::edits(&["a", "b", "c"], &["a", "b"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation::default(),
        None,
    );

    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NoBodies)
    );
    assert!(out.culprits().is_empty());
    assert_eq!(
        out.suspects
            .iter()
            .filter(|s| s.change == Some(ChangeKind::Edited))
            .count(),
        2
    );
    assert_eq!(out.suspects[2].change, Some(ChangeKind::Derived));
}

/// A pruned cache and a build that cannot mix eras both stop the search, and a consumer can act on
/// the first and not the second — so they must not collapse onto one verdict.
#[test]
fn a_build_with_no_hybrid_builder_is_not_reported_as_a_pruned_cache() {
    let case = Case::edits(&["a", "b", "c"], &["a", "b"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation {
            absent: Skipped::NoHybrids,
            ..Situation::default()
        },
        None,
    );

    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NoHybrids)
    );
    // The annotation is what still ships: it needs a baseline, not a mixture.
    assert_eq!(
        out.suspects
            .iter()
            .filter(|s| s.change == Some(ChangeKind::Edited))
            .count(),
        2
    );
    assert_eq!(out.suspects[2].change, Some(ChangeKind::Derived));
}

/// One cluster is answered by counting, so the absence of a hybrid builder must not withhold it —
/// this is the overwhelmingly common failure.
#[test]
fn a_single_edit_is_still_named_without_any_hybrid_builder() {
    let case = Case::edits(&["a", "b", "c"], &["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation {
            absent: Skipped::NoHybrids,
            ..Situation::default()
        },
        None,
    );

    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.culprits(), vec![sym("a")]);
    assert_eq!(out.bisection.search.evaluated, 0);
}

/// A derived change sorts below every edited one, because the reading list an agent is handed is
/// the ranking.
#[test]
fn derived_suspects_sort_below_edited_ones() {
    let case = Case::edits(&["aaa", "zzz"], &["zzz"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation::default(),
        None,
    );
    assert_eq!(out.suspects[0].name, sym("zzz"));
    assert_eq!(out.suspects[0].change, Some(ChangeKind::Edited));
    assert_eq!(out.suspects[1].name, sym("aaa"));
}

/// Two runs over one failure must produce byte-identical artifacts, or the artifact cannot be
/// diffed against yesterday's.
#[test]
fn two_runs_over_one_failure_agree_exactly() {
    let case = Case::edits(
        &["a", "b", "c", "d", "e", "f"],
        &["a", "b", "c", "d", "e", "f"],
    );
    let render = || {
        let mut hybrid = Culprits::new(&["c", "e"]);
        let out = simple(&case, &mut hybrid);
        (
            crate::report::failure_json(&crate::Failure {
                name: "a regression".to_string(),
                key: sym(KEY),
                diagnostic: ply_span::Diagnostic::error(ply_span::codes::ASSERTION_FAILED, "x"),
                defect: false,
                host: false,
                suspects: Vec::new(),
                assertion: None,
                attribution: out,
                seed: None,
                race: None,
            })
            .to_string(),
            hybrid.trials,
        )
    };
    assert_eq!(render(), render());
}

/// A spent budget is a superset of the cause, and says so.
#[test]
fn a_spent_budget_downgrades_confidence_and_keeps_the_cause() {
    let names: Vec<&'static str> = vec!["d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7"];
    let case = Case::edits(&names, &names);
    let mut hybrid = Culprits::new(&["d7"]);
    let out = case.run(
        &Options {
            budget: crate::Budget::new(2),
            ..Options::default()
        },
        |_| {},
        Some(&mut hybrid),
        Situation::default(),
        None,
    );

    assert!(out.bisection.search.exhausted);
    assert_eq!(out.bisection.confidence, Confidence::Partial);
    assert!(out.culprits().contains(&sym("d7")));
}

/// `--bisect always` waives the budget and nothing else.
#[test]
fn always_waives_the_budget_but_not_the_preconditions() {
    let names: Vec<&'static str> = vec!["d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7"];
    let case = Case::edits(&names, &names);
    let mut hybrid = Culprits::new(&["d7"]);
    let out = case.run(
        &Options {
            bisect: Mode::Always,
            budget: crate::Budget::new(2),
            ..Options::default()
        },
        |_| {},
        Some(&mut hybrid),
        Situation::default(),
        None,
    );
    assert!(!out.bisection.search.exhausted);
    assert_eq!(out.culprits(), vec![sym("d7")]);

    let mut hybrid = Culprits::new(&["d7"]);
    let out = case.run(
        &Options {
            bisect: Mode::Always,
            ..Options::default()
        },
        |_| {},
        Some(&mut hybrid),
        Situation {
            baseline: false,
            ..Situation::default()
        },
        None,
    );
    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NeverPassed)
    );
}

/// A trace that went green is evidence about a different execution, so it must not be folded into
/// `ran` and `depth`.
#[test]
fn a_slice_that_did_not_reproduce_does_not_annotate_the_suspects() {
    let case = Case::edits(&["a", "b"], &["a"]);
    let slice = CausalSlice {
        traced: true,
        reproduced: false,
        entered: vec![Entered {
            name: sym("a"),
            hash: None,
            calls: 1,
        }],
        stack: Vec::new(),
        observed: Footprint::empty(),
        truncated: false,
    };
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation::default(),
        Some(slice),
    );
    assert!(out.suspects.iter().all(|s| s.ran.is_none()));
}

#[test]
fn an_unanswerable_classification_is_stated_in_the_reason() {
    let case = Case::edits(&["a", "b"], &["a", "b"]);

    struct Silent;
    impl Classify for Silent {
        fn renormalized(&mut self, _: &DefKey) -> Option<DefHash> {
            None
        }
        fn renormalized_test(&mut self, _: &Symbol) -> Option<DefHash> {
            None
        }
        fn interface_stable(&mut self, _: &DefKey, _: DefHash) -> Option<bool> {
            None
        }
    }

    let base = baseline(&case.defs_before, case.test_before);
    let hashes = hashes(&case.defs_after, case.test_after);
    let suspects: Vec<Symbol> = case.defs_after.iter().map(|(n, _)| sym(n)).collect();
    let key = sym(KEY);
    let mut hybrid = Culprits::new(&["a"]);
    let out = diagnose(
        Evidence {
            key: &key,
            test_hash: Some(hash(case.test_after)),
            nondet: false,
            defect: false,
            host: false,
            suspects: &suspects,
            hashes: &hashes,
            baseline: Some(&base),
            slice: None,
        },
        &Options::default(),
        &DepEdges::new(),
        &mut Silent,
        Some(&mut hybrid),
        Skipped::NoBodies,
    );

    assert!(
        out.bisection.reason.contains("could not be told apart"),
        "{}",
        out.bisection.reason
    );
}

/// The whole delta being derived is not a delta: there is nothing to flip, and saying so is
/// different from saying the search found nothing.
#[test]
fn a_delta_of_only_derived_changes_is_no_changes() {
    let case = Case::edits(&["a", "b"], &[]);
    let mut hybrid = Culprits::new(&["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        Some(&mut hybrid),
        Situation::default(),
        None,
    );
    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NoChanges)
    );
    assert_eq!(hybrid.trials, 0);
}

/// A culprit the store never noticed change is still the answer, so it has to appear in the list
/// that is ranked.
#[test]
fn a_culprit_outside_the_raw_suspect_set_is_added_to_it() {
    let base = baseline(&[("a", 1), ("b", 2)], 200);
    let after = hashes(&[("a", 101), ("b", 2)], 201);
    let mut classify = Told::new(base.clone(), hash(201));
    let mut hybrid = Culprits::new(&["a"]);
    let key = sym(KEY);
    let out = diagnose(
        Evidence {
            key: &key,
            test_hash: Some(hash(201)),
            nondet: false,
            defect: false,
            host: false,
            suspects: &[],
            hashes: &after,
            baseline: Some(&base),
            slice: None,
        },
        &Options::default(),
        &DepEdges::new(),
        &mut classify,
        Some(&mut hybrid),
        Skipped::NoBodies,
    );

    assert_eq!(out.culprits(), vec![sym("a")]);
    assert_eq!(out.suspects[0].name, sym("a"));
    assert!(out.suspects[0].culprit);
}

#[test]
fn an_empty_baseline_map_is_still_a_baseline() {
    let base = Baseline::new(hash(200), BTreeMap::new());
    assert_eq!(base.hash(&sym("a")), None);
    assert_eq!(base.test_hash, hash(200));
}

#[test]
fn the_trial_outcomes_are_distinguishable_in_the_artifact() {
    assert_ne!(
        TrialOutcome::Unresolved(Unresolved::DoesNotCheck),
        TrialOutcome::Unresolved(Unresolved::MissingBody)
    );
}

fn failure_with(attribution: crate::Attribution) -> crate::Failure {
    crate::Failure {
        name: "a regression".to_string(),
        key: sym(KEY),
        diagnostic: ply_span::Diagnostic::error(
            ply_span::codes::ASSERTION_FAILED,
            "assertion failed: expected 0, found -5",
        ),
        defect: false,
        host: false,
        suspects: Vec::new(),
        assertion: None,
        attribution,
        seed: None,
        race: None,
    }
}

fn summary_of(attribution: crate::Attribution) -> Vec<String> {
    crate::RunReport {
        passed: 0,
        failed: 1,
        cached: 0,
        failures: vec![failure_with(attribution)],
        duration: std::time::Duration::from_millis(1),
        parallelism: Default::default(),
        results: Vec::new(),
        warnings: Vec::new(),
        simulation: Default::default(),
        audit: None,
    }
    .summary()
}

fn position_of(lines: &[String], needle: &str) -> Option<usize> {
    lines.iter().position(|l| l.contains(needle))
}

/// The culprit is the answer and the diff is the evidence, so the culprit comes first.
#[test]
fn the_culprit_line_comes_before_the_assertion() {
    let case = Case::edits(&["a", "b", "c", "d"], &["a"]);
    let mut hybrid = Culprits::new(&["a"]);
    let lines = summary_of(simple(&case, &mut hybrid));

    let culprit = position_of(&lines, "culprit: a").expect("a culprit line");
    let assertion = position_of(&lines, "assertion failed").expect("the assertion line");
    assert!(culprit < assertion, "{lines:#?}");
}

/// A failure the system could say nothing about gets no culprit line, rather than a line naming
/// something it has no evidence for.
#[test]
fn a_failure_with_no_culprit_names_nobody() {
    let case = Case::edits(&["a"], &["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation {
            baseline: false,
            ..Situation::default()
        },
        None,
    );
    let lines = summary_of(out);

    assert!(position_of(&lines, "culprit").is_none(), "{lines:#?}");
    assert!(position_of(&lines, "assertion failed").is_some());
}

/// The JSON is the artifact an agent branches on, so the fields it branches on have to be there and
/// have to mean what the artifact's field table says.
#[test]
fn the_json_artifact_leads_with_the_verdict() {
    let case = Case::edits(&["a", "b"], &["a"]);
    let mut hybrid = Culprits::new(&["a"]);
    let json = crate::report::failure_json(&failure_with(simple(&case, &mut hybrid)));

    let culprit = &json["culprit"];
    assert_eq!(culprit["verdict"], "sole");
    assert_eq!(culprit["confidence"], "minimal");
    assert_eq!(culprit["skipped"], serde_json::Value::Null);
    assert_eq!(culprit["definitions"], serde_json::json!(["a"]));
    assert_eq!(culprit["search"]["evaluated"], 0);

    let suspects = json["suspects"].as_array().expect("an array of objects");
    assert_eq!(suspects[0]["name"], "a");
    assert_eq!(suspects[0]["change"], "edited");
    assert_eq!(suspects[0]["culprit"], true);
    assert_eq!(suspects[1]["change"], "derived");
    assert_eq!(suspects[1]["culprit"], false);
}

#[test]
fn a_skipped_bisection_says_why_in_the_json() {
    let case = Case::edits(&["a"], &["a"]);
    let out = case.run(
        &Options::default(),
        |_| {},
        None,
        Situation {
            baseline: false,
            ..Situation::default()
        },
        None,
    );
    let json = crate::report::failure_json(&failure_with(out));
    assert_eq!(json["culprit"]["verdict"], "not_attempted");
    assert_eq!(json["culprit"]["skipped"], "never_passed");
    assert_eq!(json["culprit"]["confidence"], "none");
}
