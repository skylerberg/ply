use ply_core::host::*;
use ply_core::{CheckOutput, Diagnostic, EffectAtom, Footprint, Mode, Resource, Symbol, codes};
use ply_test_support::{Sketch, atom, operation};
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

/// Performs `db.get[users]` and `db.put[orders]`: two labels for `Any`, one real atom for `Only`.
/// Inside the module `m`, so the declared effect is program-wide `m.db`, as a real load makes it.
///
/// ```text
/// nondet effect db {
///   read  get[r](key: Int) -> Int
///   write put[r](key: Int, value: Int) -> Int
/// }
/// fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)
/// fn store(k: Int) -> Int / {db.write[orders]} = db.put[orders](k, 1)
/// ```
fn db() -> CheckOutput {
    Sketch::program()
        .effect(
            "m",
            "db",
            true,
            &[("get", Mode::Read, true), ("put", Mode::Write, true)],
        )
        .def("m", "lookup", vec![atom("m.db", "users", Mode::Read)])
        .def("m", "store", vec![atom("m.db", "orders", Mode::Write)])
        .check()
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn an_empty_registry_binds_hermetically_clean() {
    let binding = registry(Vec::new())
        .bind(&db())
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
        ply_core::Mode::Read
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
        .bind(&db())
        .expect_err("nothing declares `dbx`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
    assert!(
        diagnostics[0].notes.iter().any(|n| n.contains("`m.db`")),
        "the nearest declared effect is named: {:?}",
        diagnostics[0].notes
    );
}

/// A program was checked against its own declaration of an effect, so it performs no operation
/// that declaration lacks: a program built before the operation was added, a module's or the
/// language's, binds as it did, and the registration serves nothing in it.
#[test]
fn an_operation_the_programs_declaration_lacks_binds_nothing() {
    let binding = registry(vec![
        op("db", "fetch", named("users")),
        op("db", "fetch", HostResource::Any),
        op("task", "nonesuch", HostResource::Any),
        op("db", "get", HostResource::Any),
    ])
    .bind(&db())
    .expect("each declaration is what the program was checked against");
    let served: Vec<&str> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.op.as_str())
        .collect();
    assert_eq!(served, ["get"]);
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

/// A module may declare an effect by a name the language declares one by, which hides the
/// language's there. A registration by that name finds one declaration, the language's, and
/// serves nothing of the module's.
#[test]
fn a_registration_by_a_language_effects_name_is_for_the_languages_alone() {
    // `nondet effect clock { read now() -> Int }` in `m`, and a definition that reads it.
    let own = Sketch::program()
        .effect("m", "clock", true, &[("now", Mode::Read, false)])
        .def("m", "stamp", vec![atom("m.clock", "", Mode::Read)])
        .check();
    let binding = registry(vec![op("clock", "now", HostResource::Any)])
        .bind(&own)
        .expect("one declaration is the language's");
    assert!(binding.listing().is_empty(), "{:?}", binding.listing());
    assert_eq!(
        binding.would_serve(&Symbol::new("m.clock"), &Symbol::new("now"), None),
        None
    );
}

/// A program built before `fetch` joined `db` binds to the operations it declares, and what was
/// withheld from it stays withheld.
#[test]
fn a_registry_kept_to_what_a_program_declares_drops_the_operations_its_effect_lacks() {
    let mut kept = registry(vec![
        op("db", "fetch", HostResource::Any),
        op("db", "get", HostResource::Any),
    ]);
    kept.register_withheld(op("db", "put", HostResource::Any), Arc::new(Never), "a run");
    kept.retain_declared(&db());
    assert_eq!(kept.len(), 2);

    let binding = kept.bind(&db()).expect("what is left is declared");
    let (db, users, orders) = (
        Symbol::new("db"),
        Symbol::new("users"),
        Symbol::new("orders"),
    );
    assert_eq!(
        binding.would_serve(&db, &Symbol::new("fetch"), Some(&users)),
        None
    );
    assert_eq!(
        binding.would_serve(&db, &Symbol::new("get"), Some(&users)),
        Some("test::handler")
    );
    assert_eq!(
        binding.withholds(&db, &Symbol::new("put"), Some(&orders)),
        Some("a run")
    );
    assert_eq!(
        binding.withholds(&db, &Symbol::new("get"), Some(&users)),
        None
    );
}

/// A resource label the program never performs is a rename the Rust side did not follow.
#[test]
fn an_unperformed_resource_is_e0421_but_an_idle_any_is_not() {
    let diagnostics = registry(vec![op("db", "get", named("customers"))])
        .bind(&db())
        .expect_err("nothing performs `db.read[customers]`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);

    let idle = Sketch::program()
        .effect("m", "db", true, &[("get", Mode::Read, true)])
        .def("m", "pure_thing", Vec::new())
        .check();
    let binding = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&idle)
        .expect("a driver in a program that never queries is idle, not wrong");
    assert!(binding.footprint().is_empty());
    assert!(binding.listing().is_empty());
    assert_eq!(binding.listing().handlers, 1);
}

#[test]
fn a_resource_on_a_singleton_operation_is_e0421() {
    let source = Sketch::program()
        .effect("m", "wall", true, &[("now", Mode::Read, false)])
        .def("m", "stamp", vec![atom("m.wall", "", Mode::Read)])
        .check();
    let diagnostics = registry(vec![op("wall", "now", named("clock"))])
        .bind(&source)
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
        .bind(&db())
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
    // `effect store`, declared without `nondet`.
    let source = Sketch::program()
        .effect("m", "store", false, &[("get", Mode::Read, true)])
        .def("m", "lookup", vec![atom("m.store", "rows", Mode::Read)])
        .check();
    let diagnostics = registry(vec![op("store", "get", named("rows"))])
        .bind(&source)
        .expect_err("`effect store` is not declared `nondet`");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_DETERMINISM_MISMATCH]);
    let notes = diagnostics[0].notes.join(" ");
    assert!(notes.contains("nondet effect store"), "{notes}");

    let mut deterministic = op("store", "get", named("rows"));
    deterministic.determinism = Determinism::Deterministic;
    registry(vec![deterministic])
        .bind(&source)
        .expect("a deterministic handler may serve an effect declared without `nondet`");
}

