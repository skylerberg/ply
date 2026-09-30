//! The claims family over more than one run: every `configure` begins a run of its own, whatever
//! the last one was left doing, and each run's static tier answers for that run's claims.

use crate::fixture::{handed, project};
use ply_eval::Value;
use ply_eval::host::{
    HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, MachineId, Pending,
};
use ply_machine::payload::{field_of, option, record};
use ply_span::{Diagnostic, Span, Symbol};
use ply_ty::{EffectAtom, Mode, Resource};
use std::path::Path;
use std::sync::Arc;

/// The family answers every operation in hand, so nothing here is ever pending.
struct InHand;

impl HostRuntime for InHand {
    fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
        Ok(None)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }

    fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
        unreachable!("the claims family answers in hand")
    }
}

fn ask(lent: &[(HostOp, Arc<dyn HostHandler>)], op: &str, args: Vec<Value>) -> Value {
    let (declared, handler) = lent
        .iter()
        .find(|(o, _)| o.op.as_str() == op)
        .unwrap_or_else(|| panic!("the family lends `{op}`"));
    let request = HostRequest {
        atom: EffectAtom::new("prover", Resource::Named(Symbol::new("claims")), Mode::Read),
        op: declared,
        args: &args,
        span: Span::DUMMY,
        machine: MachineId::next(),
        task: None,
        declared: None,
    };
    match handler.call(&InHand, &request) {
        Ok(HostAnswer::Value(value)) => value,
        Ok(HostAnswer::Pending(_)) => panic!("`{op}` answered later"),
        Err(d) => panic!("`{op}` refused: {}", d.message),
    }
}

/// What `ply prove` would configure over `root`, hermetic and uncached.
fn options(root: &Path) -> Value {
    record(vec![
        ("path", Value::str(root.display().to_string())),
        ("no_incremental", Value::Bool(false)),
        ("no_cache", Value::Bool(true)),
        ("std", Value::Bool(false)),
        ("jobs", option(None)),
        ("host", Value::Bool(false)),
        (
            "prove",
            record(vec![
                ("cases", option(None)),
                ("roots", option(None)),
                ("budget", option(None)),
                ("shrink_budget", option(None)),
                ("steps", option(None)),
            ]),
        ),
        (
            "sim",
            record(vec![
                ("seed", option(None)),
                ("mode", Value::str("dpor")),
                ("roots", option(None)),
                ("budget", option(None)),
                ("steps", option(None)),
                ("measure_reduction", Value::Bool(false)),
            ]),
        ),
    ])
}

/// The value inside an `Ok`.
fn ok(answer: Value) -> Value {
    match &answer {
        Value::Ctor { name, args } if name.as_str() == "Ok" => args[0].clone(),
        other => panic!("the answer is a refusal: {other:?}"),
    }
}

fn list(value: &Value) -> Vec<Value> {
    value
        .as_list(Span::DUMMY, "a list")
        .expect("a list")
        .iter()
        .cloned()
        .collect()
}

/// The decision a claim's reach names; a law the static tier sees always has one.
fn decision_of(reach: &Value) -> String {
    let Value::Ctor { name, args } = reach else {
        panic!("a reach is an `Option`: {reach:?}");
    };
    assert_eq!(
        name.as_str(),
        "Some",
        "the static tier saw nothing of a law"
    );
    field_of(&args[0], "decision", Span::DUMMY)
        .and_then(|d| d.as_str(Span::DUMMY, "a decision").map(str::to_string))
        .expect("a reach names its decision")
}

fn owners(collection: &Value) -> Vec<String> {
    let claims = field_of(collection, "claims", Span::DUMMY).expect("a collection's claims");
    list(claims)
        .iter()
        .map(|claim| {
            field_of(claim, "owner", Span::DUMMY)
                .and_then(|o| o.as_str(Span::DUMMY, "an owner").map(str::to_string))
                .expect("a claim's owner")
        })
        .collect()
}

const ONE_LAW: &str = r#"
law "addition commutes"
  forall (a: Int, b: Int) {
    a + b == b + a
  }
"#;

const TWO_LAWS: &str = r#"
law "zero is the identity"
  forall (n: Int) {
    n + 0 == n
  }

law "doubling is adding"
  forall (n: Int) {
    n * 2 == n + n
  }
"#;

/// A run left holding its machine after a discharge is ended by the next `configure`, and the next
/// collection is the new project's rather than a step of the old run.
#[test]
fn a_second_configuration_is_a_second_run_over_its_own_project() {
    let lent = ply_machine::claims::lent("claims");
    let mut seen: Vec<Vec<String>> = Vec::new();
    for (source, laws) in [(ONE_LAW, 1), (TWO_LAWS, 2)] {
        let dir = project(source);
        ask(
            &lent,
            "configure",
            vec![options(dir.path()), handed(dir.path())],
        );
        let collection = ok(ask(&lent, "collected", Vec::new()));
        let named = owners(&collection);
        assert_eq!(named.len(), laws, "{named:?}");
        let all = Value::list((0..laws).map(|i| Value::Int(i as i64)).collect());
        let choice = record(vec![
            ("claims", all.clone()),
            ("runs", all),
            ("read", Value::list(Vec::new())),
            ("domains", Value::list(Vec::new())),
        ]);
        let verdicts = ok(ask(&lent, "discharged", vec![choice]));
        let outcomes = field_of(&verdicts, "outcomes", Span::DUMMY).expect("the outcomes");
        assert_eq!(list(outcomes).len(), laws);
        // The static tier is asked about each claim on its own, after anything discharged it.
        let asked = Value::list((0..laws).map(|i| Value::Int(i as i64)).collect());
        let reached = ok(ask(&lent, "reaches", vec![asked]));
        let decisions: Vec<String> = list(&reached).iter().map(decision_of).collect();
        assert_eq!(decisions.len(), laws, "{decisions:?}");
        assert!(
            decisions.iter().all(
                |d| ["proved", "guard_unsatisfiable", "open", "budget_spent"].contains(&d.as_str())
            ),
            "{decisions:?}"
        );
        seen.push(named);
    }
    assert!(
        seen[1].iter().all(|owner| !seen[0].contains(owner)),
        "the second run collected the first project's claims: {seen:?}"
    );
}
