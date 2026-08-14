//! An adversarial audit of the one thing this milestone cannot get wrong.
//!
//! `tiers.rs` checks that the prover's certificates are well formed and that the
//! corpus survives a wider sample. This file is the other half: it goes looking
//! for a **lie**, by writing claims that are false, unevaluable, or true only
//! outside the fragment, and asserting that none of them comes back wearing a
//! certificate.
//!
//! Two kinds of test live here and they are labelled as such.
//!
//! - The **soundness pins** assert a property that holds today and must keep
//!   holding. A failure is a regression in the prover.
//! - The **closed gaps** (`gap_*`) are the reproductions an audit found the tier
//!   label over-claiming at. Each one keeps the program that produced the wrong
//!   label and now asserts the honest one, so the defect stays pinned from the
//!   side it has to be pinned from: a failure here means the over-claim came
//!   back.

use ply_cli::engine::Prover;
use ply_cli::load::load;
use ply_cli::obligations;
use ply_eval::Plan;
use ply_prove::{Discharge, Evidence, Gap, Obligation, ProvePlan, Rule, Tier, VacuityKind};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

struct Run {
    results: Vec<(Obligation, Discharge)>,
}

impl Run {
    fn of(source: &str) -> Run {
        Run::at(project(source).path(), &ProvePlan::default())
    }

    fn with(source: &str, plan: &ProvePlan) -> Run {
        Run::at(project(source).path(), plan)
    }

    fn at(path: &Path, plan: &ProvePlan) -> Run {
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
                let discharge = prover.discharge_with(&o, plan);
                (o, discharge)
            })
            .collect();
        Run { results }
    }

    #[track_caller]
    fn find(&self, needle: &str) -> &(Obligation, Discharge) {
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
    }

    fn tier(&self, needle: &str) -> Option<Tier> {
        self.find(needle).1.tier()
    }

    #[track_caller]
    fn certificate(&self, needle: &str) -> &ply_prove::Certificate {
        match &self.find(needle).1 {
            Discharge::Held(Evidence::Proof(c)) => c,
            other => panic!("`{needle}` is {other:?} rather than a proof"),
        }
    }
}

/// The whole file in one assertion: whatever else happened, this claim is not
/// carrying a certificate.
#[track_caller]
fn never_proved(source: &str, needle: &str) -> Discharge {
    let run = Run::of(source);
    let discharge = run.find(needle).1.clone();
    assert_ne!(
        discharge.tier(),
        Some(Tier::Proved),
        "`{needle}` came back proved: {discharge:?}"
    );
    discharge
}

// --- Soundness pins: nothing false is ever proved ---------------------------