#[test]
fn any_expands_to_the_labels_the_program_uses() {
    let binding = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&db())
    .expect("binds");

    let atoms: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.atom.to_string())
        .collect();
    assert_eq!(atoms, ["m.db.get[users]", "m.db.put[orders]"]);
    assert_eq!(binding.listing().handlers, 2);
    assert!(!atoms.iter().any(|a| a.contains('*')));
}

#[test]
fn any_does_not_cross_modes() {
    let binding = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&db())
        .expect("binds");
    let atoms: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.atom.to_string())
        .collect();
    assert_eq!(atoms, ["m.db.get[users]"]);
}

/// A written mode atom and an inferred operation atom both name the label; the registration
/// serves it once rather than claiming it against itself.
#[test]
fn a_label_named_by_a_mode_atom_and_an_operation_atom_is_one_row() {
    // `wide` writes the mode atom `net.write[c]`, and `narrow` the operation atom `net.recv[c]`.
    let program = Sketch::program()
        .effect(
            "m",
            "net",
            true,
            &[("recv", Mode::Write, true), ("send", Mode::Write, true)],
        )
        .def("m", "wide", vec![atom("m.net", "c", Mode::Write)])
        .def(
            "m",
            "narrow",
            vec![operation("m.net", "c", Mode::Write, "recv")],
        )
        .check();
    let binding = registry(vec![
        op("net", "recv", HostResource::Any),
        op("net", "send", HostResource::Any),
    ])
    .bind(&program)
    .expect("one label under two atoms is not a conflict");
    let rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.to_string())
        .collect();
    assert_eq!(rows, ["m.net.recv[c]", "m.net.send[c]"]);
    assert_eq!(binding.footprint().atoms().count(), 2);

    let only = registry(vec![op("net", "recv", named("c"))])
        .bind(&program)
        .expect("`Only` names the same label once");
    assert_eq!(only.listing().rows.len(), 1);
}

/// A label-generic definition's row names a variable, and no perform ever carries one: the call
/// that fills it names the resource, and that atom is what a handler is bound against.
#[test]
fn a_label_a_caller_fills_is_not_a_row_of_its_own() {
    // `relay<[l]>` performs `net.send[l]`, and `answer` calls it as `relay[conn]`.
    let source = Sketch::program()
        .effect("m", "net", true, &[("send", Mode::Write, true)])
        .def(
            "m",
            "relay",
            vec![EffectAtom::operation(
                "m.net",
                Resource::Var(0),
                Mode::Write,
                "send",
            )],
        )
        .def(
            "m",
            "answer",
            vec![operation("m.net", "conn", Mode::Write, "send")],
        )
        .check();
    let binding = registry(vec![op("net", "send", HostResource::Any)])
        .bind(&source)
        .expect("the generic row binds nothing of its own");
    let rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.to_string())
        .collect();
    assert_eq!(rows, ["m.net.send[conn]"]);
}

