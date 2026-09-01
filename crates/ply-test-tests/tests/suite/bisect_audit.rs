//! An adversarial audit of M5 bisection: inputs chosen to make it name the wrong definition.

use crate::fixture::Compiled;
use ply_hash::{DefHash, HashOutput};
use ply_span::SourceId;
use ply_span::{Span, Symbol};
use ply_test::bisect::{
    Baseline, Budget, ChangeKind, Classify, Confidence, DefKey, Delta, DepEdges, Diff, EraTable,
    FusionReason, Hybrid, Regression, Renormalizer, Skipped, Trial, Unresolved, Verdict, bisect,
    diff,
};
use ply_test::{
    Attribution, CausalSlice, Entered, Event, Evidence, Frame, Options, SliceBuilder, diagnose,
};
use std::collections::BTreeMap;

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

impl Compiled {
    fn renormalizer(&self) -> Renormalizer<'_> {
        let test_keys: Vec<Symbol> = self.check.tests.iter().map(|t| t.key.clone()).collect();
        Renormalizer::new(&self.program, &self.resolved, &self.hashes, &test_keys)
            .expect("index the program")
    }

    /// The closure as a `PassRecord` records it: one hash per program-wide name *per namespace*, so
    /// an edit to a `type` whose name a `fn` also carries is not lost.
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

/// The real re-normalizer with a caller-supplied answer to the interface question, so a case can
/// isolate the `Edited`/`Derived` split from fusion.
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
    let key = sym(key);
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash(key.as_str()),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    diff(&regression, &mut classify, &DepEdges::from(&after.hashes))
}

fn kind_of(diff: &Diff, name: &str) -> Option<ChangeKind> {
    diff.delta.change(&sym(name)).map(|c| c.kind)
}

fn members(diff: &Diff) -> Vec<Vec<String>> {
    diff.delta
        .clusters
        .iter()
        .map(|c| c.members.iter().map(|m| m.to_string()).collect())
        .collect()
}

/// The whole pipeline a failing `ply test` runs, minus the evaluator: no hybrid builder, so only
/// the verdicts that need no mixture are reachable.
fn attribute(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> Attribution {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after.renormalizer(), &baseline, independent);
    let key = sym(key);
    let suspects: Vec<Symbol> = after
        .hashes
        .closure
        .get(&key)
        .into_iter()
        .flatten()
        .filter(|n| **n != key)
        .cloned()
        .collect();
    diagnose(
        Evidence {
            key: &key,
            test_hash: after.test_hash(key.as_str()),
            nondet: false,
            defect: false,
            host: false,
            suspects: &suspects,
            hashes: &after.hashes,
            baseline: Some(&baseline),
            slice: None,
        },
        &Options::default(),
        &DepEdges::from(&after.hashes),
        &mut classify,
        None,
        Skipped::NoHybrids,
    )
}

/// Answers `Fails` from an arbitrary predicate over the flipped names, so a case can model an
/// oracle that is non-monotone, that refuses, or that lies.
struct Oracle<F> {
    decide: F,
    asked: Vec<Vec<Symbol>>,
}

impl<F: FnMut(&[Symbol]) -> Trial> Oracle<F> {
    fn new(decide: F) -> Oracle<F> {
        Oracle {
            decide,
            asked: Vec::new(),
        }
    }
}

impl<F: FnMut(&[Symbol]) -> Trial> Hybrid for Oracle<F> {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
        let names = delta.flipped_names(flipped);
        self.asked.push(names.clone());
        (self.decide)(&names)
    }
}

fn independent_edits(names: &[String]) -> Vec<ply_test::Change> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            ply_test::Change::edited(
                sym(n),
                DefHash([i as u8; 32]),
                DefHash([i as u8 ^ 0x80; 32]),
                true,
            )
        })
        .collect()
}

fn names(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i:04}")).collect()
}

