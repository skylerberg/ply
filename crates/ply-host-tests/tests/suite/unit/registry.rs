use ply_eval::codes;
use ply_eval::host::{Determinism, Linearity, Pending};
use ply_host::{Host, registry};

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

/// A deterministic handler's test is cached with no binding to answer for it, so each one is a
/// claim that the answer is a function of the arguments alone.
#[test]
fn every_deterministic_operation_is_one_that_was_argued_for() {
    let deterministic: Vec<String> = registry()
        .ops()
        .filter(|op| op.determinism == Determinism::Deterministic)
        .map(|op| op.to_string())
        .collect();
    assert_eq!(
        deterministic,
        [
            // A hash is a function of the password, the salt and the parameters.
            "std.password.kdf.argon2[..]",
            "std.password.kdf.scrypt[..]",
            "std.password.kdf.bcrypt[..]",
            "std.password.kdf.pbkdf2[..]",
        ]
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
    let repeatable: Vec<String> = registry()
        .ops()
        .filter(|op| op.linearity == Linearity::Repeatable)
        .map(|op| op.to_string())
        .collect();
    assert_eq!(
        repeatable,
        [
            // A socket's port is fixed while it is open, so reading it again reads the same one.
            "std.net.net.local_port[..]",
            "std.config.config.get[..]",
            "std.config.config.secret[..]",
            "task.spawn[..]",
            "task.join[..]",
            "task.yield[..]",
            "task.cancel[..]",
            "task.await[..]",
            "task.channel[..]",
            "task.send[..]",
            "task.recv[..]",
            "task.close[..]",
            "task.select[..]",
            // Every draw is independent of every other, so a run may take as many as it likes.
            "std.random.entropy.next[..]",
            "std.random.entropy.below[..]",
            "std.time.time.now_ms[..]",
            "std.time.time.elapsed_ms[..]",
            "std.time.time.elapsed_us[..]",
            "clock.now[..]",
            // A wait changes nothing outside the program: crossed twice, it waits twice.
            "clock.sleep[..]",
            // Hashing again computes the same bytes and changes nothing outside the program.
            "std.password.kdf.argon2[..]",
            "std.password.kdf.scrypt[..]",
            "std.password.kdf.bcrypt[..]",
            "std.password.kdf.pbkdf2[..]",
            "certgen.issue[..]",
            "std.signal.signal.stopping[..]",
            "std.signal.signal.deadline_ms[..]",
            "std.process.process.args[..]",
            // `--exec` is resolved once, before anything runs.
            "std.process.process.bound[..]",
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
            .watch(&stray)
            .expect_err("a token nothing minted is refused")
            .code,
        codes::INTERNAL_ERROR
    );
}
