use ply_eval::codes;
use ply_eval::host::{Determinism, HostOp, Linearity, Pending};
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

const DETERMINISTIC: &Rows = &[
    // A hash is a function of the password, the salt and the parameters.
    (
        "std.password.kdf",
        &["argon2", "bcrypt", "pbkdf2", "scrypt"],
    ),
];

const REPEATABLE: &Rows = &[
    ("certgen", &["issue"]),
    // A wait changes nothing outside the program: crossed twice, it waits twice.
    ("clock", &["now", "sleep"]),
    ("std.config.config", &["get", "secret"]),
    // A socket's port is fixed while it is open, so reading it again reads the same one.
    ("std.net.net", &["local_port"]),
    // What the machine says of itself is read, never taken.
    (
        "std.os.os",
        &[
            "arch",
            "cpus",
            "endian",
            "executable",
            "family",
            "hostname",
            "name",
            "pid",
            "pointer_bits",
            "user",
        ],
    ),
    // Hashing again computes the same bytes and changes nothing outside the program.
    (
        "std.password.kdf",
        &["argon2", "bcrypt", "pbkdf2", "scrypt"],
    ),
    // `--exec` is resolved once, before anything runs, so what it bound is read the same twice.
    ("std.process.process", &["args", "bound"]),
    // Every draw is independent of every other, so a run may take as many as it likes.
    ("std.random.entropy", &["below", "next"]),
    ("std.signal.signal", &["deadline_ms", "stopping"]),
    // Asking what a stream is, or how large the terminal is, changes neither.
    ("std.term.term", &["is_terminal", "size"]),
    ("std.time.time", &["elapsed_ms", "elapsed_us", "now_ms"]),
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

/// A registration for an operation a program's declaration lacks binds nothing there, so one that
/// names no operation at all, a typo or one side of a rename, is caught here instead: against the
/// prelude's effects and every shipped module's, as this tree's sources declare them.
#[test]
fn every_operation_the_host_registers_is_one_this_trees_sources_declare() {
    let imports: String = ply_machine::shipped_modules::names()
        .iter()
        .filter(|name| ply_eval::host::is_std(name))
        .map(|name| format!("import {name}\n"))
        .collect();
    let source = format!("{imports}\nfn main() -> Int = 0\n");
    let check = crate::support::answered::checked("app", &source).check;
    let undeclared: Vec<String> = registry()
        .ops()
        .filter(|op| {
            !check.effects.values().any(|effect| {
                ply_eval::host::registration_names(&op.effect, &effect.name, &effect.simple_name)
                    && effect.ops.contains_key(&op.op)
            })
        })
        .map(|op| format!("{op} ({})", op.path))
        .collect();
    assert!(
        undeclared.is_empty(),
        "the host registers for what neither the prelude nor a shipped module declares: {undeclared:#?}"
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
