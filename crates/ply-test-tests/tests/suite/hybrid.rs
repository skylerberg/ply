use ply_eval::Provider;
use ply_span::{SourceId, Symbol};
use ply_store::body::{BodySet, of_front};
use ply_store::{CachedDef, Outcome, PassRecord, Store};
use ply_test::bisect::{
    Baseline, Budget, Confidence, DepEdges, Regression, Rehashed, Skipped, StoreClassify, Verdict,
    bisect, diff,
};
use ply_test::{BodyHybrid, Signature, hybrid};
use ply_ty::{CheckOutput, HashOutput, ModuleName};
use std::collections::BTreeMap;

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

struct Compiled {
    port: ply_ty::Front,
    check: CheckOutput,
    hashes: HashOutput,
    bodies: BodySet,
    texts: std::collections::HashMap<String, String>,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let port = crate::fixture::port_front(
            &[(ModuleName::from_dotted("m").to_string(), src.to_string())],
            &[SourceId(0)],
        );
        Compiled {
            check: port.check.clone(),
            hashes: port.hashes.clone(),
            bodies: of_front(&port),
            port,
            texts: std::collections::HashMap::from([(
                ModuleName::from_dotted("m").to_string(),
                src.to_string(),
            )]),
        }
    }

    fn sources(&self) -> Vec<(String, String)> {
        self.texts.clone().into_iter().collect()
    }

    fn test_index(&self, key: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.key == sym(key))
            .expect("a test by that key")
    }

    fn baseline(&self, key: &str) -> Baseline {
        let index = self.test_index(key);
        let mut closure = BTreeMap::new();
        let mut decls = BTreeMap::new();
        for name in self.hashes.closure.get(&sym(key)).into_iter().flatten() {
            if let Some(hash) = self.hashes.defs.get(name) {
                closure.insert(name.clone(), *hash);
            }
            if let Some(hash) = self.hashes.decls.get(name) {
                decls.insert(name.clone(), *hash);
            }
        }
        Baseline::with_decls(self.hashes.tests[index], closure, decls)
    }

    /// The signature every hybrid is judged against.
    fn failure(&self, key: &str) -> ply_span::Diagnostic {
        let index = self.test_index(key);
        let mut machine = ply_eval::Machine::new(&self.port);
        let unit = ply_codegen::Unit::over_front(&self.port, self.texts.clone())
            .expect("this host has a C compiler");
        let spec = ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
            ..Default::default()
        };
        machine.set_compiled(unit.attach(&spec));
        machine
            .eval_test(index)
            .expect_err("the fixture must fail as written")
    }
}

struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-hybrid-{tag}-{}-{}",
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

/// The store a passing run leaves: normalized bodies, published interfaces, and the pass record.
fn passed(before: &Compiled, key: &str) -> (TempRoot, Store) {
    let root = TempRoot::new("store");
    let mut store = Store::open(&root.0).expect("open store");
    for (hash, body) in before.bodies.defs() {
        store.put_body(hash, ply_store::DefBody::of(body.clone()));
    }
    for (name, info) in &before.check.defs {
        if let Some(hash) = before.hashes.defs.get(name) {
            store.put_def(
                *hash,
                CachedDef::new(info.scheme.clone(), info.footprint.clone()),
            );
        }
    }
    let baseline = before.baseline(key);
    store.put(baseline.test_hash, Outcome::Pass);
    store.put_pass_record(
        sym(key),
        PassRecord {
            test_hash: baseline.test_hash,
            closure: baseline.closure.clone(),
            decls: baseline.decls.clone(),
        },
    );
    (root, store)
}

/// Everything a failing `ply test` does for one failure, with the real hybrid builder in the loop.
fn narrow(before: &Compiled, after: &Compiled, key: &str) -> ply_test::Bisection {
    let (_root, store) = passed(before, key);
    let baseline = before.baseline(key);
    let rehashed = Rehashed::under(&after.sources(), &baseline)
        .unwrap_or_else(|e| panic!("the port re-hashes a checked program: {e}"));
    let mut classify = StoreClassify::new(rehashed, &store, &after.check);

    let key = sym(key);
    let regression = Regression {
        key: &key,
        test_hash: after
            .hashes
            .tests
            .get(after.test_index(key.as_str()))
            .copied(),
        baseline: &baseline,
        hashes: &after.hashes,
    };
    let diff = diff(&regression, &mut classify, &DepEdges::from(&after.hashes));

    let mixture = hybrid::mixture_for(&after.hashes, &key, &baseline);
    assert!(
        hybrid::bodies_available(&store, &after.bodies, &mixture),
        "the fixture must have every body a mixture needs"
    );
    let test_body = BodyHybrid::test_body(
        &after.bodies,
        after.hashes.tests[after.test_index(key.as_str())],
    )
    .expect("the current test's body");
    let mut builder = BodyHybrid::new(
        &store,
        &after.bodies,
        mixture,
        test_body,
        Signature::of(&after.failure(key.as_str())),
    );
    bisect(&diff.delta, &mut builder, Budget::DEFAULT)
}

