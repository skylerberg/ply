use crate::fixture::port_check;
use ply_eval::host::*;
use ply_span::{Diagnostic, Symbol, codes};
use ply_ty::CheckOutput;
use ply_ty::{EffectAtom, Footprint, Resource};
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

fn check(source: &str) -> CheckOutput {
    port_check(&[("", source)])
}

/// Inside a named module, so every declared effect is program-wide `<module>.<name>` as a real load makes it.
fn qualified(module: &str, source: &str) -> CheckOutput {
    port_check(&[(module, source)])
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

/// Performs `db.get[users]` and `db.put[orders]`: two labels for `Any`, one real atom for `Only`.
const DB: &str = r#"
nondet effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

fn store(k: Int) -> Int / {db.write[orders]} = db.put[orders](k, 1)
"#;

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn an_empty_registry_binds_hermetically_clean() {
    let binding = registry(Vec::new())
        .bind(&check(DB))
        .expect("an empty registry is a legal binding");
    assert!(binding.footprint().is_empty());
    assert!(binding.listing().is_empty());
}

#[test]
fn hermetic_is_not_bound_and_serves_nothing() {
    let binding = HostBinding::hermetic_with(registry(vec![op("db", "get", named("users"))]));
    assert!(binding.is_hermetic());
    assert!(binding.footprint().is_empty());
    assert!(!binding.serves(&EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_ty::Mode::Read
    )));
}

#[test]
fn hermetic_still_names_the_handler_that_would_serve() {
    let binding = HostBinding::hermetic_with(registry(vec![op("db", "get", HostResource::Any)]));
    assert_eq!(
        binding.would_serve(
            &Symbol::new("db"),
            &Symbol::new("get"),
            Some(&Symbol::new("users"))
        ),
        Some("test::handler")
    );
    assert_eq!(
        binding.would_serve(&Symbol::new("net"), &Symbol::new("send"), None),
        None
    );
}

