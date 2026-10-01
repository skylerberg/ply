use crate::fixture::Compiled;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_eval::{Compiled as _, Diagnostic, Span, Symbol, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// `table` is a constant, so what it answers crosses back in as memo words.
const SOURCE: &str = r#"
pub nondet effect net {
  write send[s](payload: Int) -> Int
}

fn deep(n: Int) -> Int = if n <= 0 { 0 } else { 1 + deep(n - 1) }

pub fn table() -> List<Int> = [1, 2, 3]

pub fn weigh(xs: List<Int>) -> Int = len(xs) + deep(50)

pub fn post(xs: List<Int>) -> Int / {net.send[socket]} = net.send[socket](len(xs))
"#;

/// A host handler that answers the ordinal of its own call.
#[derive(Default)]
struct Counter {
    calls: AtomicU64,
}

impl HostHandler for Counter {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(HostAnswer::Value(Value::Int(ordinal as i64)))
    }
}

fn call(machine: &mut ply_eval::Machine<'_>, name: &str, args: Vec<Value>) -> Value {
    machine
        .call(name, args, Span::DUMMY)
        .into_parts()
        .0
        .unwrap_or_else(|d| panic!("`{name}` raised: {d:#?}"))
}

/// A tier's steps are the last entry's calls, and an entry the memo answers makes none.
#[test]
fn a_pure_root_over_memo_words_is_answered_from_the_memo() {
    let compiled = Compiled::named("t", SOURCE);
    let (mut machine, tier) = compiled.machine_and_tier();
    let table = call(&mut machine, "t.table", Vec::new());

    let first = call(&mut machine, "t.weigh", vec![table.clone()]);
    assert!(tier.steps() > 0, "the first entry did not run the body");
    let second = call(&mut machine, "t.weigh", vec![table]);
    assert_eq!(second, first);
    assert_eq!(tier.steps(), 0, "the second entry ran the body again");
}

#[test]
fn an_effectful_root_over_memo_words_performs_every_time_it_is_entered() {
    let compiled = Compiled::named("t", SOURCE);
    let counter = Arc::new(Counter::default());
    let mut registry = HostRegistry::new();
    registry.register(
        HostOp {
            effect: Symbol::new("net"),
            op: Symbol::new("send"),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: false,
            secrets: false,
            path: "test::send",
        },
        counter.clone(),
    );
    let binding = registry.bind(&compiled.front.check).expect("binds");
    let (mut machine, tier) = compiled.machine_and_tier();
    machine.set_host_binding(Arc::new(binding));
    let table = call(&mut machine, "t.table", Vec::new());

    // The arguments are memo words: a pure root over them is answered from the memo.
    call(&mut machine, "t.weigh", vec![table.clone()]);
    call(&mut machine, "t.weigh", vec![table.clone()]);
    assert_eq!(tier.steps(), 0, "`table` did not answer memo words");

    for ordinal in 1..=2 {
        let sent = call(&mut machine, "t.post", vec![table.clone()]);
        assert_eq!(
            sent,
            Value::Int(ordinal),
            "entry {ordinal} was answered by an earlier one"
        );
        assert_eq!(
            machine.trace().performs(),
            1,
            "entry {ordinal} performed nothing"
        );
    }
    assert_eq!(counter.calls.load(Ordering::SeqCst), 2);
}
