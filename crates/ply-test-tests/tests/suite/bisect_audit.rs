use crate::fixture::Compiled;
use ply_span::SourceId;
use ply_span::{Span, Symbol};
use ply_test::bisect::{
    Baseline, ChangeKind, Classify, DefKey, DepEdges, Diff, FusionReason, Regression, Rehashed,
    Skipped, diff,
};
use ply_test::{Attribution, CausalSlice, Entered, Event, Frame, SliceBuilder};
use ply_ty::{DefHash, HashOutput};
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

fn diff_of(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> Diff {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after, &baseline, independent);
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

/// What the report does for one failure when the program owns the search: the change set, the
/// annotation, and a verdict that says whose the search was.
fn attribute(before: &Compiled, after: &Compiled, key: &str, independent: bool) -> Attribution {
    let baseline = before.baseline(key);
    let mut classify = Renormalizing::new(after, &baseline, independent);
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
    let regression = Regression {
        key: &key,
        test_hash: after.test_hash(key.as_str()),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let diff = diff(&regression, &mut classify, &DepEdges::from(&after.hashes));
    let mut attribution = Attribution::from_suspects(&suspects, &after.hashes);
    attribution.annotate(&diff.delta);
    attribution.resolve(ply_test::Bisection::not_attempted(Skipped::Delegated), None);
    attribution
}

/// ddmin owes only *a* 1-minimal set, so this checks reproduction and 1-minimality, not which set.

/// A kept member still names its partner's baseline hash, so flipping one alone replays the baseline.

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

    let diff = diff_of(&before, &after, "m.deep", true);
    assert_eq!(diff.delta.changes.len(), 64);
    assert_eq!(diff.delta.candidates(), 1);
    assert_eq!(kind_of(&diff, "m.f000"), Some(ChangeKind::Edited));
    assert_eq!(kind_of(&diff, "m.f063"), Some(ChangeKind::Derived));
    assert!(diff.unclassified.is_empty(), "{:?}", diff.unclassified);

    let out = attribute(&before, &after, "m.deep", true);
    assert_eq!(
        out.suspects[0].name,
        sym("m.f000"),
        "the edited definition ranks above the sixty-three that only moved under it"
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
    assert!(
        out.suspects
            .iter()
            .any(|s| s.name == sym("m.seeded") && s.change == Some(ChangeKind::Edited)),
        "the definition carrying the handler is a ranked suspect: {:?}",
        out.suspects
    );
}

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
    assert_eq!(
        out.bisection.search.evaluated, 0,
        "a rename beside one edit is still a one-cluster delta"
    );
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
}

/// The right answer for the wrong reason: `StoreClassify` fuses whenever the baseline interface is missing.
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
    assert_eq!(
        out.suspects.len(),
        2,
        "both members of the fused component are suspects"
    );
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
    let innocent = out
        .suspects
        .iter()
        .find(|s| s.name == sym("m.use_it"))
        .expect("the dependent is still a suspect");
    assert!(!innocent.culprit);
    assert_eq!(innocent.change, Some(ChangeKind::Derived));
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

    let diff = diff_of(&before, &after, "m.t", true);
    assert_eq!(
        kind_of(&diff, "m.spare"),
        None,
        "a genuinely added definition is read as a rename of `plain`"
    );
    assert_eq!(kind_of(&diff, "m.use_it"), Some(ChangeKind::Edited));
}

#[test]
fn documents_a_single_unrelated_change_is_named_without_asking_whether_it_matters() {
    let before = Compiled::new(&chain(4, "n + 1"));
    let after = Compiled::new(&chain(4, "n + 2"));

    let out = attribute(&before, &after, "m.deep", true);
    assert_eq!(
        out.suspects[0].name,
        sym("m.f000"),
        "the edited definition ranks above the sixty-three that only moved under it"
    );
    assert_eq!(
        out.bisection.search.evaluated, 0,
        "nothing was ever run to check that this change is the cause"
    );
}

#[test]
fn two_diagnoses_of_one_real_failure_agree_byte_for_byte() {
    let before = Compiled::new(&chain(16, "n + 1"));
    let after = Compiled::new(&chain(16, "n + 2"));
    let render = || {
        let out = attribute(&before, &after, "m.deep", true);
        assert_eq!(
            out.suspects[0].name,
            sym("m.f000"),
            "the edited definition ranks above the sixty-three that only moved under it"
        );
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

fn enter(name: &str) -> Event {
    Event::Enter {
        name: sym(name),
        hash: None,
        call_site: Span::new(SourceId(0), 0, 1),
    }
}

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

#[test]
fn an_untruncated_trace_still_rules_a_definition_out() {
    let mut b = SliceBuilder::new();
    b.record(enter("a"));
    b.failed();
    let slice = b.finish(true);

    assert!(!slice.truncated);
    assert_eq!(slice.did_run(&sym("b")), Some(false));
}

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
        observed: ply_ty::Footprint::empty(),
        truncated: false,
    };
    let mut hashes = HashOutput::default();
    hashes.defs.insert(sym("a"), DefHash([1; 32]));
    let mut attribution = Attribution::from_suspects(&[sym("a")], &hashes);
    attribution.resolve(ply_test::Bisection::default(), Some(slice));

    assert_eq!(attribution.suspects[0].ran, None);
    assert_eq!(attribution.suspects[0].depth, None);
}