/// Two edits, either of which alone breaks the test.
#[test]
fn either_edit_alone_being_sufficient_yields_one_minimal_culprit() {
    let all = names("d", 6);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("d0000")) || flipped.contains(&sym("d0005")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.confidence, Confidence::Minimal);
    assert_eq!(out.culprits().len(), 1, "{:?}", out.culprits());
    assert!(
        out.culprits() == vec![sym("d0000")] || out.culprits() == vec![sym("d0005")],
        "{:?}",
        out.culprits()
    );
}

/// Two edits that break the test only together, placed on opposite sides of the first partition so
/// a plain binary search would return whichever half it tried first.
#[test]
fn a_pair_that_straddles_the_first_split_is_returned_whole() {
    let all = names("d", 8);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("d0000")) && flipped.contains(&sym("d0007")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.culprits(), vec![sym("d0000"), sym("d0007")]);
    assert_eq!(out.confidence, Confidence::Minimal);
}

/// Neither edit alone is sufficient *and* neither is necessary on its own — a three-way
/// interaction. ddmin is only obliged to return a 1-minimal set; the audit is that whatever it
/// returns really does reproduce and really is 1-minimal against the same oracle.
#[test]
fn a_three_way_interaction_is_returned_as_a_genuinely_one_minimal_set() {
    let all = names("d", 7);
    let culprits = [sym("d0001"), sym("d0003"), sym("d0006")];
    let fails = |flipped: &[Symbol]| culprits.iter().all(|c| flipped.contains(c));
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if fails(flipped) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    let found = out.culprits();
    assert!(
        fails(&found),
        "the reported set does not reproduce: {found:?}"
    );
    for drop in &found {
        let smaller: Vec<Symbol> = found.iter().filter(|n| *n != drop).cloned().collect();
        assert!(
            !fails(&smaller),
            "{drop} could be dropped, so the set is not 1-minimal: {found:?}"
        );
    }
}

/// A thousand changed definitions with one cause.
#[test]
fn a_thousand_candidates_are_narrowed_logarithmically_without_spending_the_budget() {
    let all = names("d", 1024);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    assert_eq!(delta.clusters.len(), 1024, "nothing may cap the candidates");

    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("d0777")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.culprits(), vec![sym("d0777")]);
    assert_eq!(out.confidence, Confidence::Minimal);
    assert!(!out.search.exhausted, "{:?}", out.search);
    // 2·log2(1024) halvings, plus the reproduction trial and the baseline one.
    assert!(out.search.evaluated <= 22, "{:?}", out.search);
}

/// The cause reaches the assertion only through a definition nobody touched.
#[test]
fn a_cause_that_acts_only_through_an_unchanged_definition_is_still_named() {
    let all = names("d", 5);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("d0002")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);
    assert_eq!(out.culprits(), vec![sym("d0002")]);
    assert!(
        !out.culprits().contains(&sym("relay")),
        "an unchanged definition is never a candidate"
    );
}

/// A signature change that makes *every* split ill-typed.
#[test]
fn a_signature_change_that_poisons_every_split_is_fused_before_the_search() {
    let mut changes = independent_edits(&names("d", 3));
    changes.push(ply_test::Change::edited(
        sym("callee"),
        DefHash([9; 32]),
        DefHash([10; 32]),
        false,
    ));
    let mut edges = DepEdges::new();
    edges.add(sym("d0000"), sym("callee"));
    let delta = Delta::new(None, changes, &edges);

    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("callee")) != flipped.contains(&sym("d0000")) {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        }
        if flipped.contains(&sym("callee")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.confidence, Confidence::Fused);
    assert_eq!(out.search.unresolved, 0, "no split was ever built");
    assert_eq!(out.groups, vec![vec![sym("callee"), sym("d0000")]]);
    for asked in &oracle.asked {
        assert_eq!(
            asked.contains(&sym("callee")),
            asked.contains(&sym("d0000")),
            "a hybrid splitting the fused pair was built: {asked:?}"
        );
    }
}

