use ply_core::codes;
use ply_core::host::{Determinism, HostOp, Linearity, Pending};
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
    // A question to a resolver changes nothing it asks about.
    ("std.dns.dns", &["lookup", "reverse", "servers"]),
    // A socket's port and its two addresses are fixed while it is open, and so is who holds its
    // far end; its options are read as they stand, and reading them sets none.
    (
        "std.net.net",
        &[
            "local_address",
            "local_port",
            "options",
            "peer_address",
            "peer_certificate",
            "peer_credentials",
            "protocol",
        ],
    ),
    // `--exec` is resolved once, before anything runs, so what it bound is read the same twice.
    ("std.process.process", &["args", "bound"]),
    // Every draw is independent of every other, so a run may take as many as it likes.
    ("std.random.entropy", &["below", "next", "secret"]),
    ("std.signal.signal", &["deadline_ms", "stopping"]),
    // Asking what a stream is, or how large the terminal is, changes neither.
    ("std.term.term", &["is_terminal", "size"]),
    ("std.time.time", &["elapsed_ms", "elapsed_us", "now_ms"]),
    ("std.udp.udp", &["local_address", "options"]),
    (
        "task",
        &[
            "await", "cancel", "channel", "close", "join", "recv", "select", "send", "spawn",
            "yield",
        ],
    ),
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

#[test]
fn a_registry_and_a_runtime_come_from_one_host() {
    let host = Host::new();
    assert!(!host.registry().is_empty());
    let runtime = host.runtime();
    let stray = Pending {
        token: 0,
        label: "stray",
    };
    assert_eq!(
        runtime
            .watch(&stray)
            .expect_err("a token nothing minted is refused")
            .code,
        codes::INTERNAL_ERROR
    );
}
