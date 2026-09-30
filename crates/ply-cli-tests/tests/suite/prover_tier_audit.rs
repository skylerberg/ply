//! What each tier claims about the corpus and about claims chosen to sit at its edges, read from the
//! document `ply prove --json` writes: a certificate's rules, a tier, a gap.

use crate::harness::{json_of, ply, project, repo};
use serde_json::Value;
use std::path::Path;

/// What `ply prove` answers for `path` under the repository, or for the project at `dir` when there
/// is no path, discharged afresh, with `flags` besides.
fn proved_at(dir: &Path, path: Option<&str>, flags: &[&str]) -> Value {
    let mut command = ply(dir);
    command.arg("prove");
    if let Some(path) = path {
        command.arg(path);
    }
    let out = command
        .args(["--no-cache", "--json"])
        .args(flags)
        .output()
        .expect("`ply prove` runs");
    json_of(&out)
}

fn proved(source: &str) -> Value {
    proved_at(project(source).path(), None, &[])
}

fn corpus_report(path: &str, flags: &[&str]) -> Value {
    proved_at(&repo(), Some(path), flags)
}

/// The repository's own claims, and fixtures that sit at each tier's edges.
const CORPUS: &[&str] = &[
    "examples",
    "tests/fixtures/refuted_law.ply",
    "tests/fixtures/vacuous_law.ply",
    "tests/fixtures/obligation_not_discharged.ply",
    "tests/fixtures/concurrency_law_binder.ply",
];

fn obligations(report: &Value) -> &[Value] {
    report["obligations"]
        .as_array()
        .map(Vec::as_slice)
        .expect("an obligation array")
}

#[track_caller]
fn obligation<'v>(report: &'v Value, needle: &str) -> &'v Value {
    obligations(report)
        .iter()
        .find(|o| o["label"].as_str().is_some_and(|l| l.contains(needle)))
        .unwrap_or_else(|| panic!("no obligation labelled `{needle}` in {report}"))
}

fn tier(o: &Value) -> Option<&str> {
    o["tier"].as_str()
}

fn outcome(o: &Value) -> &str {
    o["outcome"].as_str().unwrap_or_default()
}

static NO_FIELDS: Value = Value::Null;

/// The fields of every rule of `o`'s certificate tagged `tag`: `null` for a rule with none.
fn rules<'v>(o: &'v Value, tag: &str) -> Vec<&'v Value> {
    o["certificate"]["rules"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| match rule {
            Value::String(named) if named == tag => Some(&NO_FIELDS),
            Value::Object(fields) => fields.get(tag),
            _ => None,
        })
        .collect()
}

/// A run's document, less the clock.
fn timeless(mut report: Value) -> Value {
    report["duration_ms"] = Value::Null;
    report
}

#[test]
fn the_certificate_audit() {
    let mut proofs = 0;
    for path in CORPUS {
        let report = corpus_report(path, &[]);
        let budget = report["plan"]["prove_budget"]
            .as_u64()
            .expect("the plan names its budget");
        for o in obligations(&report) {
            let certificate = &o["certificate"];
            if certificate.is_null() {
                continue;
            }
            proofs += 1;
            let owner = &o["owner"];
            assert_eq!(
                certificate["guard_satisfiable"], true,
                "{owner} holds a certificate that did not establish its guard"
            );
            assert!(
                certificate["rules"]
                    .as_array()
                    .is_some_and(|rules| !rules.is_empty()),
                "{owner} is proved by no rule at all"
            );
            for unfolded in rules(o, "unfold") {
                assert!(
                    unfolded["depth"]
                        .as_u64()
                        .is_some_and(|d| d <= u64::from(ply_prove::UNFOLD_DEPTH)),
                    "{owner} unfolded past the bound: {unfolded}"
                );
            }
            for searched in rules(o, "exhaustive_interleaving") {
                assert!(
                    searched["interleavings"].as_u64().is_some_and(|n| n > 0),
                    "{owner} is proved by an exhaustive search that ran no interleaving"
                );
                assert_eq!(
                    certificate["sorts"],
                    Value::Array(Vec::new()),
                    "{owner} is a proof about one program, so it has no uninterpreted sorts"
                );
            }
            for inducted in rules(o, "induction") {
                assert!(
                    rules(o, "unfold")
                        .iter()
                        .any(|unfolded| unfolded["def"] == inducted["def"]),
                    "{owner} claims induction over {} without unrolling it",
                    inducted["def"]
                );
            }
            // An enumeration spends a step a point, and anything else at most the prove budget.
            let walked = rules(o, "exhaustive_enumeration")
                .iter()
                .filter_map(|e| e["points"].as_u64())
                .max()
                .unwrap_or(0);
            assert!(
                certificate["steps"]
                    .as_u64()
                    .is_some_and(|steps| steps <= budget.max(walked)),
                "{owner} spent {} steps",
                certificate["steps"]
            );
        }
    }
    assert!(proofs >= 10, "the corpus produced only {proofs} proofs");
}

