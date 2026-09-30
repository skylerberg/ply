use crate::fixture::Compiled;
use ply_span::Symbol;
use ply_test::bisect::{
    Baseline, ChangeSet, Classify, DefKey, Regression, Rehashed, Row, change_set,
};
use ply_ty::DefHash;
use std::collections::BTreeMap;

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

impl Compiled {
    fn rehashed(&self, baseline: &Baseline) -> Rehashed {
        let mut sources: Vec<(String, String)> = self.texts.clone().into_iter().collect();
        sources.sort();
        Rehashed::under(&sources, baseline, &self.port.packages, &self.port.mod_pkg)
            .unwrap_or_else(|e| panic!("the port re-hashes a checked program: {e}"))
    }

    /// One hash per name *per namespace*, so a `type` and a `fn` sharing a name are both kept.
    fn baseline(&self, key: &str) -> Baseline {
        let key = sym(key);
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
        let index = self.check.tests.iter().position(|t| t.key == sym(key))?;
        self.hashes.tests.get(index).copied()
    }
}

/// A caller-supplied interface answer, so a case isolates the `Edited`/`Derived` split from fusion.
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

/// The facts a failing `ply test` hands the program about one failure. Which kind of change they
/// make is `suite.delta`'s to decide; this audit holds the facts to real programs.
fn facts_of(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> ChangeSet {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after, &baseline, independent);
    let key = sym(key);
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
        .unwrap_or_else(|| panic!("no row for {key:?}"))
}

fn value(name: &str) -> DefKey {
    DefKey::value(sym(name))
}

/// Its body is today's, hashed as the baseline wrote references: what an edit leaves.
fn edited(r: &Row) -> bool {
    r.before != r.after && r.rehashed.is_some() && r.rehashed != r.before
}

/// Its hash moved and its body did not: what an edit beneath it leaves.
fn derived(r: &Row) -> bool {
    r.before != r.after && r.rehashed == r.before
}

fn untouched(r: &Row) -> bool {
    r.before.is_some() && r.before == r.after
}

/// A chain of  definitions, each calling the one below it, so an edit at the leaf reaches
/// every caller through the hashes and only the leaf is a candidate.
fn chain(depth: usize, leaf: &str) -> String {
    let mut src = format!("fn f000(n: Int) -> Int = {leaf}\n");
    for i in 1..depth {
        src.push_str(&format!(
            "fn f{i:03}(n: Int) -> Int = f{:03}(n) + 1\n",
            i - 1
        ));
    }
    src.push_str(&format!(
        "\ntest \"deep\" {{\n  assert_eq(f{:03}(1), {})\n}}\n",
        depth - 1,
        depth
    ));
    src
}

#[test]
fn a_deep_chain_yields_one_candidate_and_sixty_three_derived_ones() {
    let before = Compiled::new(&chain(64, "n + 1"));
    let after = Compiled::new(&chain(64, "n + 2"));

    let facts = facts_of(&before, &after, "m.deep", true);
    assert_eq!(facts.rows.len(), 64);
    assert!(edited(row(&facts, &value("m.f000"))));
    assert_eq!(
        facts.rows.iter().filter(|r| derived(r)).count(),
        63,
        "every caller above the edit only moved under it"
    );
}