/// A hybrid that cannot be built refuses rather than answers, and the search then keeps everything
/// it could not separate and refuses to call the result minimal.
#[test]
fn an_inseparable_pair_that_refuses_keeps_both_and_drops_to_partial() {
    let all = names("d", 4);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("d0001")) != flipped.contains(&sym("d0002")) {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        }
        if flipped.contains(&sym("d0001")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert!(out.culprits().contains(&sym("d0001")));
    assert_eq!(out.confidence, Confidence::Partial);
    assert!(out.search.unresolved > 0);
}

/// Two members of one strongly connected component are hashed as `blake3(component_hash ‖ index)`,
/// so a body kept at its baseline still names its partner's baseline hash: a mixture that flips one
/// of them alone measures the baseline and passes.
#[test]
fn a_component_no_hybrid_can_split_is_never_offered_to_the_search_split() {
    let changes = independent_edits(&["even".to_string(), "odd".to_string()]);
    let mut edges = DepEdges::new();
    edges.add(sym("even"), sym("odd"));
    edges.add(sym("odd"), sym("even"));
    let component = vec![DefKey::value(sym("even")), DefKey::value(sym("odd"))];
    let delta = Delta::with_components(None, changes, &edges, &[component]);
    assert_eq!(delta.clusters.len(), 1, "the component is one atom");
    assert_eq!(delta.clusters[0].reason, FusionReason::Component);

    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        let both = flipped.contains(&sym("even")) && flipped.contains(&sym("odd"));
        if both {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Sole);
    assert_eq!(out.groups, vec![vec![sym("even"), sym("odd")]]);
    assert_eq!(out.confidence, Confidence::Fused);
    for asked in &oracle.asked {
        assert_eq!(
            asked.contains(&sym("even")),
            asked.contains(&sym("odd")),
            "a hybrid splitting the component was built: {asked:?}"
        );
    }
}

/// The current program replayed green is not evidence about anything, and must never be turned into
/// a culprit.
#[test]
fn a_failure_that_does_not_reproduce_names_nobody() {
    let all = names("d", 4);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|_: &[Symbol]| Trial::passes());
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::NotReproduced);
    assert!(out.culprits().is_empty());
    assert_eq!(out.confidence, Confidence::None);
}

/// A failure nothing in the definition graph explains — a leaked `nondet` effect, the environment,
/// a defect in Ply.
#[test]
fn a_failure_the_baseline_also_shows_is_never_attributed_to_a_change() {
    let all = names("d", 5);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|_: &[Symbol]| Trial::fails());
    let out = bisect(&delta, &mut oracle, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::NotInTheGraph);
    assert!(out.culprits().is_empty());
}

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

/// A 64-deep chain: one edit at the bottom moves 64 hashes and exactly one of them is a change
/// anybody made.
#[test]
fn a_deep_chain_yields_one_candidate_and_sixty_three_derived_ones() {
    let before = Compiled::new(&chain(64, "n + 1"));
    let after = Compiled::new(&chain(64, "n + 2"));

    let diff = diff_of(&before, &after, "m.deep", true);
    assert_eq!(diff.delta.changes.len(), 64);
    assert_eq!(diff.delta.candidates(), 1);
    assert_eq!(kind_of(&diff, "m.f000"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "m.f063"), Some(ChangeKind::Derived));
    assert!(diff.unclassified.is_empty(), "{:?}", diff.unclassified);

    let out = attribute(&before, &after, "m.deep", true);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.bisection.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("m.f000")]);
    assert_eq!(out.bisection.search.evaluated, 0);
}

