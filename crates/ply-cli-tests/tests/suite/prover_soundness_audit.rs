//! No claim is proved that the program does not establish, read where a person reads it: the
//! document `ply prove --json` writes.

use crate::harness::{json_of, ply, project, repo, stderr_of};
use serde_json::Value;
use std::path::Path;

/// What `ply prove` answers for the project at `dir`, discharged afresh, with `flags` besides.
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

/// The obligation whose label holds `needle`.
#[track_caller]
fn obligation<'v>(report: &'v Value, needle: &str) -> &'v Value {
    report["obligations"]
        .as_array()
        .expect("an obligation array")
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

fn gap(o: &Value) -> &str {
    o["gap"].as_str().unwrap_or_default()
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

#[track_caller]
fn never_proved<'v>(report: &'v Value, needle: &str) -> &'v Value {
    let o = obligation(report, needle);
    assert_ne!(tier(o), Some("proved"), "`{needle}` came back proved: {o}");
    o
}

#[test]
fn no_false_claim_is_ever_proved() {
    let report = proved(
        "\
type Color = Red | Green | Blue
type Wrap = W(Color)

fn score(c: Color) -> Int = match c { Red -> 1, Green -> 2, Blue -> 3 }
fn unwrapped(w: Wrap) -> Int = match w { W(Red) -> 1, _ -> 2 }
fn is_red(c: Color) -> Bool = match c { Red -> true, _ -> false }

// Division is uninterpreted: nothing follows from it, not even with a literal divisor.
law \"halving\" forall (x: Int) { x / 2 * 2 == x }
law \"by one\" forall (x: Int) { x / 1 == x }
law \"modulo\" forall (x: Int) { x % 1 == 0 }
// `x * y` with both factors symbolic is not linear arithmetic.
law \"square\" forall (x: Int) { x * x >= x }
// Two distinct uninterpreted symbols are neither provably equal nor provably distinct.
law \"two functions\" forall (f: (Int) -> Int, g: (Int) -> Int, x: Int) { f(x) == g(x) }
// A case analysis over every constructor still evaluates each arm, and one of them is 3.
law \"under three\" forall (c: Color) { score(c) < 3 }
// A nested constructor pattern is not split, so the `match` stays uninterpreted.
law \"always two\" forall (w: Wrap) { unwrapped(w) == 2 }
// `++` is uninterpreted, and congruence over it says nothing about whether it commutes.
law \"concat commutes\" forall (s: String, t: String) { s ++ t == t ++ s }
// List literals are injective in their elements, which is why this is false rather than unknown.
law \"one element\" forall (x: Int, y: Int) { [x] == [y] }
// A record is its fields, so two records agreeing on one field is not extensionality.
law \"half a record\" forall (x: Int, y: Int) { { a: x, b: 0 } == { a: x, b: y } }
// The wildcard arm is reached for two of three constructors.
law \"always red\" forall (c: Color) { is_red(c) }
",
    );
    for needle in [
        "\"halving\"",
        "\"by one\"",
        "\"modulo\"",
        "\"square\"",
        "\"two functions\"",
        "\"under three\"",
        "\"always two\"",
        "\"concat commutes\"",
        "\"one element\"",
        "\"half a record\"",
        "\"always red\"",
    ] {
        let o = never_proved(&report, needle);
        assert!(
            ["refuted", "property", "example", "unattempted"].contains(&outcome(o)),
            "`{needle}` came back {o}"
        );
    }
}

/// Two calls to a definition that performs may answer differently, so they may not share a term.
#[test]
fn an_effectful_call_is_not_a_function_of_its_arguments() {
    let report = proved(
        "\
effect db {
  read get[r](k: Int) -> Int
}

fn fetch(k: Int) -> Int / {db.read[main]} = db.get[main](k)

fn difference(k: Int) -> Int / {db.read[main]}
  ensures result == 0
= fetch(k) - fetch(k)
",
    );
    let o = never_proved(&report, "m.difference");
    assert_eq!(outcome(o), "unattempted", "{o}");
    assert!(
        gap(o).contains("no handler"),
        "an ensures on an effectful definition is a reported gap: {o}"
    );
}

/// A spec expression's row must be empty, so a clause cannot call an effectful definition at all.
#[test]
fn a_clause_that_performs_is_rejected_before_the_prover_sees_it() {
    let dir = project(
        "\
effect db {
  read get[r](k: Int) -> Int
}

fn fetch(k: Int) -> Int / {db.read[main]} = db.get[main](k)

fn echo(k: Int) -> Int / {db.read[main]}
  ensures result == fetch(k)
= fetch(k)
",
    );
    let out = ply(dir.path()).arg("prove").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "a spec may not perform an effect"
    );
    let stderr = stderr_of(&out);
    assert!(stderr.contains(ply_eval::codes::EFFECT_IN_SPEC), "{stderr}");
}

