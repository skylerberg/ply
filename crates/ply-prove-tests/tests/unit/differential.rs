//! A proof is a claim about every value, so a wide sample of the same claim on the compiled tier
//! never refutes it and never raises under it. Each law the static prover proves here is sampled.

use crate::checked::{binders_of, world_of};
use ply_codegen::c::producer;
use ply_eval::decode::At;
use ply_eval::{
    Compiled, DEFAULT_MAX_CALLS, DefHash, Diagnostic, Entered, SourceId, Span, Symbol, Value,
};
use ply_prove::property::{Judge, run_property};
use ply_prove::prove::claims::Code;
use ply_prove::prove::{Claims, Context, Decision, Goal, Limits, decide, read_claims};
use ply_prove::{Discharge, Gap, ProvePlan, World};
use std::collections::HashMap;
use std::rc::Rc;

const SRC: SourceId = SourceId(0);

const PROVED: &str = r#"
type Color = Red | Green | Blue
type Money = Cents(Float)
type Row = R({rate: Float})

fn score(c: Color) -> Int = match c { Red -> 1, Green -> 2, Blue -> 3 }
fn or_else(o: Option<Int>, d: Int) -> Int = match o { None -> d, Some(v) -> v }

law "congruent" forall (f: (Int) -> Int, x: Int, y: Int) where x == y { f(x) == f(y) }
law "excluded middle" forall (b: Bool) { b || !b }
law "score is positive" forall (c: Color) { score(c) > 0 }
law "records are their fields" forall (x: Int, y: Int) { { a: x, b: y } == { b: y, a: x } }
law "or_else is a function" forall (o: Option<Int>, d: Int) { or_else(o, d) == or_else(o, d) }
law "hidden in a variant" forall (m: Money) { m == m }
law "hidden in a record type" forall (r: Row) { r == r }
"#;

/// A source's claims, the world they are written over, and the tier its roots are entered on.
struct Audited {
    answer: Value,
    world: World,
    claims: Claims,
    tier: Rc<ply_codegen::Bodies>,
}

fn audited(source: &str) -> Audited {
    // Anonymous, so every root is the bare name the codegen gives it.
    let sources = [(String::new(), source.to_string())];
    producer::ensure_default();
    let answer = producer::front_pulling_std(&sources, &[])
        .unwrap_or_else(|e| panic!("check: {e:#}"))
        .dump;
    let claimed = producer::claims(&sources, &[], &[]).unwrap_or_else(|e| panic!("claims: {e:#}"));
    let claims = read_claims(At::new("the claims", &claimed), &[SRC])
        .unwrap_or_else(|e| panic!("claims: {e}"));
    let front = producer::checked_front(&sources, &[SRC])
        .unwrap_or_else(|e| panic!("the source must typecheck: {e:#}"));
    let tier = ply_codegen::Unit::over_front(&front, HashMap::from([sources[0].clone()]))
        .expect("this host has a C compiler")
        .bodies()
        .expect("the unit builds");
    Audited {
        world: world_of(At::new("the front end's answer", &answer)),
        answer,
        claims,
        tier,
    }
}

/// One law, its guard then its body entered on the tier, as a run of cases judges it.
struct OnTier {
    tier: Rc<ply_codegen::Bodies>,
    guard: Option<Symbol>,
    body: Symbol,
}

impl OnTier {
    fn truth(&self, root: &Symbol, values: &[Value]) -> Result<bool, Diagnostic> {
        match self.tier.enter_whole(root, values, DEFAULT_MAX_CALLS) {
            Entered::Answered(Value::Bool(b)) => Ok(b),
            Entered::Answered(other) => panic!("`{root}` came to `{other}`"),
            Entered::Raised(d) => Err(d),
            Entered::Declined => panic!("the tier declined `{root}`"),
        }
    }
}

impl Judge for OnTier {
    fn guard(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        match &self.guard {
            Some(root) => self.truth(root, values),
            None => Ok(true),
        }
    }

