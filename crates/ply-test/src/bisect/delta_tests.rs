//! Delta construction over real programs: the `Edited`/`Derived` split, the
//! fusion rule, and the classifier's refusals.
//!
//! The search itself is exercised against an oracle; these tests are about what
//! the search is handed, which is where a wrong answer would be silent rather
//! than loud.

use super::classify::{Classify, Unknown};
use super::{
    Baseline, Change, ChangeKind, DefKey, DepEdges, Diff, EraTable, Regression, Renormalizer, diff,
};
use ply_core::CheckOutput;
use ply_hash::{DefHash, HashOutput};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;
use std::collections::BTreeMap;

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
    hashes: HashOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let module = ply_syntax::parse(SourceId(0), src).expect("the fixture must parse");
        let mut program = Program::single(module);
        let resolved = ply_syntax::resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = ply_core::check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        let hashes = ply_hash::hash_program(&program, &resolved, &check)
            .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
            hashes,
        }
    }

    fn renormalizer(&self) -> Renormalizer<'_> {
        let test_keys: Vec<Symbol> = self.check.tests.iter().map(|t| t.key.clone()).collect();
        Renormalizer::new(&self.program, &self.resolved, &self.hashes, &test_keys)
            .expect("index the program")
    }

    /// The closure of `key`, as a pass record would have stored it.
    fn baseline(&self, key: &str) -> Baseline {
        let key = Symbol::new(key);
        let index = self
            .check
            .tests
            .iter()
            .position(|t| t.key == key)
            .expect("a test by that key");
        let mut closure = BTreeMap::new();
        let mut decls = BTreeMap::new();
        for name in self.hashes.closure.get(&key).into_iter().flatten() {
            if let Some(hash) = self.hashes.defs.get(name) {
                closure.insert(name.clone(), *hash);
            }
            if let Some(hash) = self.hashes.decls.get(name) {
                decls.insert(name.clone(), *hash);
            }
        }
        Baseline::with_decls(self.hashes.tests[index], closure, decls)
    }

    fn test_hash(&self, key: &str) -> Option<DefHash> {
        let key = Symbol::new(key);
        let index = self.check.tests.iter().position(|t| t.key == key)?;
        self.hashes.tests.get(index).copied()
    }
}

/// A classifier with the real re-normalizer and no interface evidence, so every
/// edit is fused-eligible. Interface comparison needs the front-end cache, which
/// `StoreClassify` supplies; the split being tested here is the other one.
struct Renormalizing<'a> {
    renormalizer: Renormalizer<'a>,
    table: EraTable,
    independent: bool,
}

impl<'a> Renormalizing<'a> {
    fn new(renormalizer: Renormalizer<'a>, baseline: &Baseline, independent: bool) -> Self {
        let table = renormalizer.era_table(&|key: &DefKey| baseline.hash_of(key));
        Renormalizing {
            renormalizer,
            table,
            independent,
        }
    }
}

impl Classify for Renormalizing<'_> {
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash> {
        self.renormalizer.rehash(key, &self.table)
    }
    fn renormalized_test(&mut self, key: &Symbol) -> Option<DefHash> {
        self.renormalizer.rehash_test(key, &self.table)
    }
    fn interface_stable(&mut self, _: &DefKey, _: DefHash) -> Option<bool> {
        Some(self.independent)
    }
    fn component(&mut self, key: &DefKey) -> Vec<DefKey> {
        self.renormalizer.component_of(key)
    }
    fn baseline_image(&mut self) -> std::collections::BTreeSet<DefHash> {
        self.table.image()
    }
}

fn diff_of(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> Diff {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after.renormalizer(), &baseline, independent);
    let key = Symbol::new(key);
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash(key.as_str()),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let mut edges = DepEdges::new();
    edges.extend_from_hashes(&after.hashes);
    diff(&regression, &mut classify, &edges)
}

fn kind_of(diff: &Diff, name: &str) -> Option<ChangeKind> {
    diff.delta
        .change(&Symbol::new(name))
        .map(|c: &Change| c.kind)
}

const CHAIN: &str = r#"
fn leaf(n: Int) -> Int = n + 1
fn mid(n: Int) -> Int = leaf(n) + 1
fn top(n: Int) -> Int = mid(n) + 1

test "chain" {
  assert_eq(top(1), 4)
}
"#;