const HANDLED: &str = r#"
effect db {
  read get[users](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)
fn twice(k: Int) -> Int / {db.read[users]} = lookup(k) + lookup(k)
fn seeded(k: Int) -> Int = handle { twice(k) } with { db.get[users](n) -> n * 10 }

test "handled" {
  assert_eq(seeded(2), 40)
}
"#;

/// A handler clause is part of the definition that carries it, so an edit to the double rather than
/// to any value is attributed to that definition and to nothing else.
#[test]
fn editing_an_effect_handler_names_the_definition_that_carries_it() {
    let before = Compiled::new(HANDLED);
    let after = Compiled::new(&HANDLED.replace("n * 10", "n * 11"));

    let diff = diff_of(&before, &after, "m.handled", true);
    assert_eq!(kind_of(&diff, "m.seeded"), Some(ChangeKind::Edited));
    assert_eq!(
        kind_of(&diff, "m.lookup"),
        None,
        "the performer did not move"
    );
    assert_eq!(kind_of(&diff, "m.twice"), None);
    assert!(diff.unclassified.is_empty());

    let out = attribute(&before, &after, "m.handled", true);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.culprits(), vec![sym("m.seeded")]);
}

/// An effect declaration is nominal, so everything that mentions it can see the move.
#[test]
fn editing_an_effect_declaration_makes_the_declaration_the_candidate() {
    let before = Compiled::new(HANDLED);
    let after = Compiled::new(&HANDLED.replace(
        "read get[users](key: Int) -> Int",
        "read get[users](key: Int) -> Int\n  read peek[users](key: Int) -> Int",
    ));

    let diff = diff_of(&before, &after, "m.handled", true);
    assert_eq!(kind_of(&diff, "m.db"), Some(ChangeKind::Edited));
    for user in ["m.lookup", "m.twice", "m.seeded"] {
        assert_eq!(kind_of(&diff, user), Some(ChangeKind::Derived), "{user}");
    }
    assert_eq!(diff.delta.candidates(), 1);
}

/// Renaming a definition moves no hash — that is the headline invariant — and a rename beside an
/// edit elsewhere in the closure must not change that reading.
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

    let diff = diff_of(&before, &after, "m.chain", true);
    assert_eq!(kind_of(&diff, "m.leaf"), Some(ChangeKind::Edited));
    assert_eq!(
        kind_of(&diff, "m.mid"),
        None,
        "`mid` was renamed, not removed"
    );
    assert_eq!(
        kind_of(&diff, "m.middle"),
        Some(ChangeKind::Derived),
        "and `middle` is that same definition, its hash moved by the edit below it"
    );
    assert_eq!(
        kind_of(&diff, "m.top"),
        Some(ChangeKind::Derived),
        "nobody edited `top`"
    );
    assert_eq!(members(&diff), vec![vec!["m.leaf".to_string()]]);

    let out = attribute(&before, &after, "m.chain", true);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.culprits(), vec![sym("m.leaf")]);
    assert_eq!(
        out.bisection.search.evaluated, 0,
        "a rename beside one edit is still a one-cluster delta"
    );
}

/// Two mutually recursive definitions share a component hash, so editing either moves both —
/// correctly, the component is the unit of identity.
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

    let diff = diff_of(&before, &after, "m.parity", true);
    assert_eq!(kind_of(&diff, "m.even"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "m.odd"), Some(ChangeKind::Edited));
    assert_eq!(
        members(&diff),
        vec![vec!["m.even".to_string(), "m.odd".to_string()]],
        "the component is one atom of the search"
    );
    assert!(
        diff.delta
            .clusters
            .iter()
            .all(|c| c.reason == FusionReason::Component),
        "and says so: {:?}",
        diff.delta.clusters
    );

    let out = attribute(&before, &after, "m.parity", true);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.bisection.confidence, Confidence::Fused);
    assert_eq!(out.culprits(), vec![sym("m.even"), sym("m.odd")]);
    assert!(
        out.bisection.reason.contains("mutually recursive"),
        "the artifact must say why the pair is inseparable: {}",
        out.bisection.reason
    );
}

/// Making the same pair non-independent — which is what `StoreClassify` does whenever the baseline
/// interface is missing — produces the answer the component case wanted all along, for the wrong
/// reason.
#[test]
fn a_recursive_pair_with_no_baseline_interface_fuses_into_the_right_group() {
    let src = r#"
fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }
fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }

