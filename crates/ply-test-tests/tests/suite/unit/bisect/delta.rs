use ply_span::{SourceId, Symbol};
use ply_test::bisect::{
    Baseline, Change, ChangeKind, Classify, DefKey, DepEdges, Diff, Regression, Rehashed,
    StoreClassify, Unknown, diff,
};
use ply_ty::CheckOutput;
use ply_ty::{DefHash, HashOutput};
use std::collections::BTreeMap;

struct Compiled {
    sources: Vec<(String, String)>,
    check: CheckOutput,
    hashes: HashOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        Compiled::of(&[("", src)])
    }

    fn of(modules: &[(&str, &str)]) -> Compiled {
        let sources: Vec<(String, String)> = modules
            .iter()
            .map(|(name, src)| (name.to_string(), src.to_string()))
            .collect();
        let ids: Vec<SourceId> = (0..sources.len()).map(|i| SourceId(i as u32)).collect();
        let front = ply_codegen::c::producer::checked_front(&sources, &ids)
            .unwrap_or_else(|e| panic!("the fixture must check: {e:#}"));
        Compiled {
            sources,
            check: front.check,
            hashes: front.hashes,
        }
    }

    fn rehashed(&self, baseline: &Baseline) -> Rehashed {
        Rehashed::under(&self.sources, baseline)
            .unwrap_or_else(|e| panic!("the port re-hashes a checked program: {e}"))
    }

    fn own_baseline(&self) -> Baseline {
        let defs = self
            .hashes
            .defs
            .iter()
            .map(|(n, h)| (n.clone(), *h))
            .collect();
        let decls = self
            .hashes
            .decls
            .iter()
            .map(|(n, h)| (n.clone(), *h))
            .collect();
        Baseline::with_decls(DefHash([0; 32]), defs, decls)
    }

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

struct Renormalizing {
    rehashed: Rehashed,
    independent: bool,
}

impl Renormalizing {
    fn new(after: &Compiled, baseline: &Baseline, independent: bool) -> Self {
        Renormalizing {
            rehashed: after.rehashed(baseline),
            independent,
        }
    }
}

impl Classify for Renormalizing {
    fn renormalized(&mut self, key: &DefKey) -> Option<DefHash> {
        self.rehashed.rehash(key)
    }
    fn renormalized_test(&mut self, key: &Symbol) -> Option<DefHash> {
        self.rehashed.rehash_test(key)
    }
    fn interface_stable(&mut self, _: &DefKey, _: DefHash) -> Option<bool> {
        Some(self.independent)
    }
    fn component(&mut self, key: &DefKey) -> Vec<DefKey> {
        self.rehashed.component_of(key)
    }
    fn baseline_image(&mut self) -> std::collections::BTreeSet<DefHash> {
        self.rehashed.image()
    }
}

fn diff_of(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> Diff {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after, &baseline, independent);
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

#[track_caller]
fn assert_rehash_is_the_identity(compiled: &Compiled) {
    let rehashed = compiled.rehashed(&compiled.own_baseline());
    for (name, hash) in &compiled.hashes.defs {
        assert_eq!(
            rehashed.rehash(&DefKey::value(name.clone())),
            Some(*hash),
            "{name}"
        );
    }
    for (name, hash) in &compiled.hashes.decls {
        assert_eq!(
            rehashed.rehash(&DefKey::decl(name.clone())),
            Some(*hash),
            "{name}"
        );
    }
    for (test, hash) in compiled.check.tests.iter().zip(&compiled.hashes.tests) {
        assert_eq!(rehashed.rehash_test(&test.key), Some(*hash), "{}", test.key);
    }
}

#[test]
fn re_hashing_against_the_current_table_is_the_identity() {
    for src in [CHAIN, include_str!("../../../../../../examples/ledger.ply")] {
        assert_rehash_is_the_identity(&Compiled::new(src));
    }
}

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

/// Files the baseline's interfaces as a passing run would, so `StoreClassify` has both sides.
fn stored(before: &Compiled, names: &[&str]) -> (TempRoot, ply_store::Store) {
    let root = TempRoot::new();
    let mut store = ply_store::Store::open(&root.0).expect("open store");
    for name in names {
        let name = Symbol::new(name);
        let info = &before.check.defs[&name];
        let hash = before.hashes.defs[&name];
        store.put_def(
            hash,
            ply_store::CachedDef::new(
                info.scheme.clone(),
                info.footprint.clone(),
                info.performed.clone(),
            ),
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

#[test]
fn an_interface_preserving_edit_is_independent() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let (_root, store) = stored(&before, &["scale", "total"]);

    let baseline = before.baseline("totals");
    let mut classify = StoreClassify::new(after.rehashed(&baseline), &store, &after.check);

    let scale = Symbol::new("scale");
    assert_eq!(
        classify.interface_stable(&DefKey::value(scale.clone()), before.hashes.defs[&scale]),
        Some(true)
    );
}

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
    let mut classify = StoreClassify::new(after.rehashed(&baseline), &store, &after.check);

    let scale = Symbol::new("scale");
    assert_eq!(
        classify.interface_stable(&DefKey::value(scale.clone()), before.hashes.defs[&scale]),
        Some(false)
    );
}

#[test]
fn an_interface_the_store_never_saw_is_a_refusal_rather_than_a_yes() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let root = TempRoot::new();
    let store = ply_store::Store::open(&root.0).expect("open store");

    let baseline = before.baseline("totals");
    let mut classify = StoreClassify::new(after.rehashed(&baseline), &store, &after.check);

    let scale = Symbol::new("scale");
    assert_eq!(
        classify.interface_stable(&DefKey::value(scale.clone()), before.hashes.defs[&scale]),
        None
    );
}

#[test]
fn the_store_backed_classifier_produces_the_same_split() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let (_root, store) = stored(&before, &["scale", "total"]);

    let baseline = before.baseline("totals");
    let mut classify = StoreClassify::new(after.rehashed(&baseline), &store, &after.check);

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

/// Effect slots are a de Bruijn level computed from the reference graph, never from a name.
#[test]
fn re_hashing_is_the_identity_across_a_module_boundary() {
    assert_rehash_is_the_identity(&Compiled::of(&[("store", STORE), ("app", APP)]));
}

#[test]
fn an_edit_in_one_module_leaves_its_importer_derived() {
    let before = Compiled::of(&[("store", STORE), ("app", APP)]);
    let after = Compiled::of(&[
        (
            "store",
            &STORE.replace("db.get[users](k)", "db.get[users](k + 1)"),
        ),
        ("app", APP),
    ]);

    let baseline = before.baseline("app.doubling");
    let mut classify = Renormalizing::new(&after, &baseline, true);
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