#[test]
fn an_unknown_effect_is_e0421() {
    let diagnostics = registry(vec![op("dbx", "get", named("users"))])
        .bind(&check(DB))
        .expect_err("nothing declares `dbx`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
    assert!(
        diagnostics[0].notes.iter().any(|n| n.contains("`db`")),
        "the nearest declared effect is named: {:?}",
        diagnostics[0].notes
    );
}

#[test]
fn an_unknown_operation_is_e0421_and_lists_the_declared_ones() {
    let diagnostics = registry(vec![op("db", "fetch", named("users"))])
        .bind(&check(DB))
        .expect_err("`db` has no `fetch`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
    let notes = diagnostics[0].notes.join(" ");
    assert!(
        notes.contains("`get`") && notes.contains("`put`"),
        "{notes}"
    );
}

/// A resource label the program never performs is a rename the Rust side did not follow.
#[test]
fn an_unperformed_resource_is_e0421_but_an_idle_any_is_not() {
    let diagnostics = registry(vec![op("db", "get", named("customers"))])
        .bind(&check(DB))
        .expect_err("nothing performs `db.read[customers]`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);

    let idle = r#"
nondet effect db {
  read get[r](key: Int) -> Int
}

fn pure_thing(n: Int) -> Int = n
"#;
    let binding = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&check(idle))
        .expect("a driver in a program that never queries is idle, not wrong");
    assert!(binding.footprint().is_empty());
    assert!(binding.listing().is_empty());
    assert_eq!(binding.listing().handlers, 1);
}

#[test]
fn a_resource_on_a_singleton_operation_is_e0421() {
    let source = r#"
nondet effect wall {
  read now() -> Int
}

fn stamp() -> Int / {wall.read} = wall.now()
"#;
    let diagnostics = registry(vec![op("wall", "now", named("clock"))])
        .bind(&check(source))
        .expect_err("`wall.now` takes no `[r]`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
}

#[test]
fn two_handlers_claiming_one_atom_is_e0422_naming_both() {
    let mut first = op("db", "get", named("users"));
    first.path = "test::first";
    let mut second = op("db", "get", HostResource::Any);
    second.path = "test::second";
    let diagnostics = registry(vec![first, second])
        .bind(&check(DB))
        .expect_err("both claim `db.read[users]`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_HANDLER_CONFLICT]);
    let notes = diagnostics[0].notes.join(" ");
    assert!(
        notes.contains("test::first") && notes.contains("test::second"),
        "{notes}"
    );
}

#[test]
fn a_nondet_handler_for_a_det_effect_is_e0423() {
    let source = r#"
effect store {
  read get[r](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {store.read[rows]} = store.get[rows](k)
"#;
    let diagnostics = registry(vec![op("store", "get", named("rows"))])
        .bind(&check(source))
        .expect_err("`effect store` is not declared `nondet`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_DETERMINISM_MISMATCH]);
    let notes = diagnostics[0].notes.join(" ");
    assert!(notes.contains("nondet effect store"), "{notes}");

    let mut deterministic = op("store", "get", named("rows"));
    deterministic.determinism = Determinism::Deterministic;
    registry(vec![deterministic])
        .bind(&check(source))
        .expect("a deterministic handler may serve an effect declared without `nondet`");
}

#[test]
fn any_expands_to_the_labels_the_program_uses() {
    let binding = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&check(DB))
    .expect("binds");

    let atoms: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.atom.to_string())
        .collect();
    assert_eq!(atoms, ["db.get[users]", "db.put[orders]"]);
    assert_eq!(binding.listing().handlers, 2);
    assert!(!atoms.iter().any(|a| a.contains('*')));
}

#[test]
fn any_does_not_cross_modes() {
    let binding = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&check(DB))
        .expect("binds");
    let atoms: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.atom.to_string())
        .collect();
    assert_eq!(atoms, ["db.get[users]"]);
}

/// A body with no written row publishes the operations it performs, and `Any` expands on those.
#[test]
fn any_expands_to_the_labels_an_inferred_row_names_by_operation() {
    let source = r#"
nondet effect db {
  read  get[r](key: Int) -> Int
  read  peek[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn lookup(k: Int) -> Int = db.get[users](k)

fn store(k: Int) -> Int = db.put[orders](k, 1)
"#;
    let binding = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "peek", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&check(source))
    .expect("binds");
    let rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.to_string())
        .collect();
    assert_eq!(
        rows,
        ["db.get[users]", "db.put[orders]"],
        "`peek` is never performed"
    );
    assert!(binding.serves(&EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_ty::Mode::Read,
    )));
    assert!(!binding.serves(&EffectAtom::operation(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_ty::Mode::Read,
        "peek",
    )));
}

#[test]
fn the_footprint_is_exactly_what_resolve_answers() {
    let binding = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&check(DB))
    .expect("binds");

    for atom in binding.footprint().atoms() {
        assert!(binding.serves(atom));
    }
    for row in &binding.listing().rows {
        let resource = match &row.resource {
            Resource::Named(r) => Some(r.clone()),
            Resource::Singleton => None,
        };
        let bound = binding
            .resolve(&row.effect, &row.op, resource.as_ref())
            .unwrap_or_else(|| panic!("{row} resolves"));
        assert_eq!(bound.atom, row.atom);
    }

    let absent = EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("customers")),
        ply_ty::Mode::Read,
    );
    assert!(!binding.serves(&absent));
    assert!(
        binding
            .resolve(
                &Symbol::new("db"),
                &Symbol::new("get"),
                Some(&Symbol::new("customers"))
            )
            .is_none()
    );
}

