use ply_span::{SourceId, Symbol};
use ply_test::bisect::{
    Baseline, ChangeSet, Classify, DefKey, Regression, Rehashed, Row, StoreClassify, Unknown,
    change_set,
};
use ply_ty::CheckOutput;
use ply_ty::{DefHash, HashOutput};
use std::collections::BTreeMap;

struct Compiled {
    sources: Vec<(String, String)>,
    check: CheckOutput,
    hashes: HashOutput,
    packages: Vec<(String, Vec<String>)>,
    mod_pkg: Vec<usize>,
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
            packages: front.packages,
            mod_pkg: front.mod_pkg,
        }
    }

    fn rehashed(&self, baseline: &Baseline) -> Rehashed {
        Rehashed::under(&self.sources, baseline, &self.packages, &self.mod_pkg)
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

/// What the runtime hands the program about one failure: a row of facts per definition either era's
/// closure holds. Which kind of change the facts make is `suite.delta`'s to decide, and its tests pin
/// that; these pin the facts, over real programs.
fn facts_of(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> ChangeSet {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after, &baseline, independent);
    let key = Symbol::new(key);
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash(key.as_str()),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    change_set(&regression, &mut classify)
}

#[track_caller]
fn row<'a>(facts: &'a ChangeSet, key: &DefKey) -> &'a Row {
    facts
        .rows
        .iter()
        .find(|r| &r.key == key)
        .unwrap_or_else(|| panic!("no row for {key:?} in {facts:#?}"))
}

fn value(name: &str) -> DefKey {
    DefKey::value(Symbol::new(name))
}

/// Its body is today's, hashed as the baseline wrote references: what an edit leaves.
fn edited(r: &Row) -> bool {
    r.before != r.after && r.rehashed.is_some() && r.rehashed != r.before
}

/// Its hash moved and its body did not: what an edit beneath it leaves.
fn derived(r: &Row) -> bool {
    r.before != r.after && r.rehashed == r.before
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
fn an_edit_to_a_leaf_leaves_what_references_it_rehashing_to_its_baseline() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn leaf(n: Int) -> Int = n + 1",
        "fn leaf(n: Int) -> Int = n + 2",
    ));

    assert_ne!(
        before.hashes.defs.get(&Symbol::new("top")),
        after.hashes.defs.get(&Symbol::new("top"))
    );

    let facts = facts_of(&before, &after, "chain", true);
    assert!(edited(row(&facts, &value("leaf"))));
    assert!(derived(row(&facts, &value("mid"))));
    assert!(derived(row(&facts, &value("top"))));
    // The mentions a fusion reads, restricted to the closure.
    assert_eq!(
        row(&facts, &value("leaf")).referrers,
        vec![Symbol::new("mid")]
    );
    assert_eq!(
        row(&facts, &value("mid")).referrers,
        vec![Symbol::new("top")]
    );
    // Interfaces are compared only for a body that moved on its own.
    assert_eq!(row(&facts, &value("leaf")).stable, Some(true));
    assert_eq!(row(&facts, &value("mid")).stable, None);
}

#[test]
fn a_renamed_definition_keeps_its_hash_in_the_program() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace("leaf", "first"));

    let facts = facts_of(&before, &after, "chain", true);
    let gone = row(&facts, &value("leaf"));
    assert_eq!(gone.after, None);
    assert!(
        gone.kept,
        "its hash is still in the program, under the new name"
    );
    let arrived = row(&facts, &value("first"));
    assert_eq!(arrived.before, None);
    assert_eq!(
        arrived.after, gone.before,
        "a rename moves a name and no hash"
    );
    for name in ["mid", "top"] {
        let r = row(&facts, &value(name));
        assert_eq!(r.before, r.after, "{name}");
    }
}

#[test]
fn an_edit_to_the_test_body_rehashes_away_from_its_baseline() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace("assert_eq(top(1), 4)", "assert_eq(top(2), 4)"));

    let facts = facts_of(&before, &after, "chain", true);
    assert_eq!(facts.test, Symbol::new("chain"));
    assert_ne!(facts.after, Some(facts.before));
    assert!(facts.rehashed.is_some() && facts.rehashed != Some(facts.before));
    assert!(facts.rows.iter().all(|r| r.before == r.after));
}

#[test]
fn a_test_whose_closure_moved_rehashes_to_its_baseline() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn leaf(n: Int) -> Int = n + 1",
        "fn leaf(n: Int) -> Int = n + 2",
    ));

    assert_ne!(before.test_hash("chain"), after.test_hash("chain"));
    let facts = facts_of(&before, &after, "chain", true);
    assert_eq!(facts.rehashed, Some(facts.before));
}