const LEDGER: &str = r#"
fn normal_sign(n: Int) -> Int = if n < 0 { 0 - 1 } else { 1 }
fn balance(a: Int, b: Int, c: Int) -> Int = (a + b) + c
fn presented(a: Int, b: Int, c: Int) -> Int = balance(a, b, c) * normal_sign(a)

test "balances" { assert_eq(presented(1, 2, 3), 6) }
"#;

#[test]
fn two_independent_edits_are_narrowed_to_the_one_that_broke_it() {
    let before = Compiled::new(LEDGER);
    let after = Compiled::new(
        &LEDGER
            .replace(
                "if n < 0 { 0 - 1 } else { 1 }",
                "if n < 0 { 0 - 1 } else { 0 - 1 }",
            )
            .replace("(a + b) + c", "a + (b + c)"),
    );

    let out = narrow(&before, &after, "m.balances");
    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.confidence, Confidence::Minimal);
    assert_eq!(out.culprits(), vec![sym("m.normal_sign")]);
    assert!(out.search.evaluated > 0, "{:?}", out.search);
}

#[test]
fn flipping_a_leaf_reaches_the_callers_kept_at_their_baseline() {
    let before = Compiled::new(LEDGER);
    let after = Compiled::new(&LEDGER.replace(
        "if n < 0 { 0 - 1 } else { 1 }",
        "if n < 0 { 0 - 1 } else { 0 - 1 }",
    ));

    let out = narrow(&before, &after, "m.balances");
    assert_eq!(out.verdict, Verdict::Sole);
    assert_eq!(out.culprits(), vec![sym("m.normal_sign")]);
}

const FIVE: &str = r#"
fn a(n: Int) -> Int = n + 1
fn b(n: Int) -> Int = n + 2
fn c(n: Int) -> Int = n + 3
fn d(n: Int) -> Int = n + 4
fn e(n: Int) -> Int = n + 5
fn all(n: Int) -> Int = a(n) + b(n) + c(n) + d(n) + e(n)

test "sums" { assert_eq(all(0), 15) }
"#;

#[test]
fn one_culprit_among_five_edits_is_named_within_the_logarithmic_budget() {
    let before = Compiled::new(FIVE);
    let after = Compiled::new(
        &FIVE
            .replace("fn a(n: Int) -> Int = n + 1", "fn a(n: Int) -> Int = 1 + n")
            .replace("fn b(n: Int) -> Int = n + 2", "fn b(n: Int) -> Int = 2 + n")
            .replace("fn c(n: Int) -> Int = n + 3", "fn c(n: Int) -> Int = n + 9")
            .replace("fn d(n: Int) -> Int = n + 4", "fn d(n: Int) -> Int = 4 + n")
            .replace("fn e(n: Int) -> Int = n + 5", "fn e(n: Int) -> Int = 5 + n"),
    );

    let out = narrow(&before, &after, "m.sums");
    assert_eq!(out.culprits(), vec![sym("m.c")]);
    assert_eq!(out.confidence, Confidence::Minimal);
    assert!(out.search.evaluated <= 12, "{:?}", out.search);
}

const RECURSION: &str = r#"
fn step(n: Int) -> Int = n - 1
fn guard(n: Int) -> Int = if n < 0 { 0 } else { n }
fn countdown(n: Int) -> Int = if n <= 0 { 0 } else { 0 + countdown(step(n)) }
fn total(n: Int) -> Int = countdown(guard(n))

test "terminates" { assert_eq(total(3), 0) }
"#;

#[test]
fn a_regression_that_introduces_runaway_recursion_is_bisected_to_its_culprit() {
    let before = Compiled::new(RECURSION);
    let after = Compiled::new(
        &RECURSION
            .replace(
                "fn step(n: Int) -> Int = n - 1",
                "fn step(n: Int) -> Int = n + 1",
            )
            .replace("if n < 0 { 0 } else { n }", "if n <= 0 { 0 } else { n }"),
    );

    let diagnostic = after.failure("m.terminates");
    assert_eq!(diagnostic.code, ply_span::codes::RUNTIME_ERROR);
    assert!(
        diagnostic.message.contains("recursion limit"),
        "{}",
        diagnostic.message
    );

    let out = narrow(&before, &after, "m.terminates");
    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.culprits(), vec![sym("m.step")]);
    assert!(
        out.search.evaluated > 0,
        "the culprit was named without running a mixture: {:?}",
        out.search
    );
}