/// A battery of claims that are **false** over Ply's own semantics. Each one is
/// inside or adjacent to a rule the prover has, and each one is a place where a
/// slightly wrong rule would produce a certificate.
///
/// Written as one test over a table because the interesting output is *which*
/// entry broke, and a table names it without twenty near-identical functions.
#[test]
fn no_false_claim_is_ever_proved() {
    let false_claims: &[(&str, &str)] = &[
        // Division is uninterpreted, so nothing may be concluded from it — not
        // even by a literal divisor, where the arithmetic is tempting and wrong.
        (
            "halving",
            "law \"halving\" forall (x: Int) { x / 2 * 2 == x }",
        ),
        ("by one", "law \"by one\" forall (x: Int) { x / 1 == x }"),
        ("modulo", "law \"modulo\" forall (x: Int) { x % 1 == 0 }"),
        // `x * y` with both factors symbolic is not linear arithmetic.
        ("square", "law \"square\" forall (x: Int) { x * x >= x }"),
        // Two distinct uninterpreted symbols are not equal, and are also not
        // provably distinct: neither direction may be claimed.
        (
            "two functions",
            "law \"two functions\" forall (f: (Int) -> Int, g: (Int) -> Int, x: Int) { f(x) == g(x) }",
        ),
        // A case analysis that reaches every constructor still has to evaluate
        // each arm, and one of them is 3.
        (
            "under three",
            "type Color = Red | Green | Blue\n\
             fn score(c: Color) -> Int = match c { Red -> 1, Green -> 2, Blue -> 3 }\n\
             law \"under three\" forall (c: Color) { score(c) < 3 }",
        ),
        // A nested constructor pattern is not split, so the `match` stays
        // uninterpreted rather than being guessed at.
        (
            "always two",
            "type Color = Red | Green | Blue\n\
             type Wrap = W(Color)\n\
             fn f(w: Wrap) -> Int = match w { W(Red) -> 1, _ -> 2 }\n\
             law \"always two\" forall (w: Wrap) { f(w) == 2 }",
        ),
        // `++` is uninterpreted, and congruence over it says nothing about
        // whether it commutes.
        (
            "concat commutes",
            "law \"concat commutes\" forall (s: String, t: String) { s ++ t == t ++ s }",
        ),
        // List literals are injective in their elements, which is exactly why
        // this is false rather than unknown.
        (
            "one element",
            "law \"one element\" forall (x: Int, y: Int) { [x] == [y] }",
        ),
        // A record is its fields, so two records agreeing on one field is not
        // extensionality.
        (
            "half a record",
            "law \"half a record\" forall (x: Int, y: Int) { { a: x, b: 0 } == { a: x, b: y } }",
        ),
        // The wildcard arm is reached for two of three constructors.
        (
            "always red",
            "type Color = Red | Green | Blue\n\
             fn is_red(c: Color) -> Bool = match c { Red -> true, _ -> false }\n\
             law \"always red\" forall (c: Color) { is_red(c) }",
        ),
    ];
    for (needle, source) in false_claims {
        let discharge = never_proved(source, needle);
        assert!(
            matches!(discharge, Discharge::Refuted(_))
                || discharge.tier() == Some(Tier::Property)
                || discharge.tier() == Some(Tier::Example)
                || matches!(discharge, Discharge::Unattempted(_)),
            "`{needle}` came back as {discharge:?}"
        );
    }
}

/// Two calls to a definition that performs may answer differently, so they may
/// not share a term: `f() - f() == 0` reported `proved` is the shape of that
/// mistake.
///
/// The owner's *body* is lowered into the goal even when the owner performs —
/// the static tier is attempted first, before the unhandled-effect gap — so this
/// is the one place an effectful call still reaches the congruence closure. It
/// has to come back as the gap, not as a proof that the difference is zero.
#[test]
fn an_effectful_call_is_not_a_function_of_its_arguments() {
    const SOURCE: &str = "\
effect db {
  read get[r](k: Int) -> Int
}

fn fetch(k: Int) -> Int / {db.read[main]} = db.get[main](k)

fn difference(k: Int) -> Int / {db.read[main]}
  ensures result == 0
= fetch(k) - fetch(k)
";
    let discharge = never_proved(SOURCE, "difference");
    assert!(
        matches!(discharge, Discharge::Unattempted(Gap::UnhandledEffect(_))),
        "an ensures on an effectful definition is a reported gap: {discharge:?}"
    );
}

/// The other half of the same rule, and the one the type system owns: a spec
/// expression's row must be empty, so a clause cannot call an effectful
/// definition at all. Without this the congruence closure would be reasoning
/// about a term whose value depends on the world.
#[test]
fn a_clause_that_performs_is_rejected_before_the_prover_sees_it() {
    const SOURCE: &str = "\
effect db {
  read get[r](k: Int) -> Int
}

fn fetch(k: Int) -> Int / {db.read[main]} = db.get[main](k)

fn echo(k: Int) -> Int / {db.read[main]}
  ensures result == fetch(k)
= fetch(k)
";
    let dir = project(SOURCE);
    let Err(error) = load(dir.path()) else {
        panic!("a spec may not perform an effect");
    };
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code == ply_span::codes::EFFECT_IN_SPEC),
        "{:?}",
        error
            .diagnostics
            .iter()
            .map(|d| format!("{} {}", d.code, d.message))
            .collect::<Vec<_>>()
    );
}

