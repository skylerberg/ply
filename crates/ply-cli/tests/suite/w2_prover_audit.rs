//! An adversarial audit of the prover's W2 surface.

use ply_cli::engine::Prover;
use ply_cli::load::load;
use ply_cli::obligations;
use ply_prove::{Discharge, Gap, ProvePlan, Tier};
use std::path::Path;
use tempfile::TempDir;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

struct Run {
    results: Vec<(ply_prove::Obligation, Discharge)>,
}

impl Run {
    fn of(source: &str) -> Run {
        Run::at(project(source).path())
    }

    fn at(path: &Path) -> Run {
        let loaded = load(path).unwrap_or_else(|e| {
            panic!(
                "the fixture did not compile: {:?}",
                e.diagnostics
                    .iter()
                    .map(|d| format!("{} {}", d.code, d.message))
                    .collect::<Vec<_>>()
            )
        });
        let hashes = loaded.hashes.clone();
        let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
        let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
        let results = collected
            .obligations
            .into_iter()
            .map(|o| {
                let discharge = prover.discharge_with(&o, &ProvePlan::default());
                (o, discharge)
            })
            .collect();
        Run { results }
    }

    #[track_caller]
    fn tier_of(&self, needle: &str) -> Option<Tier> {
        self.results
            .iter()
            .find(|(o, _)| o.owner.as_str().contains(needle))
            .unwrap_or_else(|| {
                panic!(
                    "no obligation named `{needle}` among {:?}",
                    self.results
                        .iter()
                        .map(|(o, _)| o.owner.as_str())
                        .collect::<Vec<_>>()
                )
            })
            .1
            .tier()
    }
}

#[track_caller]
fn never_proved(source: &str, needle: &str) {
    let run = Run::of(source);
    assert_ne!(
        run.tier_of(needle),
        Some(Tier::Proved),
        "`{needle}` came back proved"
    );
}

// --- The refusal `Float` is supposed to get --------------------------------

/// `==` on `Float` is not reflexive, so congruence closure over it is unsound and **no** obligation
/// mentioning one may be proved.
#[test]
fn no_obligation_that_can_reach_a_float_is_proved() {
    let claims: &[(&str, &str)] = &[
        (
            "visible binder",
            "law \"visible binder\" forall (x: Float) { x == x }",
        ),
        (
            "visible literal",
            "law \"visible literal\" forall (n: Int) { 1.5 == 1.5 }",
        ),
        (
            "visible container",
            "law \"visible container\" forall (xs: List<Float>) { xs == xs }",
        ),
        (
            "visible map",
            "law \"visible map\" forall (m: Map<String, Float>) { m == m }",
        ),
        (
            "visible alias",
            "type Rate = Float\nlaw \"visible alias\" forall (r: Rate) { r == r }",
        ),
        (
            "visible parameter",
            "type Box<a> = B(a)\nlaw \"visible parameter\" forall (b: Box<Float>) { b == b }",
        ),
        // The four below reach a `Float` through a *declaration* rather than through the binder's
        // written type.
        (
            "hidden in a variant",
            "type Money = Cents(Float)\nlaw \"hidden in a variant\" forall (m: Money) { m == m }",
        ),
        (
            "hidden in a record type",
            "type Row = R({rate: Float})\n\
             law \"hidden in a record type\" forall (r: Row) { r == r }",
        ),
        (
            "hidden behind a list",
            "type Money = Cents(Float)\n\
             law \"hidden behind a list\" forall (xs: List<Money>) { xs == xs }",
        ),
        (
            "hidden behind an option",
            "type Money = Cents(Float)\n\
             law \"hidden behind an option\" forall (o: Option<Money>) { o == o }",
        ),
    ];
    let mut over_claimed = Vec::new();
    for (needle, source) in claims {
        if Run::of(source).tier_of(needle) == Some(Tier::Proved) {
            over_claimed.push(*needle);
        }
    }
    assert!(
        over_claimed.is_empty(),
        "these obligations mention a `Float` and came back proved: {over_claimed:?}"
    );
}

#[test]
fn a_certificate_over_a_hidden_float_is_refuted_by_sampling() {
    let source = "type Money = Cents(Float)\n\
                  type Row = R({rate: Float})\n\
                  law \"hidden in a variant\" forall (m: Money) { m == m }\n\
                  law \"hidden in a record type\" forall (r: Row) { r == r }\n";
    let dir = project(source);
    let loaded = load(dir.path()).expect("the fixture compiles");
    let hashes = loaded.hashes.clone();
    let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
    let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
    let wide = ProvePlan {
        cases: 1_000,
        roots: (0..8).collect(),
        ..ProvePlan::default()
    };

    let mut lies = Vec::new();
    for obligation in &collected.obligations {
        if prover
            .discharge_with(obligation, &ProvePlan::default())
            .tier()
            != Some(Tier::Proved)
        {
            continue;
        }
        match prover.resample(obligation, &wide) {
            Discharge::Refuted(counterexample) => lies.push(format!(
                "`{}` is proved and sampling refutes it at {:?}",
                obligation.owner,
                counterexample
                    .bindings
                    .iter()
                    .map(|b| format!("{} = {}", b.name, b.rendered))
                    .collect::<Vec<_>>()
            )),
            Discharge::Unattempted(Gap::Raised { diagnostic, .. }) => lies.push(format!(
                "`{}` is proved and sampling raises `{}`",
                obligation.owner, diagnostic.message
            )),
            _ => {}
        }
    }
    assert!(
        lies.is_empty(),
        "a certificate is covering a false claim:\n{}",
        lies.join("\n")
    );
}