#[test]
fn two_edits_that_only_fail_together_are_both_named() {
    let src = r#"
fn flag() -> Bool = true
fn left() -> Int = 3 + 4
fn right() -> Int = 7
fn pick() -> Int = if flag() { left() } else { right() }

test "pick" { assert_eq(pick(), 7) }
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(
        &src.replace("fn flag() -> Bool = true", "fn flag() -> Bool = false")
            .replace("fn right() -> Int = 7", "fn right() -> Int = 8"),
    );

    let out = narrow(&before, &after, "m.pick");
    assert_eq!(out.verdict, Verdict::Bisected);
    assert_eq!(out.culprits(), vec![sym("m.flag"), sym("m.right")]);
}

#[test]
fn an_edited_test_beside_an_edited_definition_names_the_test() {
    let src = r#"
fn scale(n: Int) -> Int = n * 2
fn other(n: Int) -> Int = n + 1

test "doubles" { assert_eq(scale(2) + other(0), 5) }
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(&src.replace("+ other(0), 5", "+ other(0), 9").replace(
        "fn other(n: Int) -> Int = n + 1",
        "fn other(n: Int) -> Int = 1 + n",
    ));

    let out = narrow(&before, &after, "m.doubles");
    assert_eq!(out.verdict, Verdict::TestChanged);
    assert_eq!(out.culprits(), vec![sym("m.doubles")]);
    assert!(
        !out.culprits().contains(&sym("m.other")),
        "the definition that moved beside it is innocent"
    );
}

#[test]
fn a_bisection_records_no_definition_as_seen() {
    let before = Compiled::new(LEDGER);
    let after = Compiled::new(&LEDGER.replace(
        "if n < 0 { 0 - 1 } else { 1 }",
        "if n < 0 { 0 - 1 } else { 0 - 1 }",
    ));

    let (_root, store) = passed(&before, "m.balances");
    let seen_before = store.definitions_len();
    let out = narrow(&before, &after, "m.balances");
    assert!(out.is_conclusive());
    assert_eq!(store.definitions_len(), seen_before);
    for hash in after.hashes.defs.values() {
        assert!(
            !store.knows_definition(*hash),
            "a hybrid vouched for a definition it never proved"
        );
    }
}

/// `H(all)` *is* the current program, so caching its green replay would pass a red test.
#[test]
fn a_replay_that_goes_green_never_caches_a_pass_for_the_failing_test() {
    let src = r#"
fn scale(n: Int) -> Int = n * 2
fn other(n: Int) -> Int = n + 1

test "doubles" { assert_eq(scale(2) + other(0), 5) }
"#;
    let before = Compiled::new(src);
    let after = Compiled::new(&src.replace(
        "fn other(n: Int) -> Int = n + 1",
        "fn other(n: Int) -> Int = n + 3",
    ));

    let index = after.test_index("m.doubles");
    let mut report = ply_test::RunReport {
        engine: ply_test::Engine::Evaluator,
        passed: 0,
        failed: 1,
        cached: 0,
        failures: vec![ply_test::Failure {
            name: "doubles".to_string(),
            key: sym("m.doubles"),
            diagnostic: after.failure("m.doubles"),
            defect: false,
            host: false,
            suspects: vec![sym("m.other")],
            assertion: None,
            attribution: Default::default(),
            seed: None,
            race: None,
        }],
        duration: std::time::Duration::ZERO,
        parallelism: Default::default(),
        results: Vec::new(),
        warnings: Vec::new(),
        simulation: Default::default(),
    };
    let (_root, mut store) = passed(&before, "m.doubles");
    ply_test::diagnose_failures(
        &mut report,
        &after.sources(),
        &after.port,
        &mut store,
        &ply_test::Options::default(),
    );

    let now = after.hashes.tests[index];
    assert!(
        !matches!(store.get(now), Some(Outcome::Pass)),
        "the failing test's own hash was cached green"
    );
}

#[test]
fn a_pruned_body_store_is_reported_rather_than_guessed_around() {
    let before = Compiled::new(LEDGER);
    let root = TempRoot::new("nobodies");
    let store = Store::open(&root.0).expect("open store");
    let baseline = before.baseline("m.balances");
    let mixture = hybrid::mixture_for(&before.hashes, &sym("m.balances"), &baseline);

    assert!(!hybrid::bodies_available(
        &store,
        &BodySet::default(),
        &mixture
    ));
    assert_eq!(
        Skipped::NoBodies.as_str(),
        "no_bodies",
        "the artifact has to name the fixable cause"
    );
}

#[test]
fn two_bisections_of_one_failure_agree() {
    let before = Compiled::new(FIVE);
    let after =
        Compiled::new(&FIVE.replace("fn c(n: Int) -> Int = n + 3", "fn c(n: Int) -> Int = n + 9"));
    let run = || {
        let out = narrow(&before, &after, "m.sums");
        (out.groups, out.search, out.reason)
    };
    assert_eq!(run(), run());
}
