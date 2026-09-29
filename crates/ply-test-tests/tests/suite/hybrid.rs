use ply_eval::Provider;
use ply_span::{SourceId, Symbol};
use ply_store::body::{BodySet, of_front};
use ply_store::{Outcome, PassRecord, Store};
use ply_test::bisect::{
    Baseline, Delta, DepEdges, Regression, Rehashed, Skipped, StoreClassify, TrialOutcome, diff,
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
        machine.set_compiled(unit.attach());
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
    crate::fixture::file_interfaces(
        &mut store,
        &root.0.join("m.ply"),
        &before.check,
        &before.hashes,
    );
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

/// Everything a failing `ply test` does for one failure, up to the hybrid a program's own search
/// would ask: the real change set, the real mixture, and a builder that runs a mixture of them.
///
/// Choosing which definitions to flip is the program's, so the questions are asked here by name.
fn asked<R>(
    before: &Compiled,
    after: &Compiled,
    key: &str,
    ask: impl FnOnce(&mut BodyHybrid<'_>, &Delta, &Store) -> R,
) -> R {
    let (root, mut store) = passed(before, key);
    // The CLI files the program it loaded before the tests run.
    crate::fixture::file_interfaces(
        &mut store,
        &root.0.join("m.ply"),
        &after.check,
        &after.hashes,
    );
    let baseline = before.baseline(key);
    let rehashed = Rehashed::under(
        &after.sources(),
        &baseline,
        &after.port.packages,
        &after.port.mod_pkg,
    )
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
    ask(&mut builder, &diff.delta, &store)
}

/// The definitions a trial flips: the programs here are all value definitions.
fn keys(names: &[&str]) -> std::collections::BTreeSet<ply_test::bisect::DefKey> {
    names
        .iter()
        .map(|n| ply_test::bisect::DefKey::value(sym(n)))
        .collect()
}

fn flips(builder: &mut BodyHybrid<'_>, names: &[&str]) -> ply_test::bisect::TrialOutcome {
    builder.trial_over(keys(names)).outcome
}

const LEDGER: &str = r#"
fn normal_sign(n: Int) -> Int = if n < 0 { 0 - 1 } else { 1 }
fn balance(a: Int, b: Int, c: Int) -> Int = (a + b) + c
fn presented(a: Int, b: Int, c: Int) -> Int = balance(a, b, c) * normal_sign(a)

test "balances" { assert_eq(presented(1, 2, 3), 6) }
"#;

#[test]
fn flipping_the_definition_that_broke_it_reproduces_the_failure_and_the_other_flip_does_not() {
    let before = Compiled::new(LEDGER);
    let after = Compiled::new(
        &LEDGER
            .replace(
                "if n < 0 { 0 - 1 } else { 1 }",
                "if n < 0 { 0 - 1 } else { 0 - 1 }",
            )
            .replace("(a + b) + c", "a + (b + c)"),
    );

    let outcomes = asked(&before, &after, "m.balances", |builder, _delta, _store| {
        [
            flips(builder, &[]),
            flips(builder, &["m.normal_sign"]),
            flips(builder, &["m.presented"]),
            flips(builder, &["m.normal_sign", "m.presented"]),
        ]
    });
    assert_eq!(
        outcomes,
        [
            // The baseline passed, so the whole set must fail: only then is anything in it a cause.
            TrialOutcome::Passes,
            // The culprit alone, with its caller at the baseline the test last passed at.
            TrialOutcome::Fails,
            // The innocent edit alone changes nothing the test can see.
            TrialOutcome::Passes,
            // And every change at once is the program as it is now.
            TrialOutcome::Fails,
        ],
        "a trial's four answers are what a search reads"
    );
}

#[test]
fn flipping_a_leaf_reaches_the_callers_kept_at_their_baseline() {
    let before = Compiled::new(LEDGER);
    let after = Compiled::new(&LEDGER.replace(
        "if n < 0 { 0 - 1 } else { 1 }",
        "if n < 0 { 0 - 1 } else { 0 - 1 }",
    ));

    let outcome = asked(&before, &after, "m.balances", |builder, _delta, _store| {
        flips(builder, &["m.normal_sign"])
    });
    assert_eq!(
        outcome,
        TrialOutcome::Fails,
        "flipping a leaf reaches the callers kept at their baseline"
    );
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
fn only_the_culprit_among_five_edits_makes_the_test_fail() {
    let before = Compiled::new(FIVE);
    let after = Compiled::new(
        &FIVE
            .replace("fn a(n: Int) -> Int = n + 1", "fn a(n: Int) -> Int = 1 + n")
            .replace("fn b(n: Int) -> Int = n + 2", "fn b(n: Int) -> Int = 2 + n")
            .replace("fn c(n: Int) -> Int = n + 3", "fn c(n: Int) -> Int = n + 9")
            .replace("fn d(n: Int) -> Int = n + 4", "fn d(n: Int) -> Int = 4 + n")
            .replace("fn e(n: Int) -> Int = n + 5", "fn e(n: Int) -> Int = 5 + n"),
    );

    let outcomes = asked(&before, &after, "m.sums", |builder, _delta, _store| {
        [
            flips(builder, &["m.a"]),
            flips(builder, &["m.a", "m.b", "m.d", "m.e"]),
            flips(builder, &["m.c"]),
            flips(builder, &["m.a", "m.c"]),
        ]
    });
    assert_eq!(
        outcomes,
        [
            TrialOutcome::Passes,
            TrialOutcome::Passes,
            TrialOutcome::Fails,
            TrialOutcome::Fails,
        ],
        "the culprit is decisive with or without the edits that only moved"
    );
}

const RECURSION: &str = r#"
fn step(n: Int) -> Int = n - 1
fn guard(n: Int) -> Int = if n < 0 { 0 } else { n }
fn countdown(n: Int) -> Int = if n <= 0 { 0 } else { 0 + countdown(step(n)) }
fn total(n: Int) -> Int = countdown(guard(n))

test "terminates" { assert_eq(total(3), 0) }
"#;

#[test]
fn a_regression_that_introduces_runaway_recursion_fails_alone() {
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

    let outcomes = asked(
        &before,
        &after,
        "m.terminates",
        |builder, _delta, _store| [flips(builder, &["m.step"]), flips(builder, &["m.guard"])],
    );
    assert_eq!(
        outcomes,
        [TrialOutcome::Fails, TrialOutcome::Passes],
        "a mixture that runs away is the failure, and the guard beside it is not"
    );
}

#[test]
fn two_edits_that_only_fail_together_fail_only_together() {
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

    let outcomes = asked(&before, &after, "m.pick", |builder, _delta, _store| {
        [
            flips(builder, &["m.flag"]),
            flips(builder, &["m.right"]),
            flips(builder, &["m.flag", "m.right"]),
        ]
    });
    assert_eq!(
        outcomes,
        [
            TrialOutcome::Passes,
            TrialOutcome::Passes,
            TrialOutcome::Fails,
        ],
        "neither edit alone is a cause, which is why a search may not assume one is"
    );
}

#[test]
fn an_edited_test_beside_an_edited_definition_fails_with_the_baseline_definitions() {
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

    let (outcomes, names_the_test) =
        asked(&before, &after, "m.doubles", |builder, delta, _store| {
            (
                [
                    // Every definition at the baseline it passed at. What still fails is the
                    // test's own text, which every mixture carries: no trial can clear it.
                    flips(builder, &[]),
                    flips(builder, &["m.other"]),
                ],
                // So the change set is what says the test is the cause, not a mixture.
                delta.test.is_some(),
            )
        });
    assert_eq!(outcomes, [TrialOutcome::Fails, TrialOutcome::Fails]);
    assert!(
        names_the_test,
        "the test's own edit is the failure, and the definition that moved beside it is innocent"
    );
}

#[test]
fn a_trial_records_no_definition_as_seen() {
    let before = Compiled::new(LEDGER);
    let after = Compiled::new(&LEDGER.replace(
        "if n < 0 { 0 - 1 } else { 1 }",
        "if n < 0 { 0 - 1 } else { 0 - 1 }",
    ));

    let (outcome, known) = asked(&before, &after, "m.balances", |builder, _delta, store| {
        let outcome = flips(builder, &["m.normal_sign"]);
        // What a mixture proved is the mixture's; a definition it merely *ran* is not a fact
        // about the program, and nothing may be recorded as though it were.
        let known: Vec<bool> = after
            .hashes
            .defs
            .values()
            .map(|h| store.knows_definition(*h))
            .collect();
        (outcome, known)
    });
    assert_eq!(outcome, TrialOutcome::Fails);
    assert!(
        known.iter().all(|k| !k),
        "a hybrid vouched for a definition it never proved"
    );
}

/// A green mixture is a program whose definitions all pass at once, so what it proved may be
/// cached — under the *mixture's* own hash. The failing test's hash is a different test's, and
/// caching a pass for it would turn a red test green.
#[test]
fn a_green_mixture_proves_its_own_hash_and_never_the_failing_tests() {
    let src = r#"
fn scale(n: Int) -> Int = n * 2
fn other(n: Int) -> Int = n + 1

test "doubles" { assert_eq(scale(2) + other(0), 5) }
"#;
    let before = Compiled::new(src);
    // The test's text moves too — a space — so the mixture's own test hash is one no record holds.
    let after = Compiled::new(
        &src.replace(
            "fn other(n: Int) -> Int = n + 1",
            "fn other(n: Int) -> Int = n + 3",
        )
        .replace(
            "test \"doubles\" { assert_eq",
            "test \"doubles\" {  assert_eq",
        ),
    );

    let index = after.test_index("m.doubles");
    let failing = after.hashes.tests[index];
    let (outcome, proved) = asked(&before, &after, "m.doubles", |builder, _delta, _store| {
        // Every definition at the baseline it passed at, and the *test* as it is now: a program
        // no record covers, because that text is new. It has to be run for its hash to be known.
        let outcome = flips(builder, &[]);
        (outcome, builder.take_proved())
    });

    assert_eq!(outcome, TrialOutcome::Passes);
    // A proof is recorded only for a run that was actually made: a mixture whose hash a record
    // already covers proves nothing new, and this one may well have been covered.
    assert!(
        !proved.contains(&failing),
        "the failing test's own hash was offered as a proof"
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