/// A body with no written row publishes the operations it performs, and `Any` expands on those.
#[test]
fn any_expands_to_the_labels_an_inferred_row_names_by_operation() {
    // Rows written by operation: `lookup` performs `db.get[users]` and `store` `db.put[orders]`.
    let source = Sketch::program()
        .effect(
            "m",
            "db",
            true,
            &[
                ("get", Mode::Read, true),
                ("peek", Mode::Read, true),
                ("put", Mode::Write, true),
            ],
        )
        .def(
            "m",
            "lookup",
            vec![operation("m.db", "users", Mode::Read, "get")],
        )
        .def(
            "m",
            "store",
            vec![operation("m.db", "orders", Mode::Write, "put")],
        )
        .check();
    let binding = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "peek", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&source)
    .expect("binds");
    let rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.to_string())
        .collect();
    assert_eq!(
        rows,
        ["m.db.get[users]", "m.db.put[orders]"],
        "`peek` is never performed"
    );
    assert!(binding.serves(&EffectAtom::new(
        "m.db",
        Resource::Named(Symbol::new("users")),
        ply_core::Mode::Read,
    )));
    assert!(!binding.serves(&EffectAtom::operation(
        "m.db",
        Resource::Named(Symbol::new("users")),
        ply_core::Mode::Read,
        "peek",
    )));
}