/// Two operations under one mode atom are two rows, each its own operation atom.
#[test]
fn two_operations_under_one_mode_atom_are_not_a_conflict() {
    let source = r#"
nondet effect db {
  read get[r](key: Int) -> Int
  read peek[r](key: Int) -> Int
}

fn a(k: Int) -> Int / {db.read[users]} = db.get[users](k)
fn b(k: Int) -> Int / {db.read[users]} = db.peek[users](k)
"#;
    let mut get = op("db", "get", named("users"));
    get.path = "test::get";
    let mut peek = op("db", "peek", named("users"));
    peek.path = "test::peek";
    let binding = registry(vec![get, peek])
        .bind(&check(source))
        .expect("two operations, one atom, no conflict");

    assert_eq!(binding.listing().rows.len(), 2);
    assert_eq!(binding.footprint().atoms().count(), 2);
    assert_eq!(
        binding
            .resolve(
                &Symbol::new("db"),
                &Symbol::new("peek"),
                Some(&Symbol::new("users"))
            )
            .expect("resolves")
            .op
            .path,
        "test::peek"
    );
}

#[test]
fn reaches_is_footprint_intersection() {
    let binding = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&check(DB))
        .expect("binds");
    let touched = Footprint::from_atoms([EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_ty::Mode::Read,
    )]);
    assert!(binding.reaches(&touched));
    assert!(!binding.reaches(&Footprint::empty()));
}

/// CI diffs the digest, so every column has to move it.
#[test]
fn the_digest_covers_every_column() {
    let base = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&check(DB))
        .expect("binds");
    let baseline = base.listing().digest();

    let mut repeatable = op("db", "get", HostResource::Any);
    repeatable.linearity = Linearity::Repeatable;
    let moved = registry(vec![repeatable])
        .bind(&check(DB))
        .expect("binds")
        .listing()
        .digest();
    assert_ne!(baseline, moved, "linearity alone moves the digest");

    let mut blocking = op("db", "get", HostResource::Any);
    blocking.blocking = true;
    let moved = registry(vec![blocking])
        .bind(&check(DB))
        .expect("binds")
        .listing()
        .digest();
    assert_ne!(baseline, moved, "blocking alone moves the digest");

    let mut path = op("db", "get", HostResource::Any);
    path.path = "test::other";
    let moved = registry(vec![path])
        .bind(&check(DB))
        .expect("binds")
        .listing()
        .digest();
    assert_ne!(baseline, moved, "the handler path alone moves the digest");
}

#[test]
fn the_digest_is_stable_under_registration_order() {
    let forward = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&check(DB))
    .expect("binds");
    let backward = registry(vec![
        op("db", "put", HostResource::Any),
        op("db", "get", HostResource::Any),
    ])
    .bind(&check(DB))
    .expect("binds");
    assert_eq!(forward.listing().digest(), backward.listing().digest());
    assert_eq!(
        forward.listing().digest_short(),
        backward.listing().digest_short()
    );
    assert!(forward.listing().digest_short().starts_with("b3:"));
    assert_eq!(forward.listing().digest_short().len(), 15);
}

#[test]
fn a_row_carries_the_declaration_it_was_checked_against() {
    let binding = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&check(DB))
        .expect("binds");
    let row = &binding.listing().rows[0];
    assert!(row.declared_nondet, "`effect db` is declared `nondet`");
    assert!(!row.deterministic);
    assert_eq!(row.linearity, Linearity::AtMostOnce);
}

#[test]
fn every_registration_failure_is_reported_at_once() {
    let mut unknown_effect = op("dbx", "get", named("users"));
    unknown_effect.path = "test::a";
    let mut unknown_op = op("db", "fetch", named("users"));
    unknown_op.path = "test::b";
    let diagnostics = registry(vec![unknown_effect, unknown_op])
        .bind(&check(DB))
        .expect_err("two mistakes");
    assert_eq!(diagnostics.len(), 2);
}

#[test]
fn a_registration_with_no_rust_path_is_e0421() {
    let mut anonymous = op("db", "get", named("users"));
    anonymous.path = "";
    let diagnostics = registry(vec![anonymous])
        .bind(&check(DB))
        .expect_err("a nameless member of the trusted computing base");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
}

