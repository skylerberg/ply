//! `Float` and `Decimal` through the real binary.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A tiny billing module: the shape `Decimal` exists for.
const BILLING: &str = r#"
pub type Line = { description: String, unit: Decimal, quantity: Int }

pub fn line_total(l: Line) -> Decimal = l.unit * decimal_of_int(l.quantity)

pub fn subtotal(lines: List<Line>) -> Decimal =
  fold(lines, 0m, |acc, l: Line| acc + line_total(l))

pub fn tax(net: Decimal, rate: Decimal) -> Decimal =
  decimal_round(net * rate, 2, HalfEven)

pub fn total(lines: List<Line>, rate: Decimal) -> Decimal =
  subtotal(lines) + tax(subtotal(lines), rate)

fn coffee() -> Line = { description: "coffee", unit: 3.75m, quantity: 4 }
fn tea() -> Line = { description: "tea", unit: 2.20m, quantity: 3 }

test "a subtotal is exact to the cent" {
  assert_eq(subtotal([coffee(), tea()]), 21.60m)
}

test "a tenth plus two tenths is three tenths" {
  assert_eq(0.1m + 0.2m, 0.3m)
}

test "binary floating point is not" {
  assert(0.1 + 0.2 != 0.3)
}

test "a rounded tax names its mode" {
  assert_eq(tax(21.60m, 0.0825m), 1.78m)
}

pub fn main() -> Decimal = total([coffee(), tea()], 0.0825m)
"#;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        stdout_of(output),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// The exit criterion for this half of the milestone, in one run: a program written in `Decimal`
/// checks, runs, and reports a total that lost no cent.
#[test]
fn a_decimal_program_checks_runs_and_tests_clean() {
    let dir = project(BILLING);

    let check = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(check.status.code(), Some(0), "{}", combined(&check));

    let run = ply(dir.path()).arg("run").output().unwrap();
    assert_eq!(run.status.code(), Some(0), "{}", combined(&run));
    assert!(
        stdout_of(&run).contains("23.38"),
        "21.60 plus 8.25% rounded half-to-even is 23.38: {}",
        stdout_of(&run)
    );

    let test = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(test.status.code(), Some(0), "{}", combined(&test));
}

/// The four tests above are cached like any other, so a second run selects none of them.
#[test]
fn decimal_tests_are_cached_exactly_as_any_other_are() {
    let dir = project(BILLING);
    let first = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(first.status.code(), Some(0), "{}", combined(&first));

    let again = ply(dir.path()).args(["test", "--json"]).output().unwrap();
    assert_eq!(again.status.code(), Some(0), "{}", combined(&again));
    let report: Value = serde_json::from_str(&stdout_of(&again))
        .unwrap_or_else(|e| panic!("{e}: {}", stdout_of(&again)));
    assert_eq!(
        report["selection"]["selected"], 0,
        "nothing changed, so nothing re-runs: {report}"
    );
    assert_eq!(report["selection"]["cached"], 4, "{report}");
}

#[test]
fn decimal_division_is_e0209_and_names_decimal_div() {
    let dir = project("pub fn unit(total: Decimal, count: Decimal) -> Decimal = total / count\n");
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    let text = combined(&out);
    assert!(text.contains("E0209"), "{text}");
    assert!(text.contains("decimal_div"), "{text}");
    assert!(
        text.contains("rounding nobody wrote down"),
        "the note is the argument, not decoration: {text}"
    );
}

/// The fixture `tests/fixtures/` owes for `E0209`.
#[test]
fn the_decimal_division_fixture_reports_e0209_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(repo("tests/fixtures/decimal_division.ply"))
        .expect("the fixture is part of the repository");
    std::fs::write(dir.path().join("billing.ply"), source).unwrap();

    let out = ply(dir.path()).args(["check", "--json"]).output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    let report: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    let codes: Vec<&str> = report["diagnostics"]
        .as_array()
        .expect("a diagnostics array")
        .iter()
        .map(|d| d["code"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        codes,
        ["E0209", "E0209"],
        "the fixture has exactly two refusals and no incidental errors: {text}"
    );
}

const FLOAT_LAWS: &str = r#"
pub fn scale(x: Float, k: Float) -> Float = x * k