test "parity" { assert(even(4)) }
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(&src.replace("{ false }", "{ true }"));

    let diff = diff_of(&before, &after, "m.parity", false);
    assert_eq!(
        members(&diff),
        vec![vec!["m.even".to_string(), "m.odd".to_string()]]
    );

    let out = attribute(&before, &after, "m.parity", false);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.bisection.confidence, Confidence::Fused);
}

const COLLIDE: &str = r#"
type Amount = Cents(Int) | Dollars(Int)
fn Amount(n: Int) -> Int = n + 1
fn use_it(a: Amount) -> Int = match a { Cents(c) -> c, Dollars(d) -> d * 100 }

test "t" { assert_eq(use_it(Cents(5)), 5) }
"#;

/// A `fn`, a `type` and an `effect` may share a name — they are separate namespaces — so a baseline
/// has to record one hash per *namespace*.
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

    let diff = diff_of(&before, &after, "m.t", true);
    assert_eq!(kind_of(&diff, "m.Amount"), Some(ChangeKind::Edited));
    assert_eq!(
        diff.delta
            .change_of(&DefKey::value(name.clone()))
            .map(|c| c.kind),
        None,
        "the function of that name did not change"
    );
    assert_eq!(
        kind_of(&diff, "m.use_it"),
        Some(ChangeKind::Derived),
        "nobody edited `use_it`; it only mentions the type"
    );
    assert!(diff.unclassified.is_empty(), "{:?}", diff.unclassified);

    let out = attribute(&before, &after, "m.t", true);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.bisection.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![name.clone()]);
    let innocent = out
        .suspects
        .iter()
        .find(|s| s.name == sym("m.use_it"))
        .expect("the dependent is still a suspect");
    assert!(!innocent.culprit);
    assert_eq!(innocent.change, Some(ChangeKind::Derived));
}

/// The minimality claim is a claim about the *partition*, so a change the run could not classify
/// costs it: that change entered the search as a candidate on a guess.
#[test]
fn an_unclassified_change_costs_the_minimality_claim() {
    let before = Compiled::new(COLLIDE);
    let after =
        Compiled::new(&COLLIDE.replace("Cents(Int) | Dollars(Int)", "Dollars(Int) | Cents(Int)"));

    let baseline = before.baseline("m.t");
    let key = sym("m.t");
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash("m.t"),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    // `Unknown` is what a pruned front-end cache leaves behind: it can tell nobody apart from a
    // hash that merely moved.
    let diff = diff(
        &regression,
        &mut ply_test::bisect::Unknown,
        &DepEdges::from(&after.hashes),
    );
    assert!(!diff.unclassified.is_empty());
    assert!(diff.delta.unclassified >= diff.unclassified.len());

    let out = bisect(
        &diff.delta,
        &mut ply_test::bisect::NoHybrid,
        Budget::DEFAULT,
    );
    assert!(!out.culprits().is_empty());
    assert_eq!(
        out.confidence,
        Confidence::Partial,
        "a guessed partition is not a minimal one"
    );
}

/// **Documents a defect.**
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

    let diff = diff_of(&before, &after, "m.t", true);
    assert_eq!(
        kind_of(&diff, "m.spare"),
        None,
        "a genuinely added definition is read as a rename of `plain`"
    );
    assert_eq!(kind_of(&diff, "m.use_it"), Some(ChangeKind::Edited));
}

/// **Documents a defect.**
#[test]
fn documents_a_single_unrelated_change_is_named_without_asking_whether_it_matters() {
    let before = Compiled::new(&chain(4, "n + 1"));
    let after = Compiled::new(&chain(4, "n + 2"));

    let out = attribute(&before, &after, "m.deep", true);
    assert_eq!(out.bisection.verdict, Verdict::Sole);
    assert_eq!(out.bisection.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("m.f000")]);
    assert_eq!(
        out.bisection.search.evaluated, 0,
        "nothing was ever run to check that this change is the cause"
    );
}