/// An unsatisfiable guard makes `guard ⟹ body` valid and meaningless.
#[test]
fn an_unsatisfiable_guard_is_vacuous_and_never_proved() {
    let report = proved(
        "\
law \"impossible\" forall (x: Int) where x > 0 && x < 0 { x == 5 }
law \"impossible by parity\" forall (x: Int) where x % 2 == 0 && x % 2 == 1 { x == 5 }
",
    );
    for needle in ["\"impossible\"", "\"impossible by parity\""] {
        let o = never_proved(&report, needle);
        assert_eq!(outcome(o), "vacuous", "{o}");
        assert_eq!(o["vacuity"], "the guard admits no value", "{o}");
    }
    assert_eq!(report["exit_code"], 1);
}

#[test]
fn a_binder_of_an_uninhabited_type_never_carries_a_proof() {
    let report = proved(
        "\
type Bad = Wrap(Bad)

law \"a claim about nothing\" forall (b: Bad) { b == b }
",
    );
    let o = never_proved(&report, "a claim about nothing");
    assert_eq!(outcome(o), "unattempted", "{o}");
    assert!(gap(o).contains("can be generated for `b`"), "{o}");
}

#[test]
fn recursion_stops_the_unfolding_and_the_bound_is_named() {
    let report = proved(
        "\
fn sum_to(n: Int) -> Int = if n <= 0 { 0 } else { n + sum_to(n - 1) }

fn twice(x: Int) -> Int = x + x

law \"the sum is triangular\" forall (n: Int) where n > 0 && n < 1000
  { sum_to(n) * 2 == n * n + n }
law \"twice is doubling\" forall (x: Int) where x > -1000 && x < 1000
  { twice(x) == 2 * x }
",
    );
    never_proved(&report, "the sum is triangular");
    let twice = obligation(&report, "twice is doubling");
    assert_eq!(tier(twice), Some("proved"), "{twice}");
    let depths: Vec<u64> = rules(twice, "unfold")
        .iter()
        .map(|fields| {
            fields["depth"]
                .as_u64()
                .expect("an unfolding names its depth")
        })
        .collect();
    assert!(
        !depths.is_empty()
            && depths
                .iter()
                .all(|d| *d <= u64::from(ply_prove::UNFOLD_DEPTH)),
        "{twice}"
    );
}

/// A `simulate` region reached by two tasks, with a handler standing in for the resource, once per
/// `(label, header, spawned)`.
fn concurrency_laws(laws: &[(&str, &str, &str)]) -> String {
    let mut source = String::from(
        "\
effect chan {
  write put[q](v: Int) -> Unit
}

fn worker(v: Int) -> Unit / {chan.write[q], clock.read} {
  let t = clock.now();
  chan.put[q](v)
}
",
    );
    for (label, header, spawned) in laws {
        source.push_str(&format!(
            "
law \"{label}\"{header} {{
  with_cell[q]([]) {{ q -> {{
    handle {{
      simulate {{
        let a = task.spawn(|| worker({spawned}));
        let b = task.spawn(|| worker(2));
        task.join(a);
        task.join(b);
        len(cell_get(q)) == 2
      }}
    }} with {{
      chan.put[q](v) -> cell_set(q, push(cell_get(q), v)),
    }}
  }} }}
}}
"
        ));
    }
    source
}

/// What makes an interleaving proof one: an admitted guard, a search that ran, and a claim about
/// one program rather than a polymorphic one.
#[track_caller]
fn an_interleaving_proof(o: &Value) {
    assert_eq!(tier(o), Some("proved"), "{o}");
    let certificate = &o["certificate"];
    assert_eq!(certificate["guard_satisfiable"], true, "{o}");
    assert_eq!(certificate["sorts"], Value::Array(Vec::new()), "{o}");
    let searched = rules(o, "exhaustive_interleaving");
    assert!(
        searched.len() == 1 && searched[0]["interleavings"].as_u64().is_some_and(|n| n > 0),
        "{o}"
    );
}