// False, at exactly one value.
law "a float equals itself" forall (x: Float) { x == x }
law "scaling by one changes nothing" forall (x: Float) { scale(x, 1.0) == x }

// True at every `Float` there is, `NaN` included, and still not provable: `==`
// on the type is not reflexive, so the rules that would decide it are unsound
// over it whatever the sentence happens to say.
law "nothing is both above and below itself" forall (x: Float) { !(x < x && x > x) }
law "a float is a float" forall (x: Float, y: Float) where x < y { !(y < x) }

pub fn main() -> Int = 0
"#;

/// The worst defect this project can ship is a wrong `proved`, and this is the shape most likely to
/// produce one: two false laws that are true everywhere except `NaN`, and two that are true
/// *everywhere*.
#[test]
fn no_float_law_is_ever_reported_proved() {
    let dir = project(FLOAT_LAWS);
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    let text = stdout_of(&out);
    let report: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    let obligations = report["obligations"]
        .as_array()
        .expect("an obligations array");
    assert_eq!(obligations.len(), 4, "{text}");
    for obligation in obligations {
        assert_ne!(
            obligation["tier"].as_str(),
            Some("proved"),
            "a Float obligation was certified: {obligation}"
        );
    }
    assert_eq!(report["summary"]["proved"], 0, "{text}");

    // And the generator earns the refutation rather than the prover guessing at it: the
    // counterexample to `x == x` is the value the type is defined by.
    let refuted = obligations
        .iter()
        .find(|o| {
            o["label"]
                .as_str()
                .unwrap_or_default()
                .contains("equals itself")
        })
        .expect("the reflexivity law is in the report");
    assert_eq!(refuted["outcome"], "refuted", "{refuted}");
    assert_eq!(
        refuted["counterexample"]["bindings"][0]["value"], "NaN",
        "{refuted}"
    );
}

/// The `Decimal` half of the same claim, and the control that shows the refusal above is about
/// `Float` rather than about numerics in general: a congruence over `Decimal` *is* provable,
/// because its `==` is an equivalence relation.
#[test]
fn a_decimal_congruence_is_proved_and_decimal_arithmetic_is_not() {
    let dir = project(
        r#"
        pub fn rounded(d: Decimal) -> Decimal = d

        law "a decimal congruence" forall (x: Decimal) { rounded(x) == rounded(x) }
        law "a decimal zero is additive" forall (x: Decimal) { x + 0m == x }

        pub fn main() -> Int = 0
        "#,
    );
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    let text = stdout_of(&out);
    let report: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    let tier = |label: &str| -> String {
        report["obligations"]
            .as_array()
            .expect("an obligations array")
            .iter()
            .find(|o| o["label"].as_str().unwrap_or_default().contains(label))
            .unwrap_or_else(|| panic!("no obligation labelled `{label}`: {text}"))["tier"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(tier("congruence"), "proved", "{text}");
    assert_ne!(tier("additive"), "proved", "{text}");
}

/// A `Float` in a program still runs, and `-0.0` is the value most
/// likely to make them disagree.
#[test]
fn the_two_engines_agree_over_the_numeric_types() {
    let dir = project(
        r#"
        fn signed_zero() -> Float = 0.0 - 0.0
        fn inverted() -> Float = 1.0 / (0.0 - 0.0)

        test "float arithmetic answers what it should" {
          assert(inverted() == 1.0 / signed_zero());
          assert_eq(0.1m + 0.2m, 0.3m);
          assert(0.1 + 0.2 != 0.3)
        }

        pub fn main() -> Int = 0
        "#,
    );
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
}

/// A `Decimal` overflow is a diagnostic, not a wrap and not a rounding.
#[test]
fn a_decimal_overflow_is_reported_rather_than_absorbed() {
    let dir = project(
        r#"
        fn ceiling() -> Decimal = 79228162514264337593543950335m

        test "an exact type refuses to lose a digit" {
          assert_eq(ceiling() + 1m, ceiling())
        }

        pub fn main() -> Int = 0
        "#,
    );
    let out = ply(dir.path()).arg("test").output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    let text = combined(&out);
    assert!(text.contains("`Decimal` overflow in addition"), "{text}");
    assert!(
        text.contains("will not round to make room"),
        "the note is what separates this from a wrap: {text}"
    );
}