#[test]
fn the_footprint_is_exactly_what_resolve_answers() {
    let binding = registry(vec![
        op("db", "get", HostResource::Any),
        op("db", "put", HostResource::Any),
    ])
    .bind(&db())
    .expect("binds");

    for atom in binding.footprint().atoms() {
        assert!(binding.serves(atom));
    }
    for row in &binding.listing().rows {
        let resource = match &row.resource {
            Resource::Named(r) => Some(r.clone()),
            Resource::Var(_) | Resource::Singleton | Resource::Every => None,
        };
        let bound = binding
            .resolve(&row.effect, &row.op, resource.as_ref())
            .unwrap_or_else(|| panic!("{row} resolves"));
        assert_eq!(bound.op.op, row.op);
    }

    let absent = EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("customers")),
        ply_core::Mode::Read,
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
    // `a` performs `db.get[users]` and `b` `db.peek[users]`, both under `db.read[users]`.
    let source = Sketch::program()
        .effect(
            "m",
            "db",
            true,
            &[("get", Mode::Read, true), ("peek", Mode::Read, true)],
        )
        .def("m", "a", vec![atom("m.db", "users", Mode::Read)])
        .def("m", "b", vec![atom("m.db", "users", Mode::Read)])
        .check();
    let mut get = op("db", "get", named("users"));
    get.path = "test::get";
    let mut peek = op("db", "peek", named("users"));
    peek.path = "test::peek";
    let binding = registry(vec![get, peek])
        .bind(&source)
        .expect("two operations, one atom, no conflict");

    assert_eq!(binding.listing().rows.len(), 2);
    assert_eq!(binding.footprint().atoms().count(), 2);
    assert_eq!(
        binding
            .resolve(
                &Symbol::new("m.db"),
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
        .bind(&db())
        .expect("binds");
    let touched = Footprint::from_atoms([EffectAtom::new(
        "m.db",
        Resource::Named(Symbol::new("users")),
        ply_core::Mode::Read,
    )]);
    assert!(binding.reaches(&touched));
    assert!(!binding.reaches(&Footprint::empty()));
}

#[test]
fn a_footprint_that_reaches_only_deterministic_handlers_reaches_nothing_that_acts() {
    let deterministic = HostOp {
        determinism: Determinism::Deterministic,
        ..op("db", "get", HostResource::Any)
    };
    let binding = registry(vec![deterministic]).bind(&db()).expect("binds");
    let touched = Footprint::from_atoms([EffectAtom::new(
        "db",
        Resource::Named(Symbol::new("users")),
        ply_core::Mode::Read,
    )]);
    assert!(!binding.is_hermetic(), "the handler is bound");
    assert!(!binding.reaches(&touched));
}

/// CI diffs the digest, so every column has to move it.
#[test]
fn the_digest_covers_every_column() {
    let base = registry(vec![op("db", "get", HostResource::Any)])
        .bind(&db())
        .expect("binds");
    let baseline = base.listing().digest();

    let mut repeatable = op("db", "get", HostResource::Any);
    repeatable.linearity = Linearity::Repeatable;
    let moved = registry(vec![repeatable])
        .bind(&db())
        .expect("binds")
        .listing()
        .digest();
    assert_ne!(baseline, moved, "linearity alone moves the digest");

    let mut blocking = op("db", "get", HostResource::Any);
    blocking.blocking = true;
    let moved = registry(vec![blocking])
        .bind(&db())
        .expect("binds")
        .listing()
        .digest();
    assert_ne!(baseline, moved, "blocking alone moves the digest");

    let mut path = op("db", "get", HostResource::Any);
    path.path = "test::other";
    let moved = registry(vec![path])
        .bind(&db())
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
    .bind(&db())
    .expect("binds");
    let backward = registry(vec![
        op("db", "put", HostResource::Any),
        op("db", "get", HostResource::Any),
    ])
    .bind(&db())
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
        .bind(&db())
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
    let mut unperformed = op("db", "get", named("customers"));
    unperformed.path = "test::b";
    let diagnostics = registry(vec![unknown_effect, unperformed])
        .bind(&db())
        .expect_err("two mistakes");
    assert_eq!(diagnostics.len(), 2);
}

#[test]
fn a_registration_with_no_rust_path_is_e0421() {
    let mut anonymous = op("db", "get", named("users"));
    anonymous.path = "";
    let diagnostics = registry(vec![anonymous])
        .bind(&db())
        .expect_err("a nameless member of the trusted computing base");
    assert_eq!(codes_of(&diagnostics), [codes::HOST_OPERATION_UNKNOWN]);
}

#[test]
fn a_registration_names_the_declared_effect_and_resolves_to_the_program_wide_one() {
    // `db` declared in the module `store`, and read there.
    let check = Sketch::program()
        .effect("store", "db", true, &[("get", Mode::Read, true)])
        .def(
            "store",
            "lookup",
            vec![atom("store.db", "users", Mode::Read)],
        )
        .check();

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
    // `app` writes through the shipped `std.net`'s `net`.
    let shipped = Sketch::program()
        .effect("std.net", "net", true, &[("send", Mode::Write, true)])
        .def(
            "app",
            "out",
            vec![atom("std.net.net", "socket", Mode::Write)],
        )
        .check();
    let binding = registry(vec![op("std.net.net", "send", named("socket"))])
        .bind(&shipped)
        .expect("a shipped declaration is named in full");
    let rows = &binding.listing().rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].effect.as_str(), "std.net.net");
    assert_eq!(rows[0].atom.effect.as_str(), "std.net.net");

    // The same text in a project module is a different capability.
    // The same declaration's text, in `app`.
    let copied = Sketch::program()
        .effect("app", "net", true, &[("send", Mode::Write, true)])
        .def("app", "out", vec![atom("app.net", "socket", Mode::Write)])
        .check();
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
    // `db` declared in the module `store`, and read there.
    let check = Sketch::program()
        .effect("store", "db", true, &[("get", Mode::Read, true)])
        .def(
            "store",
            "lookup",
            vec![atom("store.db", "users", Mode::Read)],
        )
        .check();

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
    // Two modules each declaring `db`, each reading its own.
    let check = Sketch::program()
        .effect("a", "db", true, &[("get", Mode::Read, true)])
        .effect("b", "db", true, &[("get", Mode::Read, true)])
        .def("a", "f", vec![atom("a.db", "users", Mode::Read)])
        .def("b", "g", vec![atom("b.db", "users", Mode::Read)])
        .check();

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

#[test]
fn preview_answers_what_would_bind_without_binding() {
    let registry = registry(vec![op("db", "get", HostResource::Any)]);
    let listing = registry.preview(&db()).expect("previews");
    assert_eq!(listing.rows.len(), 1);
    let binding = registry.bind(&db()).expect("binds");
    assert_eq!(binding.listing().digest(), listing.digest());
}