    fn body(&mut self, values: &[Value]) -> Result<bool, Diagnostic> {
        self.truth(&self.body, values)
    }
}

/// Every law the static prover proves, by label, with how a wide sample of it contradicted the proof.
fn contradictions(a: &Audited) -> (Vec<String>, Vec<String>) {
    let wide = ProvePlan {
        cases: 1_000,
        roots: (0..8).collect(),
        ..ProvePlan::default()
    };
    let ctx = Context::new(a.claims.clone(), &a.world);
    let laws = At::new("the front end's answer", &a.answer)
        .field("laws")
        .and_then(|laws| laws.list())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut proved = Vec::new();
    let mut contradicted = Vec::new();
    for row in laws {
        let read = || -> Result<_, ply_eval::decode::Error> {
            Ok((
                row.field("name")?.utf8()?.to_string(),
                Symbol::new(row.field("key")?.utf8()?),
                row.field("index")?.number::<usize>()?,
                row.field("binders")?
                    .items(|b| Ok((Symbol::new(b.field("name")?.utf8()?), b.field("ty")?)))?,
            ))
        };
        let (label, key, index, named) = read().unwrap_or_else(|e| panic!("{e}"));
        let binders = binders_of(&named);
        let law = &a.claims.laws[&key];
        let guards: Vec<&Code> = law.guard.iter().map(|g| &g.code).collect();
        let goal = Goal {
            binders: &binders,
            guards: &guards,
            result: None,
            body: &law.body,
        };
        if !matches!(decide(&ctx, &goal, &Limits::default()), Decision::Proved(_)) {
            continue;
        }
        proved.push(label.clone());
        let mut vars = Vec::new();
        for binder in &binders {
            binder.sort.vars(&mut vars);
        }
        let names: Vec<Symbol> = (0..=vars.iter().copied().max().unwrap_or(0))
            .map(|v| Symbol::new(format!("t{v}")))
            .collect();
        let mut judge = OnTier {
            tier: Rc::clone(&a.tier),
            guard: law
                .guard
                .as_ref()
                .map(|_| ply_codegen::law_root_name(index, "guard")),
            body: ply_codegen::law_root_name(index, "body"),
        };
        let rendered = |bindings: &[ply_prove::Binding]| {
            bindings
                .iter()
                .map(|b| format!("{} = {}", b.name, b.rendered))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match run_property(
            DefHash([index as u8 + 1; 32]),
            &binders,
            &names,
            &a.world,
            &wide,
            Span::DUMMY,
            &mut judge,
        ) {
            Discharge::Refuted(counterexample) => contradicted.push(format!(
                "`{label}` is proved and refuted at {}",
                rendered(&counterexample.bindings)
            )),
            Discharge::Unattempted(Gap::Raised {
                bindings,
                diagnostic,
                ..
            }) => contradicted.push(format!(
                "`{label}` is proved and raises `{}` at {}",
                diagnostic.message,
                rendered(&bindings)
            )),
            // A sample Ply could not finish checks nothing, so it cannot stand as agreement.
            Discharge::Faulted(fault) => contradicted.push(format!(
                "`{label}` is proved and Ply failed sampling it: `{}` at {}",
                fault.diagnostic.message,
                rendered(&fault.bindings)
            )),
            _ => {}
        }
    }
    (proved, contradicted)
}

#[test]
fn nothing_the_static_prover_proves_is_contradicted_by_a_wide_sample() {
    let (proved, contradicted) = contradictions(&audited(PROVED));
    assert!(
        contradicted.is_empty(),
        "a certificate covers a false claim — a defect in Ply:\n{}",
        contradicted.join("\n")
    );
    // A claim reaching a `Float` is false at `NaN`, so it must never have been among them.
    assert_eq!(
        proved,
        [
            "congruent",
            "excluded middle",
            "score is positive",
            "records are their fields",
            "or_else is a function",
        ]
    );
}
