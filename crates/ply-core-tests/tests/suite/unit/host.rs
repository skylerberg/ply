use ply_core::host::*;
use ply_core::{Resource, Symbol};

fn op(effect: &str, name: &str, resource: HostResource) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource,
        determinism: Determinism::Nondeterministic,
        linearity: Linearity::AtMostOnce,
        path: "test::handler",
    }
}

fn named(r: &str) -> HostResource {
    HostResource::Only(Resource::Named(Symbol::new(r)))
}

/// The path of what `binding` serves `effect.op[label]` by, where anything does.
fn resolved(
    binding: &HostBinding,
    effect: &str,
    op: &str,
    label: Option<&str>,
) -> Option<&'static str> {
    let label = label.map(Symbol::new);
    (binding.resolve(&Symbol::new(effect), &Symbol::new(op), label.as_ref())).map(|op| op.path)
}

/// A granted binding serves a perform by its effect's declared name, a reserved or a language
/// effect's by its program-wide one.
#[test]
fn a_granted_binding_resolves_by_name() {
    let mut lent = HostRegistry::new();
    for registered in [
        op("db", "get", HostResource::Any),
        op("std.net.net", "send", named("socket")),
        op("clock", "now", HostResource::Any),
    ] {
        lent.register(registered);
    }
    let binding = lent.granted(Grants::default());
    assert!(binding.is_granted());
    assert_eq!(
        resolved(&binding, "store.db", "get", Some("users")),
        Some("test::handler")
    );
    assert_eq!(
        resolved(&binding, "std.net.net", "send", Some("socket")),
        Some("test::handler")
    );
    assert_eq!(
        resolved(&binding, "clock", "now", None),
        Some("test::handler")
    );
    for (effect, op, label) in [
        ("app.net", "send", Some("socket")),
        ("std.net.net", "send", Some("other")),
        ("m.clock", "now", None),
    ] {
        assert_eq!(resolved(&binding, effect, op, label), None, "{effect}.{op}");
    }
}

#[test]
fn an_unbound_binding_serves_nothing() {
    let binding = HostBinding::unbound();
    assert!(!binding.is_granted());
    assert_eq!(resolved(&binding, "db", "get", Some("users")), None);
}

/// A registration by one of these names is matched by program-wide name, so the list must be the
/// effects every program is checked with, which the compiler's own tests pin as well.
#[test]
fn the_language_effects_are_the_ones_every_program_is_checked_with() {
    assert_eq!(
        LANGUAGE_EFFECTS,
        ["task", "clock", "random", "sim", "abort", "diverges"]
    );
}

#[test]
fn host_use_records_what_actually_happened() {
    let mut use_ = HostUse::default();
    assert!(use_.is_empty());
    use_.record(Determinism::Deterministic);
    assert!(
        !use_.acted(),
        "a deterministic handler answers a re-run the same"
    );
    use_.record(Determinism::Nondeterministic);
    assert_eq!(use_.operations, 2);
    assert!(!use_.is_empty());
    assert!(use_.acted());
}