#[test]
fn a_registration_names_the_declared_effect_and_resolves_to_the_program_wide_one() {
    let check = qualified(
        "store",
        r#"
nondet effect db {
  read get[r](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)
"#,
    );

    let binding = registry(vec![op("db", "get", named("users"))])
        .bind(&check)
        .expect("the declared name is what a registration spells");

    let rows = &binding.listing().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].effect.as_str(), "store.db");
    assert_eq!(rows[0].atom.effect.as_str(), "store.db");
    assert!(
        binding.serves(&rows[0].atom),
        "the atom the row names is the atom a perform resolves against"
    );
}

/// A compiler-shipped declaration's module is fixed, so `ply_host::tcp` can name `std.net.net` exactly.
#[test]
fn a_registration_may_spell_a_program_wide_name_under_the_reserved_root() {
    const DECL: &str = r#"
pub nondet effect net {
  write send[s](payload: Int) -> Int
}

pub fn out(x: Int) -> Int / {net.write[socket]} = net.send[socket](x)
"#;

    let shipped = qualified("std.net", DECL);
    let binding = registry(vec![op("std.net.net", "send", named("socket"))])
        .bind(&shipped)
        .expect("a shipped declaration is named in full");
    let rows = &binding.listing().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].effect.as_str(), "std.net.net");
    assert_eq!(rows[0].atom.effect.as_str(), "std.net.net");

    // The same text in a project module is a different capability.
    let copied = qualified("app", DECL);
    let binding = registry(vec![op("std.net.net", "send", HostResource::Any)])
        .bind(&copied)
        .expect("an unmatched `Any` registration is idle, not an error");
    assert!(
        binding.listing().rows.is_empty(),
        "a copied declaration bound the shipped handler: {:?}",
        binding.listing().rows
    );

    // And naming a specific resource says so, rather than binding quietly.
    let diagnostics = registry(vec![op("std.net.net", "send", named("socket"))])
        .bind(&copied)
        .expect_err("`std.net.net` is not what `app` declares");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
}

/// `store.db` is not what any declaration writes, so it resolves to nothing.
#[test]
fn a_registration_spelling_the_program_wide_name_is_e0421() {
    let check = qualified(
        "store",
        r#"
nondet effect db {
  read get[r](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)
"#,
    );

    let diagnostics = registry(vec![op("store.db", "get", named("users"))])
        .bind(&check)
        .expect_err("`store.db` is not how the declaration spells it");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
    assert!(
        diagnostics[0].notes.iter().any(|n| n.contains("store.db")),
        "{:?}",
        diagnostics[0].notes
    );
}

#[test]
fn one_registration_over_two_declarations_of_the_name_is_e0422() {
    let check = port_check(&[
        (
            "a",
            "nondet effect db {\n  read get[r](key: Int) -> Int\n}\n\nfn f(k: Int) -> Int / {db.read[users]} = db.get[users](k)\n",
        ),
        (
            "b",
            "nondet effect db {\n  read get[r](key: Int) -> Int\n}\n\nfn g(k: Int) -> Int / {db.read[users]} = db.get[users](k)\n",
        ),
    ]);

    let diagnostics = registry(vec![op("db", "get", named("users"))])
        .bind(&check)
        .expect_err("one handler cannot be two nominal effects");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_HANDLER_CONFLICT]);
    let notes = &diagnostics[0].notes;
    assert!(
        notes
            .iter()
            .any(|n| n.contains("a.db") && n.contains("b.db")),
        "{notes:?}"
    );
}

#[test]
fn host_use_records_what_actually_happened() {
    let mut use_ = HostUse::default();
    assert!(use_.is_empty());
    let atom = EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_ty::Mode::Read,
    );
    use_.record(&atom);
    use_.record(&atom);
    assert_eq!(use_.operations, 2, "operations count, atoms deduplicate");
    assert_eq!(use_.atoms.atoms().count(), 1);
    assert!(!use_.is_empty());
}

#[test]
fn preview_answers_what_would_bind_without_binding() {
    let registry = registry(vec![op("db", "get", HostResource::Any)]);
    let listing = registry.preview(&check(DB)).expect("previews");
    assert_eq!(listing.rows.len(), 1);
    let binding = registry.bind(&check(DB)).expect("binds");
    assert_eq!(binding.listing().digest(), listing.digest());
}