/// A run is a function of the program and the plan: the worker pool decides nothing, and a
/// refutation shrinks to the same value every time.
#[test]
fn two_runs_over_one_corpus_agree() {
    for path in CORPUS {
        let one = timeless(corpus_report(path, &["--jobs", "1"]));
        let many = timeless(corpus_report(path, &["--jobs", "4"]));
        assert_eq!(one, many, "in {path}");
    }
}

#[test]
fn a_refutation_names_the_value_it_shrank_to() {
    let report = corpus_report("tests/fixtures/refuted_law.ply", &[]);
    let o = obligation(&report, "settling a day's payments drops nothing");
    assert_eq!(
        outcome(o),
        "refuted",
        "the fixture exists to be refuted: {o}"
    );
    assert!(
        o["counterexample"]["bindings"]
            .as_array()
            .is_some_and(|bindings| !bindings.is_empty()),
        "{o}"
    );
}

#[test]
fn a_finite_domain_is_proved_by_covering_it() {
    let report = proved("law \"excluded middle\" forall (b: Bool) { b || !b }\n");
    let o = obligation(&report, "excluded middle");
    assert_eq!(tier(o), Some("proved"), "{o}");
    assert!(
        rules(o, "exhaustive_enumeration")
            .iter()
            .any(|e| e["points"] == 2)
            || !rules(o, "propositional").is_empty(),
        "{o}"
    );
}

#[test]
fn a_ground_law_is_proved_rather_than_exemplified() {
    let report = proved(
        "\
fn stock() -> String = \"abc\"

law \"the stock is three deep\" {
  string_len(stock()) == 3
}
",
    );
    let o = obligation(&report, "the stock is three deep");
    assert_eq!(tier(o), Some("proved"), "{o}");
    assert!(!rules(o, "ground_evaluation").is_empty(), "{o}");
}

/// Stopping the unfolding at a general statement is what induction is for, and there is none here.
#[test]
fn a_recursive_definition_is_never_unfolded() {
    let report = proved(
        "\
fn rev_onto(xs: List<Int>, acc: List<Int>) -> List<Int> =
  match xs {
    [x, ..rest] -> rev_onto(rest, push(acc, x)),
    _ -> acc,
  }

fn rev(xs: List<Int>) -> List<Int> = rev_onto(xs, [])

law \"reverse is an involution\"
  forall (xs: List<Int>) {
    rev(rev(xs)) == xs
  }
",
    );
    assert_eq!(
        tier(obligation(&report, "reverse is an involution")),
        Some("property"),
        "a claim over unbounded data cannot be decided without induction"
    );
}

#[test]
fn a_term_outside_the_fragment_is_never_proved() {
    let report = proved(
        "\
law \"halving and doubling cancel\"
  forall (x: Int) {
    x / 2 * 2 == x
  }
",
    );
    let o = obligation(&report, "halving and doubling cancel");
    assert_eq!(
        outcome(o),
        "refuted",
        "an uninterpreted `/` must not be reasoned about: {o}"
    );
}

#[test]
fn an_uninterpreted_function_closes_under_congruence() {
    let report = proved(
        "\
law \"a pure function is a function\"
  forall (f: (Int) -> Int, x: Int) {
    f(x) == f(x)
  }
",
    );
    let o = obligation(&report, "a pure function is a function");
    assert_eq!(tier(o), Some("proved"), "{o}");
    assert!(!rules(o, "congruence").is_empty(), "{o}");
}

/// Refuted only if compiled code applies the generated functions it is handed; a raise is a gap.
#[test]
fn a_higher_order_law_is_sampled_on_the_tier() {
    let report = proved(
        "\
law \"two functions agree\"
  forall (f: (Int) -> Int, g: (Int) -> Int, x: Int) {
    f(x) == g(x)
  }
",
    );
    let o = obligation(&report, "two functions agree");
    assert_eq!(outcome(o), "refuted", "{o}");
    let first = &o["counterexample"]["bindings"][0]["value"];
    assert!(
        first.as_str().is_some_and(|v| v.starts_with("<fn |")),
        "{o}"
    );
}

#[test]
fn a_proved_polymorphic_law_records_its_sorts() {
    let report = proved(
        "\
fn pair<a>(x: a) -> a = x

law \"identity is identity\"
  forall (x: a) {
    pair(x) == x
  }
",
    );
    let o = obligation(&report, "identity is identity");
    assert_eq!(tier(o), Some("proved"), "{o}");
    assert_eq!(
        o["certificate"]["sorts"],
        serde_json::json!(["a"]),
        "a proof over a type variable names the sort it left uninterpreted as the binder prints it"
    );
}

