use ply_core::host::{Determinism, HostOp, Linearity, Pending};
use ply_core::{Span, codes};
use ply_host::{Host, registry};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn the_trusted_computing_base_declares_everything_it_must() {
    let registry = registry();
    assert!(
        !registry.is_empty(),
        "a registry that loads nothing is indistinguishable from a registry that failed to load"
    );
    for op in registry.ops() {
        assert!(
            op.path.starts_with("ply_host::"),
            "`{op}` is identified as `{}`, which names no Rust path a reviewer can find",
            op.path
        );
    }
}

/// An effect and operations of it. A table of them is written as [`operations`] answers: a row an
/// effect, effects ascending and each row's operations ascending, so a family's rows are its own
/// lines wherever it is registered.
type Rows = [(&'static str, &'static [&'static str])];

/// The operations the registry holds that `holds` is true of.
fn operations(holds: impl Fn(&HostOp) -> bool) -> Vec<(String, Vec<String>)> {
    let mut by_effect: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for op in registry().ops().filter(|op| holds(op)) {
        by_effect
            .entry(op.effect.as_str().to_string())
            .or_default()
            .insert(op.op.as_str().to_string());
    }
    by_effect
        .into_iter()
        .map(|(effect, ops)| (effect, ops.into_iter().collect()))
        .collect()
}

fn written(rows: &Rows) -> Vec<(String, Vec<String>)> {
    rows.iter()
        .map(|(effect, ops)| {
            (
                effect.to_string(),
                ops.iter().map(|op| op.to_string()).collect(),
            )
        })
        .collect()
}

const DETERMINISTIC: &Rows = &[];

const REPEATABLE: &Rows = &[
    // A wait changes nothing outside the program: crossed twice, it waits twice.
    ("clock", &["now", "sleep"]),
];

/// A deterministic handler's test is cached with no binding to answer for it, so each one is a
/// claim that the answer is a function of the arguments alone.
#[test]
fn every_deterministic_operation_is_one_that_was_argued_for() {
    assert_eq!(
        operations(|op| op.determinism == Determinism::Deterministic),
        written(DETERMINISTIC)
    );
}

/// `Repeatable` is the one column that silently re-opens multi-shot resumption over the boundary.
#[test]
fn every_repeatable_operation_is_one_that_was_argued_for() {
    assert_eq!(
        operations(|op| op.linearity == Linearity::Repeatable),
        written(REPEATABLE)
    );
}

/// The host answers the clock a host made, and nothing else reaches it.
#[test]
fn the_host_answers_the_clock_it_registers() {
    let runtime = Host::new().runtime();
    for op in registry().ops() {
        assert_eq!(op.effect.as_str(), "clock", "`{op}`");
        if op.op.as_str() == "now" {
            ply_host::registry::answer(op, &*runtime, &[], Span::DUMMY)
                .unwrap_or_else(|d| panic!("`{op}` was refused: {}", d.message));
        }
    }
}

#[test]
fn a_runtime_refuses_a_token_it_did_not_mint() {
    let runtime = Host::new().runtime();
    let stray = Pending {
        token: 0,
        label: "stray",
    };
    assert_eq!(
        (ply_host::registry::run_of(&*runtime).expect("a host made the runtime"))
            .watch(&stray)
            .expect_err("a token nothing minted is refused")
            .code,
        codes::INTERNAL_ERROR
    );
}