/// The whole point of doing this in a content-addressed system: one edit moves
/// three hashes, and only one of them is a change anybody made.
#[test]
fn an_edit_to_a_leaf_leaves_its_dependents_derived() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn leaf(n: Int) -> Int = n + 1",
        "fn leaf(n: Int) -> Int = n + 2",
    ));

    assert_ne!(
        before.hashes.defs.get(&Symbol::new("top")),
        after.hashes.defs.get(&Symbol::new("top"))
    );

    let diff = diff_of(&before, &after, "chain", true);
    assert_eq!(kind_of(&diff, "leaf"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "mid"), Some(ChangeKind::Derived));
    assert_eq!(kind_of(&diff, "top"), Some(ChangeKind::Derived));
    assert!(diff.unclassified.is_empty(), "{:?}", diff.unclassified);

    // Three hashes moved and exactly one is worth a hybrid.
    assert_eq!(diff.delta.candidates(), 1);
    assert_eq!(diff.delta.clusters.len(), 1);
    assert_eq!(diff.delta.clusters[0].members, vec![Symbol::new("leaf")]);
}

#[test]
fn two_edits_are_two_candidates() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(
        &CHAIN
            .replace(
                "fn leaf(n: Int) -> Int = n + 1",
                "fn leaf(n: Int) -> Int = n + 2",
            )
            .replace(
                "fn top(n: Int) -> Int = mid(n) + 1",
                "fn top(n: Int) -> Int = mid(n) + 5",
            ),
    );

    let diff = diff_of(&before, &after, "chain", true);
    assert_eq!(kind_of(&diff, "leaf"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "mid"), Some(ChangeKind::Derived));
    assert_eq!(kind_of(&diff, "top"), Some(ChangeKind::Edited));
    assert_eq!(diff.delta.candidates(), 2);
    assert_eq!(diff.delta.clusters.len(), 2);
}

/// A rename moves no hash, so there is nothing for the delta to explain — the
/// headline invariant, observed from the far end of the pipeline.
#[test]
fn renaming_a_definition_produces_no_change_at_all() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace("leaf", "first"));

    let diff = diff_of(&before, &after, "chain", true);
    assert!(diff.delta.changes.is_empty(), "{:?}", diff.delta.changes);
    assert!(diff.delta.clusters.is_empty());
}

#[test]
fn editing_the_test_body_is_recorded_on_the_test_rather_than_on_a_definition() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace("assert_eq(top(1), 4)", "assert_eq(top(2), 4)"));

    let diff = diff_of(&before, &after, "chain", true);
    let test = diff.delta.test.as_ref().expect("the test itself moved");
    assert_eq!(test.name, Symbol::new("chain"));
    assert_eq!(test.kind, ChangeKind::Edited);
    assert!(diff.delta.changes.is_empty(), "{:?}", diff.delta.changes);
    assert!(!diff.test_unclassified);
}

/// A test whose closure moved has a different hash too, and reading that as an
/// edit to the test would name the one definition nobody touched.
#[test]
fn a_test_whose_closure_moved_is_not_itself_a_change() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn leaf(n: Int) -> Int = n + 1",
        "fn leaf(n: Int) -> Int = n + 2",
    ));

    assert_ne!(before.test_hash("chain"), after.test_hash("chain"));
    let diff = diff_of(&before, &after, "chain", true);
    assert!(diff.delta.test.is_none());
    assert!(!diff.test_unclassified);
}

#[test]
fn an_added_definition_is_a_candidate_and_fuses_with_its_caller() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn mid(n: Int) -> Int = leaf(n) + 1",
        "fn bump(n: Int) -> Int = n\nfn mid(n: Int) -> Int = bump(leaf(n)) + 1",
    ));

    let diff = diff_of(&before, &after, "chain", true);
    assert_eq!(kind_of(&diff, "bump"), Some(ChangeKind::Added));
    assert_eq!(kind_of(&diff, "mid"), Some(ChangeKind::Edited));
    let cluster = diff
        .delta
        .clusters
        .iter()
        .find(|c| c.members.contains(&Symbol::new("bump")))
        .expect("bump is in a cluster");
    assert!(cluster.members.contains(&Symbol::new("mid")));
}

#[test]
fn a_removed_definition_is_a_candidate() {
    let before = Compiled::new(&CHAIN.replace(
        "fn mid(n: Int) -> Int = leaf(n) + 1",
        "fn spare(n: Int) -> Int = n\nfn mid(n: Int) -> Int = spare(leaf(n)) + 1",
    ));
    let after = Compiled::new(CHAIN);

    let diff = diff_of(&before, &after, "chain", true);
    assert_eq!(kind_of(&diff, "spare"), Some(ChangeKind::Removed));
    assert_eq!(kind_of(&diff, "mid"), Some(ChangeKind::Edited));
}

/// Mutual recursion goes through component hashing, which this classifier
/// reproduces; the assertion is that it stays exact rather than degrading.
#[test]
fn a_mutually_recursive_pair_is_classified_rather_than_given_up_on() {
    let src = r#"
fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }
fn parity(n: Int) -> Bool = even(n)