#[test]
fn a_concurrency_law_is_proved_only_when_both_domains_were_covered() {
    let report = proved(&concurrency_laws(&[
        ("two writers land twice", "", "1"),
        (
            "two writers over a flip",
            " forall (flip: Bool)",
            "if flip { 1 } else { 3 }",
        ),
        ("two writers over a unit", " forall (u: Unit)", "1"),
    ]));
    let ground = obligation(&report, "two writers land twice");
    an_interleaving_proof(ground);
    assert!(
        rules(ground, "exhaustive_enumeration").is_empty(),
        "a ground law has no value domain, so it names only the interleaving search: {ground}"
    );

    let flipped = obligation(&report, "two writers over a flip");
    an_interleaving_proof(flipped);
    let enumerated = rules(flipped, "exhaustive_enumeration");
    assert!(
        enumerated.len() == 1 && enumerated[0]["points"] == 2,
        "both coverage claims have to be named: {flipped}"
    );

    // A one-point domain was covered too, and the certificate says which.
    let unit = obligation(&report, "two writers over a unit");
    an_interleaving_proof(unit);
    let enumerated = rules(unit, "exhaustive_enumeration");
    assert!(
        enumerated.len() == 1 && enumerated[0]["points"] == 1,
        "{unit}"
    );
}

#[test]
fn an_int_binder_drops_a_concurrency_law_to_property() {
    let report = proved_at(
        &repo(),
        Some("tests/fixtures/concurrency_law_binder.ply"),
        &[],
    );
    let o = obligation(&report, "no interleaving");
    assert_eq!(
        tier(o),
        Some("property"),
        "exhaustive over schedules says nothing about an `Int` binder: {o}"
    );
}

#[test]
fn a_sampled_schedule_search_never_proves() {
    let dir = project(&concurrency_laws(&[("two writers land twice", "", "1")]));
    for flags in [
        &["--sim", "random", "--seeds", "4"][..],
        &["--seed", "7"][..],
    ] {
        let report = proved_at(dir.path(), None, flags);
        never_proved(&report, "two writers land twice");
    }
}

/// A body that entered no `simulate` region emptied a frontier it never filled, so `exhaustive:
/// true` is about nothing.
#[test]
fn a_search_that_reached_no_region_never_proves() {
    let report = proved(
        "\
effect chan {
  write put[q](v: Int) -> Unit
}

fn worker(v: Int) -> Unit / {chan.write[q], clock.read} {
  let t = clock.now();
  chan.put[q](v)
}

law \"sometimes concurrent\" forall (flip: Bool) {
  if flip {
    with_cell[q]([]) { q -> {
      handle {
        simulate {
          let a = task.spawn(|| worker(1));
          let b = task.spawn(|| worker(2));
          task.join(a);
          task.join(b);
          len(cell_get(q)) == 2
        }
      } with {
        chan.put[q](v) -> cell_set(q, push(cell_get(q), v)),
      }
    } }
  } else {
    true
  }
}
",
    );
    never_proved(&report, "sometimes concurrent");
}

#[test]
fn gap_a_definition_that_never_returns_still_carries_a_proof() {
    let report = proved(
        "\
fn spin(x: Int) -> Int = spin(x)

fn go(x: Int) -> Int
  ensures result == spin(x)
= spin(x)

law \"spin is a function\" forall (x: Int) { spin(x) == spin(x) }

law \"a divisor is a function\" forall (a: Int, b: Int) { a / b == a / b }
",
    );
    for needle in ["m.go", "spin is a function", "a divisor is a function"] {
        let o = never_proved(&report, needle);
        assert_eq!(
            outcome(o),
            "unattempted",
            "`{needle}` is a theorem about a total symbol and not about this program: {o}"
        );
    }
    // Nothing is covered, which is the half a reviewer reads: coverage counts only a claim that
    // holds.
    assert_eq!(report["coverage"]["covered"], 0, "{report}");
}

#[test]
fn gap_a_guard_outside_the_generators_range_is_still_decided() {
    let report = proved(
        "law \"a narrow window\" forall (x: Int) where x > 1000000 && x < 1000010 { x > 0 }\n",
    );
    let o = obligation(&report, "a narrow window");
    assert_eq!(
        tier(o),
        Some("proved"),
        "the guard admits nine values and the body is decided over all of them: {o}"
    );
}

/// `==` on `Float` is not reflexive, so congruence closure over it is unsound.
#[test]
fn no_obligation_that_can_reach_a_float_is_proved() {
    let report = proved(
        "\
type Rate = Float
type Box<a> = B(a)
type Money = Cents(Float)
type Row = R({rate: Float})

law \"visible binder\" forall (x: Float) { x == x }
law \"visible literal\" forall (n: Int) { 1.5 == 1.5 }
law \"visible container\" forall (xs: List<Float>) { xs == xs }
law \"visible map\" forall (m: Map<String, Float>) { m == m }
law \"visible alias\" forall (r: Rate) { r == r }
law \"visible parameter\" forall (b: Box<Float>) { b == b }
// These reach a `Float` through a declaration rather than the binder's written type.
law \"hidden in a variant\" forall (m: Money) { m == m }
law \"hidden in a record type\" forall (r: Row) { r == r }
law \"hidden behind a list\" forall (xs: List<Money>) { xs == xs }
law \"hidden behind an option\" forall (o: Option<Money>) { o == o }
",
    );
    let over_claimed: Vec<&str> = report["obligations"]
        .as_array()
        .expect("an obligation array")
        .iter()
        .filter(|o| tier(o) == Some("proved"))
        .map(|o| o["label"].as_str().unwrap_or_default())
        .collect();
    assert!(
        over_claimed.is_empty(),
        "these obligations mention a `Float` and came back proved: {over_claimed:?}"
    );
    assert_eq!(report["summary"]["obligations"], 10, "{report}");
}

