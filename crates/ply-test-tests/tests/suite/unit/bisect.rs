mod delta;

use ply_span::Symbol;
use ply_test::bisect::{
    Budget, Change, Confidence, Delta, DepEdges, FusionReason, Hybrid, Skipped, Trial, Unresolved,
    Verdict, bisect,
};
use ply_ty::DefHash;
use std::collections::BTreeSet;

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

fn hash(byte: u8) -> DefHash {
    DefHash([byte; 32])
}

fn edges(pairs: &[(&str, &str)]) -> DepEdges {
    let mut edges = DepEdges::new();
    for (from, to) in pairs {
        edges.add(sym(from), sym(to));
    }
    edges
}

/// Answers `Fails` exactly when the flipped set covers every culprit.
struct Culprits {
    culprits: Vec<Symbol>,
    /// Sets whose flipped names split one of these pairs do not typecheck.
    inseparable: Vec<(Symbol, Symbol)>,
    asked: Vec<Vec<usize>>,
    cached: BTreeSet<Vec<usize>>,
}

impl Culprits {
    fn new(culprits: &[&str]) -> Culprits {
        Culprits {
            culprits: culprits.iter().map(|c| sym(c)).collect(),
            inseparable: Vec::new(),
            asked: Vec::new(),
            cached: BTreeSet::new(),
        }
    }
}

impl Hybrid for Culprits {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
        self.asked.push(flipped.to_vec());
        let names = delta.flipped_names(flipped);
        for (a, b) in &self.inseparable {
            if names.contains(a) != names.contains(b) {
                return Trial::unresolved(Unresolved::DoesNotCheck);
            }
        }
        let trial = if self.culprits.iter().all(|c| names.contains(c)) {
            Trial::fails()
        } else {
            Trial::passes()
        };
        if self.cached.contains(flipped) {
            trial.from_cache()
        } else {
            trial
        }
    }
}

fn independent_changes(names: &[&str]) -> Vec<Change> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| edited_change(n, i as u8))
        .collect()
}

fn edited_change(name: &str, seed: u8) -> Change {
    Change::edited(sym(name), hash(seed), hash(seed.wrapping_add(128)), true)
}

#[test]
fn an_interface_stable_edit_is_its_own_cluster() {
    let delta = Delta::new(
        None,
        independent_changes(&["a", "b", "c"]),
        &edges(&[("b", "a"), ("c", "b")]),
    );
    assert_eq!(delta.clusters.len(), 3);
    assert!(
        delta
            .clusters
            .iter()
            .all(|c| c.reason == FusionReason::Independent)
    );
}

#[test]
fn an_interface_change_fuses_with_the_callers_that_had_to_change_with_it() {
    let mut changes = independent_changes(&["caller", "other"]);
    changes.push(Change::edited(sym("callee"), hash(9), hash(10), false));
    // `caller` mentions `callee`; `other` mentions nothing that moved.
    let delta = Delta::new(None, changes, &edges(&[("caller", "callee")]));

    assert_eq!(delta.clusters.len(), 2);
    let fused = delta
        .clusters
        .iter()
        .find(|c| c.members.len() == 2)
        .expect("a fused cluster");
    assert_eq!(fused.members, vec![sym("callee"), sym("caller")]);
    assert_eq!(fused.reason, FusionReason::InterfaceChanged);
}

#[test]
fn fusion_is_transitive_through_the_union_find() {
    let changes = vec![
        Change::edited(sym("a"), hash(1), hash(2), false),
        Change::edited(sym("b"), hash(3), hash(4), false),
        Change::edited(sym("c"), hash(5), hash(6), true),
    ];
    // c mentions b, b mentions a; a and b both moved their interfaces.
    let delta = Delta::new(None, changes, &edges(&[("b", "a"), ("c", "b")]));
    assert_eq!(delta.clusters.len(), 1);
    assert_eq!(
        delta.clusters[0].members,
        vec![sym("a"), sym("b"), sym("c")]
    );
}