/// The artifact is diffed against yesterday's, so one failure must render the same bytes twice —
/// over a real program, not just over a synthetic delta.
#[test]
fn two_diagnoses_of_one_real_failure_agree_byte_for_byte() {
    let before = Compiled::new(&chain(16, "n + 1"));
    let after = Compiled::new(&chain(16, "n + 2"));
    let render = || {
        let out = attribute(&before, &after, "m.deep", true);
        ply_test::report::failure_json(&ply_test::Failure {
            name: "deep".to_string(),
            key: sym("m.deep"),
            diagnostic: ply_span::Diagnostic::error(ply_span::codes::ASSERTION_FAILED, "x"),
            defect: false,
            host: false,
            suspects: Vec::new(),
            assertion: None,
            attribution: out,
            seed: None,
            race: None,
        })
        .to_string()
    };
    assert_eq!(render(), render());
}

/// A test whose baseline was never recorded must not be bisected at all, whatever the definition
/// graph looks like.
#[test]
fn a_test_that_never_passed_is_not_bisected_over_a_real_program() {
    let after = Compiled::new(&chain(8, "n + 2"));
    let key = sym("m.deep");
    let suspects: Vec<Symbol> = after.hashes.defs.keys().cloned().collect();
    let out = diagnose(
        Evidence {
            key: &key,
            test_hash: after.test_hash("m.deep"),
            nondet: false,
            defect: false,
            host: false,
            suspects: &suspects,
            hashes: &after.hashes,
            baseline: None,
            slice: None,
        },
        &Options::default(),
        &DepEdges::from(&after.hashes),
        &mut ply_test::bisect::Unknown,
        None,
        Skipped::NoHybrids,
    );

    assert_eq!(
        out.bisection.verdict,
        Verdict::NotAttempted(Skipped::NeverPassed)
    );
    assert!(out.culprits().is_empty());
    assert!(out.suspects.iter().all(|s| s.change.is_none()));
}

fn enter(name: &str) -> Event {
    Event::Enter {
        name: sym(name),
        hash: None,
        call_site: Span::new(SourceId(0), 0, 1),
    }
}

/// Everything the slice names must have run, and the stack must end where the assertion blew up.
#[test]
fn the_slice_names_only_definitions_that_ran_and_ends_at_the_failing_frame() {
    let mut b = SliceBuilder::new();
    for e in [
        enter("outer"),
        enter("helper"),
        Event::Return,
        enter("inner"),
    ] {
        b.record(e);
    }
    b.failed();
    b.record(Event::Return);
    b.record(Event::Return);
    let slice = b.finish(true);

    assert_eq!(slice.path(), vec![&sym("outer"), &sym("inner")]);
    assert!(slice.ran(&sym("helper")));
    assert_eq!(slice.depth_of(&sym("helper")), None);
    assert_eq!(slice.depth_of(&sym("inner")), Some(0));
    assert!(!slice.ran(&sym("never_called")));
    for frame in &slice.stack {
        assert!(slice.ran(&frame.name), "{} is on the stack", frame.name);
    }
}

/// A `Return` with nothing live is a tracer bug, not a user program's; the stack must degrade
/// rather than panic or go negative.
#[test]
fn unbalanced_returns_do_not_corrupt_the_stack() {
    let mut b = SliceBuilder::new();
    b.record(Event::Return);
    b.record(enter("f"));
    b.record(Event::Return);
    b.record(Event::Return);
    b.record(enter("g"));
    b.failed();
    let slice = b.finish(true);
    assert_eq!(slice.path(), vec![&sym("g")]);
}