test "parity holds" {
  assert(parity(4))
}
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(&src.replace(
        "fn parity(n: Int) -> Bool = even(n)",
        "fn parity(n: Int) -> Bool = even(n + 2)",
    ));

    let diff = diff_of(&before, &after, "parity holds", true);
    assert_eq!(kind_of(&diff, "parity"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "even"), None, "the pair did not move");
    assert!(diff.unclassified.is_empty(), "{:?}", diff.unclassified);
}

/// Editing one member of a recursive component moves both members' hashes, and
/// neither is derived: the component is the unit of identity.
#[test]
fn editing_one_member_of_a_component_moves_the_whole_component() {
    let src = r#"
fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }

test "parity holds" {
  assert(even(4))
}
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(&src.replace(
        "fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }",
        "fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) && true }",
    ));

    let diff = diff_of(&before, &after, "parity holds", true);
    assert_eq!(kind_of(&diff, "odd"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "even"), Some(ChangeKind::Edited));
}

/// The refusal path. A classifier that cannot decide must widen the search, and
/// must never quietly mark something derived.
#[test]
fn a_classifier_with_no_evidence_calls_everything_edited() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn leaf(n: Int) -> Int = n + 1",
        "fn leaf(n: Int) -> Int = n + 2",
    ));

    let baseline = before.baseline("chain");
    let key = Symbol::new("chain");
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash("chain"),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let diff = diff(&regression, &mut Unknown, &DepEdges::from(&after.hashes));

    assert_eq!(diff.delta.candidates(), 3);
    assert_eq!(diff.unclassified.len(), 3);
    assert!(diff.test_unclassified);
    assert!(
        diff.delta.test.is_none(),
        "an unclassifiable test is not accused"
    );
    // Nothing is independent without an interface to compare, so the three fuse.
    assert_eq!(diff.delta.clusters.len(), 1);
}

/// The witness is what makes a private copy of the hashing algorithm safe. If it
/// ever fails on a program this simple, the copy has drifted and every `Derived`
/// it would have produced is a guess.
#[test]
fn the_renormalizer_reproduces_every_hash_ply_hash_published() {
    for src in [CHAIN, include_str!("../../../../examples/ledger.ply")] {
        let compiled = Compiled::new(src);
        let renormalizer = compiled.renormalizer();
        assert_eq!(
            renormalizer.unwitnessed(),
            0,
            "the re-normalizer disagrees with ply-hash"
        );
        for name in compiled.hashes.defs.keys() {
            assert!(renormalizer.witnessed(name), "{name} is not witnessed");
        }
    }
}

#[test]
fn re_normalizing_against_the_current_table_is_the_identity() {
    let compiled = Compiled::new(CHAIN);
    let renormalizer = compiled.renormalizer();
    let table = renormalizer.era_table(&|key: &DefKey| match key.ns {
        super::Ns::Value => compiled.hashes.defs.get(&key.name).copied(),
        super::Ns::Decl => compiled.hashes.decls.get(&key.name).copied(),
    });
    for (name, hash) in &compiled.hashes.defs {
        let key = DefKey::value(name.clone());
        assert_eq!(renormalizer.rehash(&key, &table), Some(*hash), "{name}");
    }
}

// ------------------------------------------------------- the real classifier

struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> TempRoot {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-bisect-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp root");
        TempRoot(dir)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Files the baseline's interface into the store the way a passing run would
/// have, so `StoreClassify` has both sides to compare.
fn stored(before: &Compiled, names: &[&str]) -> (TempRoot, ply_store::Store) {
    let root = TempRoot::new();
    let mut store = ply_store::Store::open(&root.0).expect("open store");
    for name in names {
        let name = Symbol::new(name);
        let info = &before.check.defs[&name];
        let hash = before.hashes.defs[&name];
        store.put_def(
            hash,
            ply_store::CachedDef::new(info.scheme.clone(), info.footprint.clone()),
        );
    }
    (root, store)
}

const SIGNATURE: &str = r#"
fn scale(n: Int) -> Int = n * 2
fn total(xs: List<Int>) -> Int = fold(xs, 0, |acc, x| acc + scale(x))

test "totals" {
  assert_eq(total([1, 2]), 6)
}
"#;

/// An edit that leaves the published interface alone can be swapped under its
/// callers without any of them noticing, which is exactly the condition under
/// which a hybrid still typechecks.
#[test]
fn an_interface_preserving_edit_is_independent() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let (_root, store) = stored(&before, &["scale", "total"]);

    let baseline = before.baseline("totals");
    let renormalizer = after.renormalizer();
    let mut classify = super::StoreClassify::new(&renormalizer, &baseline, &store, &after.check);

    let scale = Symbol::new("scale");
    assert_eq!(
        classify.interface_stable(&DefKey::value(scale.clone()), before.hashes.defs[&scale]),
        Some(true)
    );
}