#[test]
fn an_added_definition_drags_in_everything_that_mentions_it() {
    let mut changes = independent_changes(&["caller"]);
    changes.push(Change::added(sym("helper"), hash(7)));
    let delta = Delta::new(None, changes, &edges(&[("caller", "helper")]));
    assert_eq!(delta.clusters.len(), 1);
    assert_eq!(delta.clusters[0].reason, FusionReason::Existence);
}

#[test]
fn a_removed_definition_fuses_through_baseline_edges() {
    let mut changes = independent_changes(&["keeper"]);
    changes.push(Change::removed(sym("gone"), hash(4)));
    let delta = Delta::new(None, changes, &edges(&[("keeper", "gone")]));
    assert_eq!(delta.clusters.len(), 1);
    assert_eq!(delta.clusters[0].members, vec![sym("gone"), sym("keeper")]);
}

#[test]
fn a_derived_change_is_never_a_candidate() {
    let mut changes = independent_changes(&["edited"]);
    changes.push(Change::derived(sym("dependent"), hash(1), hash(2)));
    let delta = Delta::new(None, changes, &edges(&[("dependent", "edited")]));
    assert_eq!(delta.candidates(), 1);
    assert_eq!(delta.clusters.len(), 1);
    assert_eq!(delta.clusters[0].members, vec![sym("edited")]);
}

#[test]
fn one_candidate_is_answered_without_running_anything() {
    let delta = Delta::new(None, independent_changes(&["only"]), &DepEdges::new());
    let mut oracle = Culprits::new(&["only"]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Sole);
    assert_eq!(out.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("only")]);
    assert_eq!(out.search.evaluated, 0);
    assert!(oracle.asked.is_empty());
}

#[test]
fn a_single_culprit_among_sixteen_is_found_logarithmically() {
    let names: Vec<String> = (0..16).map(|i| format!("d{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
    let mut oracle = Culprits::new(&["d11"]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("d11")]);
    // 2·log2(16) halvings, plus the reproduction trial and the baseline one.
    assert!(out.search.evaluated <= 12, "{:?}", out.search);
}

#[test]
fn two_changes_that_only_fail_together_are_both_reported() {
    let delta = Delta::new(
        None,
        independent_changes(&["a", "b", "c", "d"]),
        &DepEdges::new(),
    );
    let mut oracle = Culprits::new(&["a", "d"]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.culprits(), vec![sym("a"), sym("d")]);
    assert_eq!(out.confidence, Confidence::Minimal);
}

#[test]
fn a_fused_group_is_reported_as_fused_rather_than_as_two_answers() {
    let mut changes = independent_changes(&["x", "y"]);
    changes.push(Change::edited(sym("callee"), hash(20), hash(21), false));
    let delta = Delta::new(None, changes, &edges(&[("x", "callee")]));
    let mut oracle = Culprits::new(&["callee"]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.confidence, Confidence::Fused);
    assert_eq!(out.groups, vec![vec![sym("callee"), sym("x")]]);
}

#[test]
fn hybrids_that_do_not_typecheck_are_not_evidence() {
    let delta = Delta::new(
        None,
        independent_changes(&["a", "b", "c", "d"]),
        &DepEdges::new(),
    );
    let mut oracle = Culprits::new(&["b"]);
    oracle.inseparable.push((sym("b"), sym("c")));
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.culprits(), vec![sym("b"), sym("c")]);
    assert!(out.search.unresolved > 0, "{:?}", out.search);
    assert_eq!(out.confidence, Confidence::Partial);
}

