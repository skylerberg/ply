use ply_eval::host::{Determinism, Linearity, Pending};
use ply_host::{Host, registry};
use ply_span::codes;

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
        assert_eq!(
            op.determinism,
            Determinism::Nondeterministic,
            "`{op}` claims to be a function of the program state; nothing in W1 is"
        );
    }
}

/// `Repeatable` is the one column that silently re-opens multi-shot resumption over the boundary.
#[test]
fn every_repeatable_operation_is_one_that_was_argued_for() {
    let repeatable: Vec<String> = registry()
        .ops()
        .filter(|op| op.linearity == Linearity::Repeatable)
        .map(|op| op.to_string())
        .collect();
    assert_eq!(
        repeatable,
        [
            "std.config.config.get[..]",
            "std.config.config.secret[..]",
            "task.spawn[..]",
            "task.join[..]",
            "task.yield[..]",
            "std.time.time.now_ms[..]",
            "std.time.time.elapsed_ms[..]",
            "std.signal.signal.stopping[..]",
            "std.signal.signal.deadline_ms[..]",
            "std.process.process.args[..]",
        ]
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
            .poll(&stray)
            .expect_err("a token nothing minted is refused")
            .code,
        codes::INTERNAL_ERROR
    );
}