/// An unsatisfiable guard makes `guard ⟹ body` valid and meaningless. Deciding
/// vacuity **before** the body is what stops a typo in a guard from becoming a
/// proof of everything, and the second entry checks it through an uninterpreted
/// operator, where the contradiction comes from congruence rather than from the
/// arithmetic.
#[test]
fn an_unsatisfiable_guard_is_vacuous_and_never_proved() {
    for source in [
        "law \"impossible\" forall (x: Int) where x > 0 && x < 0 { x == 5 }",
        "law \"impossible\" forall (x: Int) where x % 2 == 0 && x % 2 == 1 { x == 5 }",
    ] {
        let discharge = never_proved(source, "impossible");
        assert!(
            matches!(
                discharge,
                Discharge::Vacuous(ply_prove::Vacuity {
                    kind: VacuityKind::ProvedUnsatisfiable,
                    ..
                })
            ),
            "{discharge:?}"
        );
    }
}

/// `guard ⟹ body` over a domain with no values is valid and says nothing, so a
/// binder of an uninhabited type may not carry a proof however trivial the body
/// is.
#[test]
fn a_binder_of_an_uninhabited_type_never_carries_a_proof() {
    const SOURCE: &str = "\
type Bad = Wrap(Bad)

law \"a claim about nothing\" forall (b: Bad) { b == b }
";
    let discharge = never_proved(SOURCE, "a claim about nothing");
    assert!(
        matches!(discharge, Discharge::Unattempted(Gap::Ungeneratable { .. })),
        "{discharge:?}"
    );
}

/// A member of a recursive component is never unfolded, so nothing about its
/// behaviour over unbounded data is decided. The positive control beside it is
/// what keeps this test honest: the non-recursive definition *is* unfolded, and
/// to a depth the certificate names.
#[test]
fn recursion_stops_the_unfolding_and_the_bound_is_named() {
    const SOURCE: &str = "\
fn sum_to(n: Int) -> Int = if n <= 0 { 0 } else { n + sum_to(n - 1) }

fn twice(x: Int) -> Int = x + x

law \"the sum is triangular\" forall (n: Int) where n > 0 && n < 1000
  { sum_to(n) * 2 == n * n + n }
law \"twice is doubling\" forall (x: Int) where x > -1000 && x < 1000
  { twice(x) == 2 * x }
";
    never_proved(SOURCE, "the sum is triangular");

    let run = Run::of(SOURCE);
    assert_eq!(run.tier("twice is doubling"), Some(Tier::Proved));
    let unfolded: Vec<u32> = run
        .certificate("twice is doubling")
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::Unfold { depth, .. } => Some(*depth),
            _ => None,
        })
        .collect();
    assert!(
        !unfolded.is_empty() && unfolded.iter().all(|d| *d <= ply_prove::UNFOLD_DEPTH),
        "{unfolded:?}"
    );
}

