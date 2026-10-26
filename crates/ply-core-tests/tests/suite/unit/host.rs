use ply_core::host::*;
use ply_core::{Diagnostic, EffectAtom, Resource, Symbol, codes};
use std::sync::Arc;

struct Never;

impl HostHandler for Never {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        Err(
            Diagnostic::error(codes::INTERNAL_ERROR, "the test handler was called")
                .primary(req.span, "here"),
        )
    }
}

fn op(effect: &str, name: &str, resource: HostResource) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource,
        determinism: Determinism::Nondeterministic,
        linearity: Linearity::AtMostOnce,
        blocking: false,
        secrets: false,
        path: "test::handler",
    }
}

fn named(r: &str) -> HostResource {
    HostResource::Only(Resource::Named(Symbol::new(r)))
}

fn registry(ops: Vec<HostOp>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    for op in ops {
        registry.register(op, Arc::new(Never));
    }
    registry
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code).collect()
}

/// What `binding` makes of `effect.op[label]`: the path of what serves it, or why nothing does.
fn resolved(
    binding: &HostBinding,
    effect: &str,
    op: &str,
    label: Option<&str>,
) -> Result<&'static str, Unserved> {
    let label = label.map(Symbol::new);
    (binding.resolve(&Symbol::new(effect), &Symbol::new(op), label.as_ref())).map(|b| b.op.path)
}

/// A granted binding serves a perform by its effect's declared name, a reserved or a language
/// effect's by its program-wide one, and the first registration not withheld; where none serves
/// it, it says whether none is for it or each that is is withheld.
#[test]
fn a_granted_binding_resolves_by_name_and_says_why_it_serves_nothing() {
    let mut lent = HostRegistry::new();
    lent.register_withheld(
        op("db", "get", named("users")),
        Arc::new(Never),
        "under `--host`",
    );
    for registered in [
        op("db", "get", HostResource::Any),
        op("std.net.net", "send", named("socket")),
        op("clock", "now", HostResource::Any),
    ] {
        lent.register(registered, Arc::new(Never));
    }
    lent.register_withheld(
        op("db", "put", HostResource::Any),
        Arc::new(Never),
        "under `--host`",
    );
    let binding = lent.granted(Grants::default()).expect("nothing conflicts");
    assert_eq!(
        resolved(&binding, "store.db", "get", Some("users")),
        Ok("test::handler")
    );
    assert_eq!(
        resolved(&binding, "std.net.net", "send", Some("socket")),
        Ok("test::handler")
    );
    assert_eq!(
        resolved(&binding, "clock", "now", None),
        Ok("test::handler")
    );
    for (effect, op, label) in [
        ("app.net", "send", Some("socket")),
        ("std.net.net", "send", Some("other")),
        ("m.clock", "now", None),
    ] {
        assert_eq!(
            resolved(&binding, effect, op, label),
            Err(Unserved::Unregistered),
            "{effect}.{op}"
        );
    }
    assert_eq!(
        resolved(&binding, "store.db", "put", Some("users")),
        Err(Unserved::Withheld {
            path: "test::handler",
            served: "under `--host`"
        })
    );
}

#[test]
fn a_hermetic_binding_serves_nothing_and_names_what_would() {
    let binding = HostBinding::hermetic_with(registry(vec![op("db", "get", HostResource::Any)]));
    assert!(binding.is_hermetic());
    assert_eq!(
        resolved(&binding, "db", "get", Some("users")),
        Err(Unserved::Hermetic("test::handler"))
    );
    assert_eq!(
        binding.would_serve(
            &Symbol::new("db"),
            &Symbol::new("get"),
            Some(&Symbol::new("users"))
        ),
        Some("test::handler")
    );
    assert_eq!(
        resolved(&binding, "net", "send", None),
        Err(Unserved::Unregistered)
    );
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
    let atom = EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_core::Mode::Read,
    );
    use_.record(&atom, Determinism::Deterministic);
    assert!(
        !use_.acted(),
        "a deterministic handler answers a re-run the same"
    );
    use_.record(&atom, Determinism::Nondeterministic);
    assert_eq!(use_.operations, 2, "operations count, atoms deduplicate");
    assert_eq!(use_.atoms.atoms().count(), 1);
    assert!(!use_.is_empty());
    assert!(use_.acted());
}

#[test]
fn a_binding_of_grants_refuses_two_handlers_of_one_operation_and_label() {
    let twice = registry(vec![
        op("db", "get", named("users")),
        op("db", "get", named("users")),
    ]);
    match twice.granted(Grants::default()) {
        Ok(_) => panic!("two handlers of `db.get[users]` were bound"),
        Err(diagnostics) => assert_eq!(codes_of(&diagnostics), [codes::HOST_HANDLER_CONFLICT]),
    }
    let apart = registry(vec![
        op("db", "get", named("users")),
        op("db", "get", named("orders")),
        op("db", "get", HostResource::Any),
    ]);
    assert!(apart.granted(Grants::default()).is_ok());
    let mut withheld = HostRegistry::new();
    withheld.register_withheld(
        op("db", "get", named("users")),
        Arc::new(Never),
        "elsewhere",
    );
    withheld.register(op("db", "get", named("users")), Arc::new(Never));
    assert!(
        withheld.granted(Grants::default()).is_ok(),
        "a withheld registration serves nothing, so it conflicts with nothing"
    );
}