#[test]
fn an_ensures_over_a_hidden_float_is_not_proved() {
    let report = proved(
        "pub type Money = Cents(Float)\n\
         pub fn keep(m: Money) -> Money\n\
        \x20 ensures result == m\n\
         = m\n\
         law \"destructured\" forall (m: Money) { match m { Cents(x) -> x == x } }\n",
    );
    never_proved(&report, "m.keep");
    never_proved(&report, "destructured");
}

/// `Decimal`'s `==` is an equivalence relation, so congruence over it is sound.
#[test]
fn decimal_is_congruent_and_never_arithmetic() {
    let report = proved(
        "fn scaled(d: Decimal) -> Decimal = d\n\
         law \"congruence\" forall (x: Decimal) { scaled(x) == scaled(x) }\n\
         law \"additive\" forall (x: Decimal) { x + 0m == x }\n\
         law \"commutes\" forall (x: Decimal, y: Decimal) { x + y == y + x }\n\
         law \"ordered\" forall (x: Decimal) { x >= x }\n\
         law \"scale is value\" forall (n: Int) { 1.5m == 1.50m }\n",
    );
    for needle in ["\"congruence\"", "\"scale is value\""] {
        let o = obligation(&report, needle);
        assert_eq!(tier(o), Some("proved"), "{o}");
    }
    for needle in ["\"additive\"", "\"commutes\"", "\"ordered\""] {
        never_proved(&report, needle);
    }
}

/// There is no theory of arrays: nothing about `map_get` after `map_insert`, or about `map_len`,
/// may be concluded.
#[test]
fn a_map_is_opaque_to_the_prover() {
    let report = proved(
        "law \"reflexive\" forall (m: Map<String, Int>) { m == m }\n\
         law \"get after insert\" forall (m: Map<String, Int>, k: String, v: Int) \
           { map_get(map_insert(m, k, v), k) == Some(v) }\n\
         law \"insert grows\" forall (m: Map<String, Int>, k: String, v: Int) \
           { map_len(map_insert(m, k, v)) == map_len(m) + 1 }\n\
         law \"keys match len\" forall (m: Map<String, Int>) \
           { len(map_keys(m)) == map_len(m) }\n",
    );
    assert_eq!(tier(obligation(&report, "\"reflexive\"")), Some("proved"));
    for needle in ["get after insert", "insert grows", "keys match len"] {
        never_proved(&report, needle);
    }
}

/// Two occurrences of one `Bytes` literal are deliberately not one term, so even the last is not
/// proved.
#[test]
fn the_byte_builtins_are_uninterpreted() {
    let report = proved(
        "law \"index of self\" forall (b: Bytes) { bytes_index_of(b, b) == Some(0) }\n\
         law \"empty needle\" forall (b: Bytes) { bytes_index_of(b, b\"\") == Some(0) }\n\
         law \"starts with itself\" forall (b: Bytes) { bytes_starts_with(b, b) }\n\
         law \"scan is bounded\" forall (b: Bytes, f: Int, s: Bytes, m: Int) \
           { bytes_scan(b, f, s, m) <= f + m }\n\
         law \"split rejoins\" forall (b: Bytes) { len(bytes_split(b, b\",\")) >= 1 }\n\
         law \"two literals\" forall (n: Int) { b\"ab\" == b\"ab\" }\n",
    );
    for needle in [
        "index of self",
        "empty needle",
        "starts with itself",
        "scan is bounded",
        "split rejoins",
        "two literals",
    ] {
        never_proved(&report, needle);
    }
}

/// A derived dictionary is an ordinary record of closures, outside the fragment.
#[test]
fn a_derived_dictionary_carries_no_proof() {
    let report = proved(
        "pub type Point = P(Int)\n\
         derive eq for Point\n\
         derive ord for Point\n\
         law \"eq is reflexive\" forall (p: Point) { (point_eq().eq)(p, p) }\n\
         law \"ord is reflexive\" forall (p: Point) { (point_ord().compare)(p, p) == Equal }\n",
    );
    for needle in ["eq is reflexive", "ord is reflexive"] {
        never_proved(&report, needle);
    }
}