#[test]
fn a_spent_budget_reports_the_weaker_tier() {
    const LABEL: &str = "below, equal to, or above";
    let dir = project(
        "\
law \"one is below, equal to, or above the other\"
  forall (x: Int, y: Int) {
    (x < y) || (x == y) || (x > y)
  }
",
    );
    let decided = proved_at(dir.path(), None, &[]);
    let o = obligation(&decided, LABEL);
    assert_eq!(
        tier(o),
        Some("proved"),
        "case analysis over the comparisons decides this when the budget allows: {o}"
    );
    assert!(
        o["certificate"]["steps"].as_u64().is_some_and(|s| s > 1),
        "a law a budget of one already settles cannot test what a spent one does: {o}"
    );

    let starved = proved_at(dir.path(), None, &["--prove-budget", "1"]);
    let o = obligation(&starved, LABEL);
    assert_eq!(
        tier(o),
        Some("property"),
        "a spent budget is inconclusive, and inconclusive is the weaker tier: {o}"
    );
}

#[test]
fn an_unsatisfiable_guard_is_vacuous_rather_than_proved() {
    let report = corpus_report("tests/fixtures/vacuous_law.ply", &[]);
    assert!(!obligations(&report).is_empty());
    for o in obligations(&report) {
        assert_eq!(outcome(o), "vacuous", "{o}");
        assert!(tier(o).is_none(), "{o}");
    }
}

/// Checking an `ensures` means calling the definition, and one that performs needs a handler
/// nothing supplies; a spec that raises is not false, so it is neither a refutation nor a hold.
#[test]
fn a_gap_is_reported_as_one_and_carries_no_tier() {
    let report = corpus_report("tests/fixtures/obligation_not_discharged.ply", &[]);
    let recorded = obligation(&report, "recorded");
    assert_eq!(outcome(recorded), "unattempted", "{recorded}");
    assert!(
        recorded["gap"]
            .as_str()
            .is_some_and(|gap| gap.contains("no handler")),
        "{recorded}"
    );
    let share = obligation(&report, "share");
    assert_eq!(outcome(share), "unattempted", "{share}");
    assert!(
        share["gap"]
            .as_str()
            .is_some_and(|gap| gap.starts_with("raised: ") && gap.contains("division by zero")),
        "{share}"
    );
    assert!(tier(recorded).is_none() && tier(share).is_none());
}

#[test]
fn a_ground_concurrency_law_whose_search_is_exhaustive_is_proved() {
    const LABEL: &str = "no interleaving of two guarded settlements";
    let report = corpus_report("examples", &["--filter", LABEL]);
    let o = obligation(&report, LABEL);
    assert_eq!(tier(o), Some("proved"), "{o}");
    assert!(
        !rules(o, "exhaustive_interleaving").is_empty(),
        "an execution-derived proof names the rule that says so: {o}"
    );

    // One interleaving is one concrete case, which is not a coverage claim at all.
    let once = corpus_report("examples", &["--filter", LABEL, "--seed", "7"]);
    assert_eq!(tier(obligation(&once, LABEL)), Some("example"));
}

#[test]
fn a_precondition_alone_is_not_an_obligation() {
    let report = proved(
        "\
fn withdraw(balance: Int, amount: Int) -> Int
  requires amount > 0
= balance - amount
",
    );
    assert!(
        obligations(&report).is_empty(),
        "a `requires` is a filter on a domain, not a claim to discharge: {report}"
    );
}

#[test]
fn each_postcondition_is_discharged_at_its_own_tier() {
    let report = corpus_report("examples/ledger.ply", &[]);
    let tiers: Vec<Option<&str>> = obligations(&report)
        .iter()
        .filter(|o| {
            o["owner"]
                .as_str()
                .is_some_and(|owner| owner.ends_with("ledger.transfer"))
                && o["kind"] == "ensures"
        })
        .map(tier)
        .collect();
    assert_eq!(
        tiers,
        [Some("proved"), Some("proved"), Some("property")],
        "one clause per obligation, or the pair would share the weaker label"
    );
}

#[test]
fn a_total_index_is_proved_where_a_raising_one_is_a_gap() {
    let report = proved(
        "\
fn peek(xs: List<Int>, i: Int) -> Int =
  match list_at(xs, i) { Some(v) -> v, None -> 0 }

fn bpeek(b: Bytes, i: Int) -> Int = bytes_at(b, i)

law \"a total index peeks at every index\"
  forall (xs: List<Int>, i: Int) {
    peek(xs, i) == peek(xs, i)
  }

law \"a raising index does not\"
  forall (b: Bytes, i: Int) {
    bpeek(b, i) == bpeek(b, i)
  }
",
    );
    assert_eq!(
        tier(obligation(&report, "a total index peeks at every index")),
        Some("proved"),
        "an unguarded `list_at` peek is a value, so the claim is a case split over its answer"
    );
    let raising = obligation(&report, "a raising index does not");
    assert_eq!(
        outcome(raising),
        "unattempted",
        "the control must raise, or the arm above is compared with nothing: {raising}"
    );
    assert!(
        raising["gap"]
            .as_str()
            .is_some_and(|gap| gap.starts_with("raised: `bytes_at` index")),
        "the control raises at its index: {raising}"
    );
    assert!(tier(raising).is_none(), "{raising}");
}