/// A `proved` obligation is a claim about every plan, so the two tiers must
/// never disagree in the direction that matters. This is `tiers.rs`'s
/// differential audit narrowed onto the constructs this file attacks, at a plan
/// wide enough that a sampled run reaches every edge the generator draws.
///
/// A **raise** counts as a disagreement, not only a refutation. Ply's
/// arithmetic is checked and its recursion is bounded, so an obligation the
/// prover got wrong about totality can never come back false — it comes back
/// `E0502`, and an audit blind to that could not fail on the defect it exists
/// for.
#[test]
fn nothing_proved_here_is_refutable_by_sampling() {
    let sources: &[&str] = &[
        "law \"congruent\" forall (f: (Int) -> Int, x: Int, y: Int) where x == y { f(x) == f(y) }",
        "law \"excluded middle\" forall (b: Bool) { b || !b }",
        "type Color = Red | Green | Blue\n\
         fn score(c: Color) -> Int = match c { Red -> 1, Green -> 2, Blue -> 3 }\n\
         law \"score is positive\" forall (c: Color) { score(c) > 0 }",
        "law \"records are their fields\" forall (x: Int, y: Int) \
           { { a: x, b: y } == { b: y, a: x } }",
        "type Option<a> = None | Some(a)\n\
         fn or_else(o: Option<Int>, d: Int) -> Int = match o { None -> d, Some(v) -> v }\n\
         law \"or_else is a function\" forall (o: Option<Int>, d: Int) \
           { or_else(o, d) == or_else(o, d) }",
    ];
    let wide = ProvePlan {
        cases: 1_000,
        roots: (0..8).collect(),
        ..ProvePlan::default()
    };
    let mut audited = 0;
    for source in sources {
        let dir = project(source);
        let loaded = load(dir.path()).expect("the fixture compiles");
        let hashes = loaded.hashes.clone();
        let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
        let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
        for obligation in &collected.obligations {
            if prover
                .discharge_with(obligation, &ProvePlan::default())
                .tier()
                != Some(Tier::Proved)
            {
                continue;
            }
            audited += 1;
            match prover.resample(obligation, &wide) {
                Discharge::Refuted(counterexample) => panic!(
                    "`{}` is proved and a sampled run refutes it at {:?} — a defect in Ply",
                    obligation.owner,
                    counterexample
                        .bindings
                        .iter()
                        .map(|b| format!("{} = {}", b.name, b.rendered))
                        .collect::<Vec<_>>()
                ),
                Discharge::Unattempted(Gap::Raised {
                    diagnostic,
                    bindings,
                }) => panic!(
                    "`{}` is proved and a sampled run raises `{}` at {:?} — a defect in Ply",
                    obligation.owner,
                    diagnostic.message,
                    bindings
                        .iter()
                        .map(|b| format!("{} = {}", b.name, b.rendered))
                        .collect::<Vec<_>>()
                ),
                _ => {}
            }
        }
    }
    assert_eq!(audited, sources.len(), "every source above is a proof");
}

// --- Soundness pins: concurrency --------------------------------------------

/// A `simulate` region reached by two tasks, with a handler standing in for the
/// resource. The binder is the variable under test in the three cases below.
fn concurrency_law(header: &str, spawned: &str) -> String {
    format!(
        "\
effect chan {{
  write put[q](v: Int) -> Unit
}}

fn worker(v: Int) -> Unit / {{chan.write[q], clock.read}} {{
  let t = clock.now();
  chan.put[q](v)
}}

law \"two writers land twice\"{header} {{
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
    )
}

/// ADR 0007 §6's five conditions plus the sixth, one case each.
///
/// The ground law is the positive control; the `Bool` binder is the only shape
/// in which a *quantified* concurrency law may be proved, and the certificate
/// has to carry both coverage claims because they are independent.
#[test]
fn a_concurrency_law_is_proved_only_when_both_domains_were_covered() {
    let ground = Run::of(&concurrency_law("", "1"));
    assert_eq!(ground.tier("two writers"), Some(Tier::Proved));
    assert!(
        ground
            .certificate("two writers")
            .rules
            .iter()
            .any(|r| matches!(r, Rule::ExhaustiveInterleaving { .. })),
        "a ground concurrency law is proved by its search"
    );

    let finite = Run::of(&concurrency_law(
        " forall (flip: Bool)",
        "if flip { 1 } else { 3 }",
    ));
    assert_eq!(finite.tier("two writers"), Some(Tier::Proved));
    let rules = &finite.certificate("two writers").rules;
    assert!(
        rules
            .iter()
            .any(|r| matches!(r, Rule::ExhaustiveEnumeration { points: 2, .. }))
            && rules
                .iter()
                .any(|r| matches!(r, Rule::ExhaustiveInterleaving { .. })),
        "both coverage claims have to be named: {rules:?}"
    );
}

/// The condition an implementer drops. `exhaustive: true` is a claim about
/// **schedules**, and this law also ranges over 2⁶⁴ values of `amount`.
#[test]
fn an_int_binder_drops_a_concurrency_law_to_property() {
    let run = Run::at(
        &repo("tests/fixtures/concurrency_law_binder.ply"),
        &ProvePlan::default(),
    );
    assert_eq!(run.tier("no interleaving"), Some(Tier::Property));
}