/// The roster of entered definitions is capped; past the cap a definition that *did* run is simply
/// not recorded.
#[test]
fn a_truncated_trace_never_claims_a_definition_did_not_run() {
    let mut b = SliceBuilder::with_cap(2);
    for name in ["a", "b", "culprit"] {
        b.record(enter(name));
    }
    b.failed();
    let slice = b.finish(true);

    assert!(slice.truncated);
    assert!(
        !slice.ran(&sym("culprit")),
        "it ran; the roster simply forgot it"
    );
    assert_eq!(
        slice.depth_of(&sym("culprit")),
        Some(0),
        "it is on the stack"
    );
    assert_eq!(
        slice.did_run(&sym("culprit")),
        Some(true),
        "it is on the stack"
    );
    assert_eq!(
        slice.did_run(&sym("never_entered")),
        None,
        "a truncated roster cannot rule anything out"
    );

    let mut hashes = HashOutput::default();
    hashes.defs.insert(sym("culprit"), DefHash([7; 32]));
    let mut attribution = Attribution::from_suspects(&[sym("culprit")], &hashes);
    attribution.resolve(ply_test::Bisection::default(), Some(slice));

    assert_eq!(attribution.suspects[0].ran, Some(true));
    assert_eq!(attribution.suspects[0].depth, Some(0));
}

/// An untruncated roster still answers `false`, which is the whole value of the field: it is what
/// lets a consumer stop reading a suspect.
#[test]
fn an_untruncated_trace_still_rules_a_definition_out() {
    let mut b = SliceBuilder::new();
    b.record(enter("a"));
    b.failed();
    let slice = b.finish(true);

    assert!(!slice.truncated);
    assert_eq!(slice.did_run(&sym("b")), Some(false));
}

/// A slice from a run that went green is evidence about a different execution and must not annotate
/// anything.
#[test]
fn a_slice_that_did_not_reproduce_annotates_nothing() {
    let slice = CausalSlice {
        traced: true,
        reproduced: false,
        entered: vec![Entered {
            name: sym("a"),
            hash: None,
            calls: 1,
        }],
        stack: vec![Frame {
            name: sym("a"),
            hash: None,
            call_site: Span::DUMMY,
        }],
        observed: ply_core::Footprint::empty(),
        truncated: false,
    };
    let mut hashes = HashOutput::default();
    hashes.defs.insert(sym("a"), DefHash([1; 32]));
    let mut attribution = Attribution::from_suspects(&[sym("a")], &hashes);
    attribution.resolve(ply_test::Bisection::default(), Some(slice));

    assert_eq!(attribution.suspects[0].ran, None);
    assert_eq!(attribution.suspects[0].depth, None);
}

/// **Documents a defect.**
#[test]
fn documents_a_budget_spent_before_the_first_question_still_reports_bisected() {
    let all = names("d", 5);
    let delta = Delta::new(None, independent_edits(&all), &DepEdges::new());
    let mut oracle = Oracle::new(|flipped: &[Symbol]| {
        if flipped.contains(&sym("d0002")) {
            Trial::fails()
        } else {
            Trial::passes()
        }
    });
    let out = bisect(&delta, &mut oracle, Budget::new(1));

    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.confidence, Confidence::Partial);
    assert!(out.search.exhausted);
    assert_eq!(
        out.search.unresolved, 0,
        "a spent budget is not an unresolved trial"
    );
    assert_eq!(out.culprits().len(), 5, "nothing was narrowed");
    assert!(
        out.reason
            .starts_with("narrowed 5 changed definitions to d0000")
    );
}

/// One cluster beside an edited test is the one case the fast path must not take: the definition
/// that moved may be innocent and `H(∅)` is the question that separates it from the test edit.
#[test]
fn a_single_cluster_beside_an_edited_test_cannot_claim_minimality() {
    let delta = Delta::new(
        Some(ply_test::Change::edited(
            sym("m.t"),
            DefHash([1; 32]),
            DefHash([2; 32]),
            true,
        )),
        independent_edits(&names("d", 1)),
        &DepEdges::new(),
    );
    let out = bisect(&delta, &mut ply_test::bisect::NoHybrid, Budget::DEFAULT);

    assert_eq!(out.verdict, Verdict::Sole);
    assert_eq!(out.confidence, Confidence::Partial);
    assert_eq!(out.search.unresolved, 1);
}
