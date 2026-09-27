use crate::fixture::{project, repo};
use ply_eval::Plan;
use ply_machine::engine::Prover;
use ply_machine::load::load;
use ply_machine::obligations;
use ply_prove::{Discharge, Evidence, Gap, Obligation, ProvePlan, Rule, Tier, VacuityKind};
use std::path::Path;

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
        let collected = obligations::collect(&loaded.front, &loaded.check, &hashes);
        let prover = Prover::new(&loaded)
            .expect("the port lowers the claims")
            .with_backend(Some(
                ply_machine::support::prover_backend(&loaded)
                    .expect("the program compiles to a tier"),
            ));
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

#[test]
fn no_false_claim_is_ever_proved() {
    let false_claims: &[(&str, &str)] = &[
        // Division is uninterpreted: nothing follows from it, not even with a literal divisor.
        (
            "halving",
            "law \"halving\" forall (x: Int) { x / 2 * 2 == x }",
        ),
        ("by one", "law \"by one\" forall (x: Int) { x / 1 == x }"),
        ("modulo", "law \"modulo\" forall (x: Int) { x % 1 == 0 }"),
        // `x * y` with both factors symbolic is not linear arithmetic.
        ("square", "law \"square\" forall (x: Int) { x * x >= x }"),
        // Two distinct uninterpreted symbols are neither provably equal nor provably distinct.
        (
            "two functions",
            "law \"two functions\" forall (f: (Int) -> Int, g: (Int) -> Int, x: Int) { f(x) == g(x) }",
        ),
        // A case analysis over every constructor still evaluates each arm, and one of them is 3.
        (
            "under three",
            "type Color = Red | Green | Blue\n\
             fn score(c: Color) -> Int = match c { Red -> 1, Green -> 2, Blue -> 3 }\n\
             law \"under three\" forall (c: Color) { score(c) < 3 }",
        ),
        // A nested constructor pattern is not split, so the `match` stays uninterpreted.
        (
            "always two",
            "type Color = Red | Green | Blue\n\
             type Wrap = W(Color)\n\
             fn f(w: Wrap) -> Int = match w { W(Red) -> 1, _ -> 2 }\n\
             law \"always two\" forall (w: Wrap) { f(w) == 2 }",
        ),
        // `++` is uninterpreted, and congruence over it says nothing about whether it commutes.
        (
            "concat commutes",
            "law \"concat commutes\" forall (s: String, t: String) { s ++ t == t ++ s }",
        ),
        // List literals are injective in their elements, which is why this is false rather than unknown.
        (
            "one element",
            "law \"one element\" forall (x: Int, y: Int) { [x] == [y] }",
        ),
        // A record is its fields, so two records agreeing on one field is not extensionality.
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

/// Two calls to a definition that performs may answer differently, so they may not share a term.
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

/// A spec expression's row must be empty, so a clause cannot call an effectful definition at all.
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

/// An unsatisfiable guard makes `guard ⟹ body` valid and meaningless.
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
        // The prelude's `Option`: declaring a second would be `E0105`.
        "fn or_else(o: Option<Int>, d: Int) -> Int = match o { None -> d, Some(v) -> v }\n\
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
        let collected = obligations::collect(&loaded.front, &loaded.check, &hashes);
        let prover = Prover::new(&loaded)
            .expect("the port lowers the claims")
            .with_backend(Some(
                ply_machine::support::prover_backend(&loaded)
                    .expect("the program compiles to a tier"),
            ));
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

/// A `simulate` region reached by two tasks, with a handler standing in for the resource.
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

#[test]
fn an_int_binder_drops_a_concurrency_law_to_property() {
    let run = Run::at(
        &repo().join("tests/fixtures/concurrency_law_binder.ply"),
        &ProvePlan::default(),
    );
    assert_eq!(run.tier("no interleaving"), Some(Tier::Property));
}

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

/// A body that entered no `simulate` region emptied a frontier it never filled, so `exhaustive: true` is about nothing.
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

    // A guard recovers the reach: the same claim over a domain the arithmetic fits in is decided outright.
    assert_eq!(run.tier("bounded_inc"), Some(Tier::Proved));
}

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
    let collected = obligations::collect(&loaded.front, &loaded.check, &hashes);
    let prover = Prover::new(&loaded)
        .expect("the port lowers the claims")
        .with_backend(Some(
            ply_machine::support::prover_backend(&loaded).expect("the program compiles to a tier"),
        ));
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

    // A body nothing decides: not a proof, and still not a claim that the guard admits nothing.
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

    // The ground law has no value domain, so it names only the interleaving search.
    let ground = Run::of(&concurrency_law("", "1"));
    assert!(
        ground
            .certificate("two writers")
            .rules
            .iter()
            .all(|r| !matches!(r, Rule::ExhaustiveEnumeration { .. }))
    );
}

/// `==` on `Float` is not reflexive, so congruence closure over it is unsound.
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
        // The four below reach a `Float` through a declaration rather than the binder's written type.
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
        if Run::of(source).tier(needle) == Some(Tier::Proved) {
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
    let collected = obligations::collect(&loaded.front, &loaded.check, &hashes);
    let prover = Prover::new(&loaded)
        .expect("the port lowers the claims")
        .with_backend(Some(
            ply_machine::support::prover_backend(&loaded).expect("the program compiles to a tier"),
        ));
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
        run.tier("keep"),
        Some(Tier::Proved),
        "an `ensures` false at `Cents(NaN)` carries a certificate"
    );
    assert_ne!(
        run.tier("destructured"),
        Some(Tier::Proved),
        "the destructured form must stay refused as well"
    );
}

/// `Decimal`'s `==` is an equivalence relation, so congruence over it is sound.
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
    assert_eq!(run.tier("congruence"), Some(Tier::Proved));
    assert_eq!(run.tier("scale is value"), Some(Tier::Proved));
    for needle in ["additive", "commutes", "ordered"] {
        assert_ne!(
            run.tier(needle),
            Some(Tier::Proved),
            "`{needle}` grew the arithmetic fragment"
        );
    }
}

/// There is no theory of arrays: nothing about `map_get` after `map_insert`, or about `map_len`, may be concluded.
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
    assert_eq!(run.tier("reflexive"), Some(Tier::Proved));
    for needle in ["get after insert", "insert grows", "keys match len"] {
        assert_ne!(
            run.tier(needle),
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

    // Two occurrences of one `Bytes` literal are deliberately not one term, so even this is `property`.
    never_proved(
        "law \"two literals\" forall (n: Int) { b\"ab\" == b\"ab\" }",
        "two literals",
    );
}

/// A derived dictionary is an ordinary record of closures, outside the fragment.
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