/// A signature change is what makes most mixtures ill-typed, and it is the one
/// the fusion rule exists for.
#[test]
fn a_signature_change_is_not_independent() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(
        &SIGNATURE
            .replace(
                "fn scale(n: Int) -> Int = n * 2",
                "fn scale(n: Int, by: Int) -> Int = n * by",
            )
            .replace("acc + scale(x)", "acc + scale(x, 3)"),
    );
    let (_root, store) = stored(&before, &["scale", "total"]);

    let baseline = before.baseline("totals");
    let renormalizer = after.renormalizer();
    let mut classify = super::StoreClassify::new(&renormalizer, &baseline, &store, &after.check);

    let scale = Symbol::new("scale");
    assert_eq!(
        classify.interface_stable(&DefKey::value(scale.clone()), before.hashes.defs[&scale]),
        Some(false)
    );
}

/// A pruned cache costs a fused cluster, not a wrong answer, so the refusal has
/// to be distinguishable from a "yes".
#[test]
fn an_interface_the_store_never_saw_is_a_refusal_rather_than_a_yes() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let root = TempRoot::new();
    let store = ply_store::Store::open(&root.0).expect("open store");

    let baseline = before.baseline("totals");
    let renormalizer = after.renormalizer();
    let mut classify = super::StoreClassify::new(&renormalizer, &baseline, &store, &after.check);

    let scale = Symbol::new("scale");
    assert_eq!(
        classify.interface_stable(&DefKey::value(scale.clone()), before.hashes.defs[&scale]),
        None
    );
}

/// The real classifier, end to end: the store answers the interface question and
/// the re-normalizer answers the edited/derived one.
#[test]
fn the_store_backed_classifier_produces_the_same_split() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let (_root, store) = stored(&before, &["scale", "total"]);

    let baseline = before.baseline("totals");
    let renormalizer = after.renormalizer();
    let mut classify = super::StoreClassify::new(&renormalizer, &baseline, &store, &after.check);

    let key = Symbol::new("totals");
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash("totals"),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let diff = diff(&regression, &mut classify, &DepEdges::from(&after.hashes));

    assert_eq!(kind_of(&diff, "scale"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "total"), Some(ChangeKind::Derived));
    assert!(diff.delta.test.is_none());
    assert_eq!(diff.delta.clusters.len(), 1);
    assert_eq!(diff.delta.clusters[0].members, vec![Symbol::new("scale")]);
}

// ------------------------------------------------------------ across modules

fn compiled_program(modules: &[(&str, &str)]) -> Compiled {
    use ply_syntax::ast::ModuleName;
    let inputs: Vec<(SourceId, ModuleName, &str)> = modules
        .iter()
        .enumerate()
        .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
        .collect();
    let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
    let resolved = ply_syntax::resolve(&mut program)
        .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
    let check = ply_core::check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
    let hashes = ply_hash::hash_program(&program, &resolved, &check)
        .unwrap_or_else(|d| panic!("the fixture must hash: {d:#?}"));
    Compiled {
        program,
        resolved,
        check,
        hashes,
    }
}

const STORE: &str = r#"
pub effect db {
  read get[users](key: Int) -> Int
}

pub fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)
"#;

const APP: &str = r#"
import store

pub fn doubled(k: Int) -> Int = store::lookup(k) * 2

test "doubling" {
  with_cell[users](0) { cell ->
    handle {
      assert_eq(doubled(3), 0)
    } with { store::db.get[users](k) -> cell_get(cell) }
  }
}
"#;

/// Effect slots are a de Bruijn level over the effects a component can reach,
/// and they are computed from the reference graph rather than from any name. A
/// copy that got that wrong would still produce plausible hashes, so the witness
/// has to see a program with an effect and a module boundary in it.
#[test]
fn the_witness_holds_across_a_module_boundary() {
    let compiled = compiled_program(&[("store", STORE), ("app", APP)]);
    let renormalizer = compiled.renormalizer();
    assert_eq!(renormalizer.unwitnessed(), 0);
    assert!(renormalizer.witnessed_test(&Symbol::new("app.doubling")));
}

/// Moving a definition between modules changes no hash, so it must produce no
/// change for a bisection to chase either.
#[test]
fn an_edit_in_one_module_leaves_its_importer_derived() {
    let before = compiled_program(&[("store", STORE), ("app", APP)]);
    let after = compiled_program(&[
        (
            "store",
            &STORE.replace("db.get[users](k)", "db.get[users](k + 1)"),
        ),
        ("app", APP),
    ]);

    let baseline = before.baseline("app.doubling");
    let mut classify = Renormalizing::new(after.renormalizer(), &baseline, true);
    let key = Symbol::new("app.doubling");
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash("app.doubling"),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let diff = diff(&regression, &mut classify, &DepEdges::from(&after.hashes));

    assert_eq!(kind_of(&diff, "store.lookup"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "app.doubled"), Some(ChangeKind::Derived));
    assert_eq!(diff.delta.candidates(), 1);
}
