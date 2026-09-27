mod delta;

use ply_span::Symbol;
use ply_test::bisect::{Change, Delta, DepEdges, FusionReason, Skipped};
use ply_ty::DefHash;

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