/// A sampled schedule search has no exhaustiveness to claim, whatever it
/// reports. Both non-`Dpor` modes, because it is one `match` away from being
/// only one of them.
#[test]
fn a_sampled_schedule_search_never_proves() {
    let source = concurrency_law("", "1");
    for sim in [Plan::random(4), Plan::once(ply_eval::Seed::root(7))] {
        let plan = ProvePlan {
            sim,
            ..ProvePlan::default()
        };
        let run = Run::with(&source, &plan);
        assert_ne!(
            run.tier("two writers"),
            Some(Tier::Proved),
            "a sampled search is not a covered one"
        );
    }
}

/// The sixth condition, which no signature over an `Exploration` can express: a
/// body that entered no `simulate` region emptied a frontier it never filled,
/// and `exhaustive: true` over it is a claim about nothing.
///
/// Half the domain here takes the branch with no region, so the law must not be
/// proved even though every search that *did* run was exhaustive.
#[test]
fn a_search_that_reached_no_region_never_proves() {
    const SOURCE: &str = "\
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
";
    never_proved(SOURCE, "sometimes concurrent");
}

// --- Soundness pins: a cached proof is never a stale one --------------------

/// A proof written under the bare obligation key survives every widening of the
/// plan, so the *only* thing standing between a cached `proved` and a proof of
/// something no longer true is the key covering the implementation's whole
/// transitive closure. Each edit below must re-open exactly the obligations it
/// invalidates.
#[test]
fn editing_what_a_proof_rests_on_re_opens_it() {
    use assert_cmd::Command;
    use serde_json::Value;

    fn outcomes(dir: &Path) -> Vec<(String, String)> {
        let out = Command::cargo_bin("ply")
            .unwrap()
            .arg("--color")
            .arg("never")
            .current_dir(dir)
            .arg("prove")
            .arg("--json")
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        let json: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        json["obligations"]
            .as_array()
            .expect("an obligation array")
            .iter()
            .map(|o| {
                (
                    o["label"].as_str().unwrap_or_default().to_string(),
                    o["outcome"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    // Two links between the claim and the value it is about, so the second edit
    // tests the transitive half rather than the direct one.
    let good = "\
fn leaf() -> Int = 1

fn base() -> Int = leaf()

fn shift(x: Int) -> Int
  requires x > 0 && x < 1000
  ensures result == x + 1
= x + base()

law \"shift agrees with base\" forall (x: Int) where x > 0 && x < 1000
  { shift(x) == x + base() }
";
    let edits: &[(&str, &str, &[&str])] = &[
        ("the body", "= x + base()\n", &["ensures", "law"]),
        ("the spec", "ensures result == x + 1", &["ensures"]),
        (
            "a transitive dependency",
            "fn leaf() -> Int = 1",
            &["ensures"],
        ),
    ];
    for (what, needle, expect_refuted) in edits {
        let dir = project(good);
        let first = outcomes(dir.path());
        assert!(
            first.iter().all(|(_, outcome)| outcome == "proved"),
            "the baseline is two proofs: {first:?}"
        );

        let broken = match *what {
            "the body" => good.replace(needle, "= x + base() + 1\n"),
            "the spec" => good.replace(needle, "ensures result == x + 3"),
            _ => good.replace(needle, "fn leaf() -> Int = 2"),
        };
        assert_ne!(broken, good, "the edit to {what} matched nothing");
        std::fs::write(dir.path().join("m.ply"), &broken).unwrap();

        let after = outcomes(dir.path());
        for kind in *expect_refuted {
            let found = after
                .iter()
                .find(|(label, _)| label.contains(kind))
                .unwrap_or_else(|| {
                    panic!("no `{kind}` obligation after editing {what}: {after:?}")
                });
            assert_eq!(
                found.1, "refuted",
                "editing {what} left `{}` cached as `{}`, which is a proof of something no \
                 longer true",
                found.0, found.1
            );
        }
    }
}

// --- Characterizations: where the tier label over-claims ---------------------

/// **Closed.** `proved` was reported for an obligation that is not valid at
/// every input, because at the `Int` boundary the program raises rather than
/// answering.
///
/// ADR 0007 §5.1(a) disclosed that the prover reasons over ℤ and named a
/// mitigation — "the generator draws `i64::MIN` and `i64::MAX` on every property
/// run, so the *sampled* tier catches what the proved tier assumes" — which
/// could not fire, for two independent reasons:
///
/// 1. `Prover::discharge_with` returns the static proof before the sampled tier
///    is reached, so a `proved` obligation is never sampled by an ordinary run;
/// 2. Ply's arithmetic is `checked_*`, so the boundary can only ever surface as
///    a raise, and §11's differential tier audit treated only
///    `Discharge::Refuted` as a defect.
///
/// Both halves are fixed. A proof now discharges the definedness of every
/// arithmetic term it reasoned about, so this is not proved at all; and the
/// differential audit treats a raise on a proved obligation as a defect, so the
/// mitigation has teeth even where a future rule loses them.
#[test]
fn gap_a_proved_obligation_may_raise_at_the_int_boundary() {
    const SOURCE: &str = "\
fn inc(x: Int) -> Int
  ensures result > x
= x + 1

fn bounded_inc(x: Int) -> Int
  requires x < 100
  ensures result > x
= x + 1
";
    let run = Run::of(SOURCE);
    let discharge = &run.find("m.inc").1;
    assert!(
        matches!(discharge, Discharge::Unattempted(Gap::Raised { diagnostic, .. })
            if diagnostic.message.contains("overflow")),
        "`x + 1 > x` has no answer at `i64::MAX`, so no tier covers every input: {discharge:?}"
    );

    // The reach is recovered by a guard rather than by a disclosure: the same
    // claim over a domain the arithmetic fits in is decided outright.
    assert_eq!(run.tier("bounded_inc"), Some(Tier::Proved));
}

/// **Closed.** An uninterpreted function symbol is total, so congruence proved
/// claims about definitions that provably never return — and the definition then
/// counted as *covered*, telling a reviewer they may stop reading a function
/// that cannot answer.
///
/// ADR 0007 §12 says of `DEFAULT_MAX_CALLS` that an obligation hitting it is
/// "`Unattempted { Raised }`, never `proved` and never `refuted`". The static
/// tier answered first, so that sentence was false as implemented. It is true
/// now: a call this prover did not inline carries a definedness requirement, and
/// a member of a recursive component is never inlined.
#[test]
fn gap_a_definition_that_never_returns_still_carries_a_proof() {
    const SOURCE: &str = "\
fn spin(x: Int) -> Int = spin(x)

fn go(x: Int) -> Int
  ensures result == spin(x)
= spin(x)

law \"spin is a function\" forall (x: Int) { spin(x) == spin(x) }

law \"a divisor is a function\" forall (a: Int, b: Int) { a / b == a / b }
";
    let run = Run::of(SOURCE);
    for label in ["go", "spin is a function", "a divisor is a function"] {
        let discharge = &run.find(label).1;
        assert_ne!(
            discharge.tier(),
            Some(Tier::Proved),
            "`{label}` is a theorem about a total symbol and not about this program: \
             {discharge:?}"
        );
        assert!(
            matches!(discharge, Discharge::Unattempted(_)),
            "`{label}`: {discharge:?}"
        );
    }

    // And the coverage line says so, which is the half a reviewer reads.
    let dir = project(SOURCE);
    let loaded = load(dir.path()).expect("the fixture compiles");
    let hashes = loaded.hashes.clone();
    let laws = ply_test::obligation::Laws::of(&loaded.check, &hashes);
    let collected = obligations::collect(&loaded.program, &loaded.check, &hashes);
    let prover = Prover::new(&loaded.program, &loaded.resolved, &loaded.check);
    let results: Vec<(Obligation, Discharge)> = collected
        .obligations
        .into_iter()
        .map(|o| {
            let d = prover.discharge_with(&o, &ProvePlan::default());
            (o, d)
        })
        .collect();
    let coverage = ply_test::obligation::coverage(&loaded.check, &laws, &results);
    assert_eq!(coverage.covered, 0, "{:?}", coverage.uncovered);
    assert!(coverage.uncovered.iter().any(|n| n.as_str() == "m.spin"));
    assert!(coverage.uncovered.iter().any(|n| n.as_str() == "m.go"));
}

/// **Closed.** A guard the generator cannot satisfy was reported as `E0420`,
/// "the guard admits no value" — a statement about the program that is false —
/// and the static argument the prover had already made for the body was dropped
/// on the way.
///
/// `x > 1000000 && x < 1000010` admits nine values. The prover *decides*
/// `guard ⟹ body` here; all it could not do is witness the domain, which
/// `Proof::certify` correctly refuses to assume. So the domain is now looked for
/// directly, by evaluating the guard at the points its own literals name, and a
/// value found there vouches for the argument exactly as a kept case would.
///
/// Where no witness is found the vacuity stands: `kept == 0` with nothing to
/// show for the domain is still ADR 0007 §5.4's row, and
/// `tests/fixtures/vacuous_law.ply` is the fixture for it.
#[test]
fn gap_a_guard_outside_the_generators_range_is_called_vacuous() {
    const SOURCE: &str =
        "law \"a narrow window\" forall (x: Int) where x > 1000000 && x < 1000010 { x > 0 }";
    let run = Run::of(SOURCE);
    let discharge = &run.find("a narrow window").1;
    assert_eq!(
        discharge.tier(),
        Some(Tier::Proved),
        "the guard admits nine values and the body is decided over all of them: {discharge:?}"
    );

    // The same window over a body nothing decides: not a proof, and still not a
    // claim that the guard admits nothing.
    const UNDECIDED: &str = "\
fn seen(xs: List<Int>, x: Int) -> Bool =
  match xs {
    [y, ..rest] -> if y == x { true } else { seen(rest, x) },
    _ -> false,
  }

law \"a narrow window nobody samples\" forall (xs: List<Int>, x: Int)
  where x > 1000000 && x < 1000010 { seen(push(xs, x), x) }
";
    let run = Run::of(UNDECIDED);
    let discharge = &run.find("a narrow window nobody samples").1;
    assert!(
        matches!(
            discharge,
            Discharge::Unattempted(Gap::GuardNotSampled { witness, .. }) if !witness.is_empty()
        ),
        "a guard the search missed is a gap in the search, not a defect in the spec: \
         {discharge:?}"
    );
}

/// **Closed.** `concurrency::audit_interleaving_proof` rejected a *correct*
/// proof whenever the covered value domain had exactly one point.
///
/// `Totals::certificate` emitted `Rule::ExhaustiveEnumeration` only when
/// `points > 1`, while the audit demands that rule whenever the law has binders.
/// A law over `Unit` has its domain covered — one point, and no way to miss any
/// of it — and still failed the check, so `tiers.rs`'s certificate audit panicked
/// on a correct program. The certificate now names the coverage whenever the law
/// has binders, whatever the point count, so the audit is checking the condition
/// rather than a proxy for it.
#[test]
fn gap_a_one_point_domain_fails_the_interleaving_audit() {
    let run = Run::of(&concurrency_law(" forall (u: Unit)", "1"));
    assert_eq!(run.tier("two writers"), Some(Tier::Proved));
    let (obligation, _) = run.find("two writers");
    let certificate = run.certificate("two writers");
    assert!(
        certificate
            .rules
            .iter()
            .any(|r| matches!(r, Rule::ExhaustiveEnumeration { points: 1, .. })),
        "a one-point domain was covered, and the certificate says which: {:?}",
        certificate.rules
    );
    assert_eq!(
        ply_prove::concurrency::audit_interleaving_proof(obligation, certificate),
        Ok(())
    );

    // The ground law next door still names the interleaving search and nothing
    // else: there is no value domain to have covered.
    let ground = Run::of(&concurrency_law("", "1"));
    assert!(
        ground
            .certificate("two writers")
            .rules
            .iter()
            .all(|r| !matches!(r, Rule::ExhaustiveEnumeration { .. }))
    );
}