#[test]
fn an_added_definition_is_named_with_what_mentions_it() {
    let before = Compiled::new(CHAIN);
    let after = Compiled::new(&CHAIN.replace(
        "fn mid(n: Int) -> Int = leaf(n) + 1",
        "fn bump(n: Int) -> Int = n\nfn mid(n: Int) -> Int = bump(leaf(n)) + 1",
    ));

    let facts = facts_of(&before, &after, "chain", true);
    let added = row(&facts, &value("bump"));
    assert_eq!(added.before, None);
    assert!(added.after.is_some());
    assert_eq!(added.referrers, vec![Symbol::new("mid")]);
    assert!(edited(row(&facts, &value("mid"))));
}

#[test]
fn a_removed_definition_is_gone_from_the_program() {
    let before = Compiled::new(&CHAIN.replace(
        "fn mid(n: Int) -> Int = leaf(n) + 1",
        "fn spare(n: Int) -> Int = n\nfn mid(n: Int) -> Int = spare(leaf(n)) + 1",
    ));
    let after = Compiled::new(CHAIN);

    let facts = facts_of(&before, &after, "chain", true);
    let removed = row(&facts, &value("spare"));
    assert_eq!(removed.after, None);
    assert!(!removed.kept && !removed.renamed, "{removed:?}");
    assert!(edited(row(&facts, &value("mid"))));
}

#[test]
fn editing_one_member_of_a_component_moves_the_whole_component_and_names_it() {
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

    let facts = facts_of(&before, &after, "parity holds", true);
    assert!(edited(row(&facts, &value("odd"))));
    assert!(edited(row(&facts, &value("even"))));
    let pair = vec![value("even"), value("odd")];
    assert_eq!(row(&facts, &value("even")).component, pair);
    assert_eq!(row(&facts, &value("odd")).component, pair);
}

#[test]
fn a_classifier_with_no_evidence_answers_nothing() {
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
    let facts = change_set(&regression, &mut Unknown);
    assert_eq!(facts.rehashed, None);
    assert!(
        facts
            .rows
            .iter()
            .all(|r| r.rehashed.is_none() && r.stable.is_none() && r.component.is_empty())
    );
    // The hashes themselves are facts no classifier is needed for.
    assert_eq!(facts.rows.iter().filter(|r| r.before != r.after).count(), 3);
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

/// Files the baseline's interfaces as a passing run would, then the current program's as the CLI
/// does before its tests run, so `StoreClassify` has both sides.
fn stored(before: &Compiled, after: &Compiled) -> (TempRoot, ply_store::Store) {
    let root = TempRoot::new();
    let mut store = ply_store::Store::open(&root.0).expect("open store");
    let file = root.0.join("m.ply");
    crate::fixture::file_interfaces(&mut store, &file, &before.sources);
    crate::fixture::file_interfaces(&mut store, &file, &after.sources);
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
    let (_root, store) = stored(&before, &after);

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
    let (_root, store) = stored(&before, &after);

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
fn the_store_backed_classifier_answers_the_same_facts() {
    let before = Compiled::new(SIGNATURE);
    let after = Compiled::new(&SIGNATURE.replace("n * 2", "n * 3"));
    let (_root, store) = stored(&before, &after);

    let baseline = before.baseline("totals");
    let mut classify = StoreClassify::new(after.rehashed(&baseline), &store, &after.check);

    let key = Symbol::new("totals");
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash("totals"),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let facts = change_set(&regression, &mut classify);

    let scale = row(&facts, &value("scale"));
    assert!(edited(scale));
    assert_eq!(
        scale.stable,
        Some(true),
        "`n * 3` keeps the published interface"
    );
    assert!(derived(row(&facts, &value("total"))));
    assert_eq!(facts.rehashed, Some(facts.before));
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
fn an_edit_in_one_module_leaves_its_importer_rehashing_to_its_baseline() {
    let before = Compiled::of(&[("store", STORE), ("app", APP)]);
    let after = Compiled::of(&[
        (
            "store",
            &STORE.replace("db.get[users](k)", "db.get[users](k + 1)"),
        ),
        ("app", APP),
    ]);

    let facts = facts_of(&before, &after, "app.doubling", true);
    assert!(edited(row(&facts, &value("store.lookup"))));
    assert!(derived(row(&facts, &value("app.doubled"))));
}