const HANDLED: &str = r#"
effect db {
  read get[users](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.get[users]} = db.get[users](k)
fn twice(k: Int) -> Int / {db.get[users]} = lookup(k) + lookup(k)
fn seeded(k: Int) -> Int = handle { twice(k) } with { db.get[users](n) -> n * 10 }

test "handled" {
  assert_eq(seeded(2), 40)
}
"#;

#[test]
fn editing_an_effect_handler_names_the_definition_that_carries_it() {
    let before = Compiled::new(HANDLED);
    let after = Compiled::new(&HANDLED.replace("n * 10", "n * 11"));

    let facts = facts_of(&before, &after, "m.handled", true);
    assert!(edited(row(&facts, &value("m.seeded"))));
    assert!(
        untouched(row(&facts, &value("m.lookup"))),
        "the performer did not move"
    );
    assert!(untouched(row(&facts, &value("m.twice"))));
}

#[test]
fn editing_an_effect_declaration_makes_the_declaration_the_candidate() {
    let before = Compiled::new(HANDLED);
    let after = Compiled::new(&HANDLED.replace(
        "read get[users](key: Int) -> Int",
        "read get[users](key: Int) -> Int\n  read peek[users](key: Int) -> Int",
    ));

    let facts = facts_of(&before, &after, "m.handled", true);
    assert!(edited(row(&facts, &DefKey::decl(sym("m.db")))));
    for user in ["m.lookup", "m.twice", "m.seeded"] {
        assert!(derived(row(&facts, &value(user))), "{user}");
    }
}

#[test]
fn a_rename_beside_an_edit_leaves_untouched_callers_derived() {
    let before = Compiled::new(
        r#"
fn leaf(n: Int) -> Int = n + 1
fn mid(n: Int) -> Int = leaf(n) + 1
fn top(n: Int) -> Int = mid(n) + 1

test "chain" { assert_eq(top(1), 4) }
"#,
    );
    let after = Compiled::new(
        r#"
fn leaf(n: Int) -> Int = n + 2
fn middle(n: Int) -> Int = leaf(n) + 1
fn top(n: Int) -> Int = middle(n) + 1

test "chain" { assert_eq(top(1), 4) }
"#,
    );

    let facts = facts_of(&before, &after, "m.chain", true);
    assert!(edited(row(&facts, &value("m.leaf"))));
    let mid = row(&facts, &value("m.mid"));
    assert_eq!(mid.after, None);
    assert!(
        mid.renamed,
        "`mid` was renamed, not removed: the program re-normalizes to its baseline hash"
    );
    let middle = row(&facts, &value("m.middle"));
    assert_eq!(middle.before, None);
    assert_eq!(
        middle.rehashed, mid.before,
        "and `middle` is that same definition, its hash moved by the edit below it"
    );
    assert!(derived(row(&facts, &value("m.top"))), "nobody edited `top`");
}

#[test]
fn a_recursive_component_is_fused_because_no_hybrid_can_separate_it() {
    let src = r#"
fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }

test "parity" { assert(even(4)) }
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(&src.replace(
        "fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }",
        "fn odd(n: Int) -> Bool = if n == 0 { true } else { even(n - 1) }",
    ));

    let facts = facts_of(&before, &after, "m.parity", true);
    assert!(edited(row(&facts, &value("m.even"))));
    assert!(edited(row(&facts, &value("m.odd"))));
    let pair = vec![value("m.even"), value("m.odd")];
    assert_eq!(
        row(&facts, &value("m.even")).component,
        pair,
        "the component is named, so the search can take it as one atom"
    );
    assert_eq!(row(&facts, &value("m.odd")).component, pair);
}

const COLLIDE: &str = r#"
type Amount = Cents(Int) | Dollars(Int)
fn Amount(n: Int) -> Int = n + 1
fn use_it(a: Amount) -> Int = match a { Cents(c) -> c, Dollars(d) -> d * 100 }

test "t" { assert_eq(use_it(Cents(5)), 5) }
"#;

#[test]
fn a_name_shared_by_a_fn_and_a_type_still_names_the_edited_one() {
    let before = Compiled::new(COLLIDE);
    let after =
        Compiled::new(&COLLIDE.replace("Cents(Int) | Dollars(Int)", "Dollars(Int) | Cents(Int)"));

    let name = sym("m.Amount");
    assert_ne!(
        before.hashes.decls.get(&name),
        after.hashes.decls.get(&name),
        "the type is what moved"
    );
    assert_eq!(
        before.hashes.defs.get(&name),
        after.hashes.defs.get(&name),
        "the function of the same name did not"
    );
    let baseline = before.baseline("m.t");
    assert_eq!(
        baseline.hash_of(&DefKey::decl(name.clone())),
        before.hashes.decls.get(&name).copied(),
        "the pass record keeps both"
    );
    assert_eq!(
        baseline.hash_of(&DefKey::value(name.clone())),
        before.hashes.defs.get(&name).copied()
    );

    let facts = facts_of(&before, &after, "m.t", true);
    assert!(edited(row(&facts, &DefKey::decl(name.clone()))));
    assert!(
        untouched(row(&facts, &DefKey::value(name.clone()))),
        "the function of that name did not change"
    );
    assert!(
        derived(row(&facts, &value("m.use_it"))),
        "nobody edited `use_it`; it only mentions the type"
    );
}

#[test]
fn documents_an_added_definition_is_suppressed_when_its_body_matches_a_baseline_one() {
    let before = Compiled::new(
        r#"
fn plain(n: Int) -> Int = n + 1
fn use_it(n: Int) -> Int = plain(n)

test "t" { assert_eq(use_it(1), 2) }
"#,
    );
    let after = Compiled::new(
        r#"
fn plain(n: Int) -> Int = n + 1
fn spare(n: Int) -> Int = n + 1
fn use_it(n: Int) -> Int = plain(n) + spare(n)

test "t" { assert_eq(use_it(1), 2) }
"#,
    );

    assert!(after.hashes.defs.contains_key(&sym("m.spare")));
    assert!(before.baseline("m.t").hash(&sym("m.spare")).is_none());

    let facts = facts_of(&before, &after, "m.t", true);
    assert_eq!(
        row(&facts, &value("m.spare")).after,
        row(&facts, &value("m.plain")).before,
        "a genuinely added definition hashes as `plain` did, so it reads as a rename of it"
    );
    assert!(edited(row(&facts, &value("m.use_it"))));
}

#[test]
fn two_readings_of_one_real_failure_agree() {
    let before = Compiled::new(&chain(16, "n + 1"));
    let after = Compiled::new(&chain(16, "n + 2"));
    assert_eq!(
        facts_of(&before, &after, "m.deep", true),
        facts_of(&before, &after, "m.deep", true)
    );
}