/// The same defect on a definition's own `ensures`, and the tightest statement of it available: one
/// file where the spec is `proved` and a law asserting the very thing the proof would have to cover
/// is *refuted*, at the value the proof does not hold at.
#[test]
fn an_ensures_over_a_hidden_float_is_not_proved() {
    let run = Run::of(
        "pub type Money = Cents(Float)\n\
         pub fn keep(m: Money) -> Money\n\
        \x20 ensures result == m\n\
         = m\n\
         law \"destructured\" forall (m: Money) { match m { Cents(x) -> x == x } }\n",
    );
    assert_ne!(
        run.tier_of("keep"),
        Some(Tier::Proved),
        "an `ensures` false at `Cents(NaN)` carries a certificate"
    );
    assert_ne!(
        run.tier_of("destructured"),
        Some(Tier::Proved),
        "the destructured form must stay refused as well"
    );
}

// --- `Decimal`, `Map`, `Bytes`: the refusals that must stay refusals --------

/// `Decimal`'s `==` is an equivalence relation, so congruence over it is sound and reflexivity is a
/// genuine proof.
#[test]
fn decimal_is_congruent_and_never_arithmetic() {
    let run = Run::of(
        "fn scaled(d: Decimal) -> Decimal = d\n\
         law \"congruence\" forall (x: Decimal) { scaled(x) == scaled(x) }\n\
         law \"additive\" forall (x: Decimal) { x + 0m == x }\n\
         law \"commutes\" forall (x: Decimal, y: Decimal) { x + y == y + x }\n\
         law \"ordered\" forall (x: Decimal) { x >= x }\n\
         law \"scale is value\" forall (n: Int) { 1.5m == 1.50m }\n",
    );
    assert_eq!(run.tier_of("congruence"), Some(Tier::Proved));
    assert_eq!(run.tier_of("scale is value"), Some(Tier::Proved));
    for needle in ["additive", "commutes", "ordered"] {
        assert_ne!(
            run.tier_of(needle),
            Some(Tier::Proved),
            "`{needle}` grew the arithmetic fragment"
        );
    }
}

/// A `Map` is opaque: there is no theory of arrays here, so nothing about `map_get` after
/// `map_insert` may be concluded, and neither may anything about `map_len`.
#[test]
fn a_map_is_opaque_to_the_prover() {
    let run = Run::of(
        "law \"reflexive\" forall (m: Map<String, Int>) { m == m }\n\
         law \"get after insert\" forall (m: Map<String, Int>, k: String, v: Int) \
           { map_get(map_insert(m, k, v), k) == Some(v) }\n\
         law \"insert grows\" forall (m: Map<String, Int>, k: String, v: Int) \
           { map_len(map_insert(m, k, v)) == map_len(m) + 1 }\n\
         law \"keys match len\" forall (m: Map<String, Int>) \
           { len(map_keys(m)) == map_len(m) }\n",
    );
    assert_eq!(run.tier_of("reflexive"), Some(Tier::Proved));
    for needle in ["get after insert", "insert grows", "keys match len"] {
        assert_ne!(
            run.tier_of(needle),
            Some(Tier::Proved),
            "`{needle}` was decided by a theory of maps that does not exist"
        );
    }
}

#[test]
fn the_byte_builtins_are_uninterpreted() {
    for (needle, source) in [
        (
            "index of self",
            "law \"index of self\" forall (b: Bytes) { bytes_index_of(b, b) == Some(0) }",
        ),
        (
            "empty needle",
            "law \"empty needle\" forall (b: Bytes) { bytes_index_of(b, b\"\") == Some(0) }",
        ),
        (
            "starts with itself",
            "law \"starts with itself\" forall (b: Bytes) { bytes_starts_with(b, b) }",
        ),
        (
            "scan is bounded",
            "law \"scan is bounded\" forall (b: Bytes, f: Int, s: Bytes, m: Int) \
               { bytes_scan(b, f, s, m) <= f + m }",
        ),
        (
            "split rejoins",
            "law \"split rejoins\" forall (b: Bytes) { len(bytes_split(b, b\",\")) >= 1 }",
        ),
    ] {
        never_proved(source, needle);
    }

    // Two occurrences of one `Bytes` literal are deliberately not one term, so even the trivially
    // true claim about them is `property`.
    never_proved(
        "law \"two literals\" forall (n: Int) { b\"ab\" == b\"ab\" }",
        "two literals",
    );
}

/// A derived dictionary is an ordinary record of closures, so an obligation stated through one is
/// outside the fragment.
#[test]
fn a_derived_dictionary_carries_no_proof() {
    for (needle, source) in [
        (
            "eq is reflexive",
            "pub type Point = P(Int)\n\
             derive eq for Point\n\
             law \"eq is reflexive\" forall (p: Point) { (point_eq().eq)(p, p) }",
        ),
        (
            "ord is reflexive",
            "pub type Point = P(Int)\n\
             derive ord for Point\n\
             law \"ord is reflexive\" forall (p: Point) \
               { (point_ord().compare)(p, p) == Equal }",
        ),
    ] {
        never_proved(source, needle);
    }
}