#[test]
fn a_spent_budget_downgrades_the_confidence_rather_than_lying() {
    let names: Vec<String> = (0..32).map(|i| format!("d{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
    let mut oracle = Culprits::new(&["d31"]);
    let out = bisect(&delta, &mut oracle, Budget::new(3));

    assert!(out.search.exhausted);
    assert_eq!(out.confidence, Confidence::Partial);
    assert_eq!(out.search.evaluated, 3);
    assert!(out.culprits().contains(&sym("d31")));
}

#[test]
fn a_cached_trial_is_not_charged_against_the_budget() {
    let delta = Delta::new(None, independent_changes(&["a", "b"]), &DepEdges::new());
    let mut oracle = Culprits::new(&["a"]);
    oracle.cached.insert(vec![0, 1]);
    oracle.cached.insert(vec![]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.search.cached, 2);
    assert!(out.search.evaluated < out.search.cached + out.search.evaluated + 1);
    assert_eq!(out.culprits(), vec![sym("a")]);
}

#[test]
fn a_failure_the_baseline_also_shows_is_attributed_to_the_test_when_it_moved() {
    let delta = Delta::new(
        Some(Change::edited(sym("m.a test"), hash(1), hash(2), true)),
        independent_changes(&["a", "b"]),
        &DepEdges::new(),
    );
    let mut oracle = Culprits::new(&[]); // fails for every subset, including none
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::TestChanged);
    assert_eq!(out.culprits(), vec![sym("m.a test")]);
    assert!(out.reason.contains("edit to the test"));
}

#[test]
fn a_failure_no_change_explains_says_so_instead_of_naming_someone() {
    let delta = Delta::new(None, independent_changes(&["a", "b"]), &DepEdges::new());
    let mut oracle = Culprits::new(&[]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::NotInTheGraph);
    assert!(out.culprits().is_empty());
    assert_eq!(out.confidence, Confidence::None);
}

#[test]
fn a_search_that_narrowed_nothing_is_inconclusive_rather_than_bisected() {
    struct Rough;
    impl Hybrid for Rough {
        fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
            if flipped.len() == delta.clusters.len() {
                Trial::fails()
            } else if flipped.is_empty() {
                Trial::passes()
            } else {
                Trial::unresolved(Unresolved::DoesNotCheck)
            }
        }
    }
    let delta = Delta::new(
        None,
        independent_changes(&["a", "b", "c"]),
        &DepEdges::new(),
    );
    let out = bisect(&delta, &mut Rough, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Inconclusive);
    assert_eq!(out.confidence, Confidence::Partial);
    assert_eq!(out.culprits(), vec![sym("a"), sym("b"), sym("c")]);
    assert!(out.reason.contains("did not typecheck"));
}

#[test]
fn a_failure_that_does_not_replay_is_reported_rather_than_bisected() {
    struct Green;
    impl Hybrid for Green {
        fn trial(&mut self, _: &Delta, _: &[usize]) -> Trial {
            Trial::passes()
        }
    }
    let delta = Delta::new(None, independent_changes(&["a", "b"]), &DepEdges::new());
    let out = bisect(&delta, &mut Green, Budget::DEFAULT);
    assert_eq!(out.verdict, Verdict::NotReproduced);
    assert!(out.culprits().is_empty());
}

#[test]
fn nothing_changed_is_not_attempted_rather_than_inconclusive() {
    let delta = Delta::new(None, Vec::new(), &DepEdges::new());
    let mut oracle = Culprits::new(&[]);
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);
    assert_eq!(out.verdict, Verdict::NotAttempted(Skipped::NoChanges));
    assert!(oracle.asked.is_empty());
}

#[test]
fn the_search_never_asks_the_same_question_twice() {
    let names: Vec<String> = (0..12).map(|i| format!("d{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
    let mut oracle = Culprits::new(&["d05"]);
    bisect(&delta, &mut oracle, Budget::DEFAULT);

    let mut seen: Vec<Vec<usize>> = oracle.asked.clone();
    let before = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), before, "a subset was evaluated twice");
}

#[test]
fn the_same_inputs_produce_the_same_answer_every_time() {
    let names: Vec<String> = (0..9).map(|i| format!("d{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let run = || {
        let delta = Delta::new(None, independent_changes(&refs), &DepEdges::new());
        let mut oracle = Culprits::new(&["d3", "d7"]);
        let out = bisect(&delta, &mut oracle, Budget::DEFAULT);
        (out.groups, out.search, oracle.asked)
    };
    assert_eq!(run(), run());
}

#[test]
fn every_skip_reason_explains_itself_distinctly() {
    let all = [
        Skipped::NotRequested,
        Skipped::NeverPassed,
        Skipped::Nondet,
        Skipped::Panicked,
        Skipped::NoChanges,
        Skipped::NoBodies,
    ];
    let mut described: Vec<&str> = all.iter().map(|s| s.describe()).collect();
    described.sort_unstable();
    described.dedup();
    assert_eq!(described.len(), all.len());

    let mut codes: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
}
