//! Cross-module content addressing.
//!
//! The claim under test is the one the whole language rests on: the namespace is
//! metadata over hashes. A cross-module reference normalizes to the referent's
//! `DefHash` exactly as a same-module one does, so renaming a definition is free
//! and — the stronger, newer property — so is *moving* one between modules.
//!
//! Everything here is written at the source level, because that is the surface a
//! user edits.

use ply_hash::{DefHash, HashOutput, hash_ast, hash_program_ast};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{Ident, ImportKind, Item, ModuleName, Program, TypeDefBody};
use ply_syntax::resolve::{Binding, Namespace, Resolved, Scope};

fn program_of(files: &[(&str, &str)]) -> Program {
    let inputs = files
        .iter()
        .enumerate()
        .map(|(i, (name, source))| (SourceId(i as u32), ModuleName::from_dotted(name), *source));
    match ply_syntax::parse_program(inputs) {
        Ok(program) => program,
        Err(diags) => panic!("program did not parse: {diags:#?}"),
    }
}

fn hashes(files: &[(&str, &str)]) -> HashOutput {
    let mut program = program_of(files);
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(resolved) => resolved,
        Err(diags) => panic!("program did not resolve: {diags:#?}"),
    };
    hash_program_ast(&program, &resolved).expect("program should hash")
}

/// Resolution for a program whose modules import each other, which the real
/// resolver rejects. Hashing consumes only what a name denotes, and that is
/// well defined even when the load order is not — so the two tests below can
/// show that a definition-level cycle spanning modules costs this crate nothing.
fn hashes_ignoring_cycles(files: &[(&str, &str)]) -> HashOutput {
    let program = program_of(files);
    let index: Vec<Symbol> = program
        .modules
        .iter()
        .map(|m| m.name.as_symbol().clone())
        .collect();
    let mut resolved = Resolved::default();
    for (owner, module) in program.modules.iter().enumerate() {
        let mut scope = Scope {
            module: module.name.clone(),
            ..Scope::default()
        };
        let mut bind = |ns: Namespace, name: &Ident| {
            scope.space_mut(ns).insert(
                name.name.clone(),
                Binding {
                    qualified: module.name.qualify(&name.name),
                    owner,
                    span: name.span,
                },
            );
        };
        for item in &module.items {
            match item {
                Item::Fn(d) => bind(Namespace::Value, &d.name),
                Item::Effect(d) => bind(Namespace::Effect, &d.name),
                Item::Type(d) => {
                    bind(Namespace::Type, &d.name);
                    if let TypeDefBody::Sum(variants) = &d.body {
                        for v in variants {
                            bind(Namespace::Value, &v.name);
                        }
                    }
                }
                Item::Test(_) | Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => {}
            }
        }
        for import in &module.imports {
            let target = import.module_name();
            let owner = index
                .iter()
                .position(|n| n == target.as_symbol())
                .expect("imported");
            if let (ImportKind::Module | ImportKind::Alias(_), Some(binder)) =
                (&import.kind, import.binder())
            {
                scope.modules.insert(binder, (owner, import.binder_span()));
            }
        }
        resolved.scopes.push(scope);
    }
    hash_program_ast(&program, &resolved).expect("program should hash")
}

#[track_caller]
fn def(out: &HashOutput, name: &str) -> DefHash {
    *out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition named `{name}`; have {:?}", out.defs.keys()))
}

// ---- the two headline properties ----

const A_BEFORE: &str = "\
pub fn shared(x: Int) -> Int = x + 1
fn helper(x: Int) -> Int = x - 1
";
const B_BEFORE: &str = "\
import a
pub fn use_b(x: Int) -> Int = a::shared(x) * 2
test \"b uses shared\" { assert_eq(use_b(1), 4) }
";
const C_BEFORE: &str = "\
import a
fn use_c(x: Int) -> Int = a::shared(x) + 3
test \"c uses shared\" { assert_eq(use_c(1), 5) }
";

fn before() -> HashOutput {
    hashes(&[("a", A_BEFORE), ("b", B_BEFORE), ("c", C_BEFORE)])
}

/// The property the module system exists to preserve: `shared` is called from
/// two other modules and is moved to a third, which rewrites two `import`s and
/// four qualified references, and not one hash in the program moves.
#[test]
fn moving_a_definition_between_modules_changes_no_hash() {
    let after = hashes(&[
        ("a", "fn helper(x: Int) -> Int = x - 1\n"),
        (
            "b",
            "import d\npub fn use_b(x: Int) -> Int = d::shared(x) * 2\n\
             test \"b uses shared\" { assert_eq(use_b(1), 4) }\n",
        ),
        (
            "c",
            "import d\nfn use_c(x: Int) -> Int = d::shared(x) + 3\n\
             test \"c uses shared\" { assert_eq(use_c(1), 5) }\n",
        ),
        ("d", "pub fn shared(x: Int) -> Int = x + 1\n"),
    ]);
    let before = before();

    assert_eq!(
        def(&before, "a.shared"),
        def(&after, "d.shared"),
        "the moved definition"
    );
    assert_eq!(
        def(&before, "a.helper"),
        def(&after, "a.helper"),
        "its former neighbour"
    );
    assert_eq!(def(&before, "b.use_b"), def(&after, "b.use_b"), "a caller");
    assert_eq!(
        def(&before, "c.use_c"),
        def(&after, "c.use_c"),
        "the other caller"
    );
    assert_eq!(before.tests, after.tests, "the tests that reach it");
}

#[test]
fn renaming_a_definition_imported_by_two_modules_changes_no_hash() {
    let after = hashes(&[
        ("a", &A_BEFORE.replace("shared", "compute")),
        ("b", &B_BEFORE.replace("shared", "compute")),
        ("c", &C_BEFORE.replace("shared", "compute")),
    ]);
    let before = before();

    assert_eq!(def(&before, "a.shared"), def(&after, "a.compute"));
    assert_eq!(def(&before, "b.use_b"), def(&after, "b.use_b"));
    assert_eq!(def(&before, "c.use_c"), def(&after, "c.use_c"));
    assert_eq!(before.tests, after.tests);
}

/// Renaming a module is renaming its file, so every importer's `import` line and
/// every qualified reference changes. None of that is a definition.
#[test]
fn renaming_a_module_changes_no_hash() {
    let after = hashes(&[
        ("helpers", A_BEFORE),
        (
            "b",
            &B_BEFORE
                .replace("a::", "helpers::")
                .replace("import a", "import helpers"),
        ),
        (
            "c",
            &C_BEFORE
                .replace("a::", "helpers::")
                .replace("import a", "import helpers"),
        ),
    ]);
    let before = before();

    assert_eq!(def(&before, "a.shared"), def(&after, "helpers.shared"));
    assert_eq!(def(&before, "b.use_b"), def(&after, "b.use_b"));
    assert_eq!(def(&before, "c.use_c"), def(&after, "c.use_c"));
    assert_eq!(before.tests, after.tests);
}

#[test]
fn visibility_and_imports_are_erased() {
    let before = before();

    let extra_pub = hashes(&[
        ("a", &A_BEFORE.replace("fn helper", "pub fn helper")),
        ("b", B_BEFORE),
        ("c", C_BEFORE),
    ]);
    let reordered_and_extra_imports = hashes(&[
        ("a", A_BEFORE),
        ("b", &B_BEFORE.replace("import a", "import c\nimport a")),
        ("c", C_BEFORE),
    ]);
    let aliased = hashes(&[
        ("a", A_BEFORE),
        (
            "b",
            &B_BEFORE
                .replace("import a", "import a as alpha")
                .replace("a::", "alpha::"),
        ),
        ("c", C_BEFORE),
    ]);
    let selective = hashes(&[
        ("a", A_BEFORE),
        (
            "b",
            &B_BEFORE
                .replace("import a", "import a (shared)")
                .replace("a::shared", "shared"),
        ),
        ("c", C_BEFORE),
    ]);

    for (what, after) in [
        ("adding `pub`", extra_pub),
        ("adding and reordering imports", reordered_and_extra_imports),
        ("`as`-renaming an import", aliased),
        ("importing selectively instead of qualifying", selective),
    ] {
        assert_eq!(before.defs, after.defs, "{what} moved a definition hash");
        assert_eq!(before.tests, after.tests, "{what} moved a test hash");
    }
}

// ---- a cross-module reference is an ordinary reference ----

#[test]
fn editing_a_definition_moves_its_dependents_in_other_modules() {
    let before = before();
    let after = hashes(&[
        ("a", &A_BEFORE.replace("x + 1", "x + 2")),
        ("b", B_BEFORE),
        ("c", C_BEFORE),
    ]);
    assert_ne!(def(&before, "a.shared"), def(&after, "a.shared"));
    assert_ne!(def(&before, "b.use_b"), def(&after, "b.use_b"));
    assert_ne!(def(&before, "c.use_c"), def(&after, "c.use_c"));
    assert_ne!(before.tests, after.tests);
    assert_eq!(
        def(&before, "a.helper"),
        def(&after, "a.helper"),
        "an unrelated neighbour"
    );
}

#[test]
fn structurally_identical_definitions_in_different_modules_share_a_hash() {
    let out = hashes(&[
        ("a", "pub fn increment(x: Int) -> Int = x + 1\n"),
        ("b", "pub fn succ(n: Int) -> Int = n + 1\n"),
        ("c", "pub fn plus_two(n: Int) -> Int = n + 2\n"),
    ]);
    assert_eq!(def(&out, "a.increment"), def(&out, "b.succ"));
    assert_ne!(def(&out, "a.increment"), def(&out, "c.plus_two"));
}

#[test]
fn a_definition_hashes_the_same_whether_or_not_it_crosses_a_module_boundary() {
    let split = hashes(&[
        ("a", "pub fn leaf(n: Int) -> Int = n + 1\n"),
        ("b", "import a\nfn stem(n: Int) -> Int = a::leaf(n) * 2\n"),
    ]);
    let together = hashes(&[(
        "a",
        "fn leaf(n: Int) -> Int = n + 1\nfn stem(n: Int) -> Int = leaf(n) * 2\n",
    )]);
    assert_eq!(def(&split, "a.leaf"), def(&together, "a.leaf"));
    assert_eq!(def(&split, "b.stem"), def(&together, "a.stem"));
}

#[test]
fn one_name_in_two_modules_is_two_definitions() {
    let out = hashes(&[
        ("a", "pub fn f(x: Int) -> Int = x + 1\n"),
        ("b", "pub fn f(x: Int) -> Int = x - 1\n"),
    ]);
    assert_ne!(def(&out, "a.f"), def(&out, "b.f"));
    assert_eq!(out.defs.len(), 2);
}

/// A module binder lives in its own namespace, so `a` the parameter cannot hide
/// `a` the module — and if it ever did, the reference would fall back to a free
/// name and this hash would drift.
#[test]
fn a_local_binder_does_not_hide_a_module_binder() {
    let shadowing = hashes(&[
        ("a", "pub fn shared(x: Int) -> Int = x + 1\n"),
        ("b", "import a\nfn g(a: Int) -> Int = a + a::shared(a)\n"),
    ]);
    let plain = hashes(&[
        ("a", "pub fn shared(x: Int) -> Int = x + 1\n"),
        ("b", "import a\nfn g(z: Int) -> Int = z + a::shared(z)\n"),
    ]);
    assert_eq!(def(&shadowing, "b.g"), def(&plain, "b.g"));
}

#[test]
fn deps_and_closures_are_keyed_by_the_program_wide_name() {
    let out = before();
    assert_eq!(
        out.deps[&Symbol::new("b.use_b")],
        vec![Symbol::new("a.shared")]
    );
    assert_eq!(out.deps[&Symbol::new("a.shared")], Vec::<Symbol>::new());

    let closure: Vec<String> = out.closure[&Symbol::new("b.use_b")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure, vec!["a.shared", "b.use_b"]);

    let test_closure: Vec<String> = out.closure[&Symbol::new("b.b uses shared")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(test_closure, vec!["a.shared", "b.b uses shared", "b.use_b"]);
}

// ---- tests are keyed per module ----

#[test]
fn identically_labelled_tests_in_two_modules_stay_distinct() {
    let out = hashes(&[
        (
            "a",
            "fn f() -> Int = 1\ntest \"same label\" { assert_eq(f(), 1) }\n",
        ),
        (
            "b",
            "fn g() -> Int = 2\ntest \"same label\" { assert_eq(g(), 2) }\n",
        ),
    ]);
    assert!(out.closure.contains_key(&Symbol::new("a.same label")));
    assert!(out.closure.contains_key(&Symbol::new("b.same label")));
    assert_ne!(out.tests[0], out.tests[1]);
}

/// Two identical tests in different modules are one computation and share one
/// cache entry. That is correct, not a collision to break.
#[test]
fn identical_tests_in_two_modules_share_a_hash() {
    let out = hashes(&[
        ("a", "test \"first\" { assert_eq(1, 1) }\n"),
        ("b", "test \"second\" { assert_eq(1, 1) }\n"),
    ]);
    assert_eq!(out.tests[0], out.tests[1]);
}

// ---- effects stay nominal across modules ----

const LOOK_ALIKE: &str = "\
pub effect db {
  write emit[r](v: Int) -> Int
}
pub fn log(v: Int) -> Int / {db.write[audit]} = db.emit[audit](v)
";

/// Two modules each declaring `effect db` declare two capabilities, not one —
/// and `a.log` and `b.log` are still one definition, because they differ only by
/// which of the two identical declarations they name. Separating them would
/// have to be done by a name or by a file position, and both of those make
/// unrelated edits move unrelated hashes.
///
/// The capabilities stay apart where it counts: a handler that discharges one
/// of them, with both in view, records which.
#[test]
fn performers_of_two_identically_declared_effects_are_one_definition() {
    let out = hashes(&[("a", LOOK_ALIKE), ("b", LOOK_ALIKE)]);
    assert_eq!(def(&out, "a.log"), def(&out, "b.log"));
}

#[test]
fn a_handler_records_which_modules_look_alike_effect_it_discharges() {
    let caught = |m: &str| {
        format!(
            "import a\nimport b\n\
             pub fn caught(v: Int) -> Int =\n\
               handle a::log(v) + b::log(v) with {{ {m}::db.emit[audit](x) -> x, }}\n"
        )
    };
    let via_a = hashes(&[("a", LOOK_ALIKE), ("b", LOOK_ALIKE), ("c", &caught("a"))]);
    let via_b = hashes(&[("a", LOOK_ALIKE), ("b", LOOK_ALIKE), ("c", &caught("b"))]);
    assert_ne!(def(&via_a, "c.caught"), def(&via_b, "c.caught"));
}

#[test]
fn performing_an_imported_effect_hashes_like_performing_a_local_one() {
    let split = hashes(&[
        ("a", "pub effect db {\n  read get[r](k: Int) -> Int\n}\n"),
        (
            "b",
            "import a\nfn read_one(k: Int) -> Int / {a::db.read[users]} = a::db.get[users](k)\n",
        ),
    ]);
    let together = hashes(&[(
        "a",
        "effect db {\n  read get[r](k: Int) -> Int\n}\n\
         fn read_one(k: Int) -> Int / {db.read[users]} = db.get[users](k)\n",
    )]);
    assert_eq!(def(&split, "b.read_one"), def(&together, "a.read_one"));
}

/// Look-alike effects are ranked by the name the source wrote, program-wide, so
/// the rank — and every performer's hash — survives one of them moving.
#[test]
fn moving_a_look_alike_effect_between_modules_changes_no_hash() {
    let audit = LOOK_ALIKE
        .replace("effect db", "effect audit")
        .replace("db.", "audit.");
    let before = hashes(&[("a", LOOK_ALIKE), ("b", &audit)]);
    let after = hashes(&[("a", LOOK_ALIKE), ("b", ""), ("z", &audit)]);
    assert_eq!(def(&before, "a.log"), def(&after, "a.log"));
    assert_eq!(def(&before, "b.log"), def(&after, "z.log"));
}

#[test]
fn moving_a_definition_past_look_alike_effects_changes_no_hash() {
    let before = hashes(&[
        ("a", LOOK_ALIKE),
        ("b", LOOK_ALIKE),
        ("c", "fn drifting(x: Int) -> Int = x * 3\n"),
    ]);
    let after = hashes(&[
        ("a", LOOK_ALIKE),
        ("b", LOOK_ALIKE),
        ("c", ""),
        ("d", "fn drifting(x: Int) -> Int = x * 3\n"),
    ]);
    assert_eq!(def(&before, "a.log"), def(&after, "a.log"));
    assert_eq!(def(&before, "b.log"), def(&after, "b.log"));
    assert_eq!(def(&before, "c.drifting"), def(&after, "d.drifting"));
}

#[test]
fn a_type_and_its_constructors_survive_a_move() {
    let before = hashes(&[
        ("a", "pub type Status = Active | Banned(String)\n"),
        (
            "b",
            "import a\nfn describe(s: a::Status) -> String = \
             match s { a::Active -> \"active\", a::Banned(why) -> why }\n",
        ),
    ]);
    let after = hashes(&[
        ("a", ""),
        (
            "b",
            "import c\nfn describe(s: c::Status) -> String = \
             match s { c::Active -> \"active\", c::Banned(why) -> why }\n",
        ),
        ("c", "pub type Status = Active | Banned(String)\n"),
    ]);
    assert_eq!(def(&before, "b.describe"), def(&after, "b.describe"));
}

// ---- a definition graph that spans modules ----

/// Import cycles are rejected upstream, but the definition graph is built over
/// resolved references and is module-blind, so an SCC that happens to span two
/// files must be hashed exactly like one that does not. Lifting the restriction
/// on cycles is meant to cost this crate nothing; these two tests are the
/// evidence.
#[test]
fn a_strongly_connected_component_may_span_modules() {
    let split = hashes_ignoring_cycles(&[
        ("a", "import b\npub fn ping(n: Int) -> Int = b::pong(n)\n"),
        (
            "b",
            "import a\npub fn pong(n: Int) -> Int = a::ping(n - 1)\n",
        ),
    ]);
    let together = hashes(&[(
        "a",
        "fn ping(n: Int) -> Int = pong(n)\nfn pong(n: Int) -> Int = ping(n - 1)\n",
    )]);

    assert_eq!(def(&split, "a.ping"), def(&together, "a.ping"));
    assert_eq!(def(&split, "b.pong"), def(&together, "a.pong"));
    assert_ne!(def(&split, "a.ping"), def(&split, "b.pong"));

    let closure: Vec<String> = split.closure[&Symbol::new("a.ping")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure, vec!["a.ping", "b.pong"]);
}

#[test]
fn a_cross_module_component_does_not_depend_on_module_order() {
    let forward = hashes_ignoring_cycles(&[
        ("a", "import b\npub fn ping(n: Int) -> Int = b::pong(n)\n"),
        (
            "b",
            "import a\npub fn pong(n: Int) -> Int = a::ping(n - 1)\n",
        ),
    ]);
    let backward = hashes_ignoring_cycles(&[
        (
            "b",
            "import a\npub fn pong(n: Int) -> Int = a::ping(n - 1)\n",
        ),
        ("a", "import b\npub fn ping(n: Int) -> Int = b::pong(n)\n"),
    ]);
    assert_eq!(def(&forward, "a.ping"), def(&backward, "a.ping"));
    assert_eq!(def(&forward, "b.pong"), def(&backward, "b.pong"));
}

#[test]
fn mutual_recursion_inside_one_module_is_unaffected() {
    let out = hashes(&[(
        "a",
        "fn is_even(n: Int) -> Bool = if n == 0 { true } else { is_odd(n - 1) }\n\
         fn is_odd(n: Int) -> Bool = if n == 0 { false } else { is_even(n - 1) }\n\
         fn caller() -> Bool = is_even(4)\n",
    )]);
    assert_ne!(def(&out, "a.is_even"), def(&out, "a.is_odd"));
    let closure: Vec<String> = out.closure[&Symbol::new("a.caller")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure, vec!["a.caller", "a.is_even", "a.is_odd"]);
}

// ---- the entry points agree ----

#[test]
fn a_one_module_program_hashes_like_a_bare_module() {
    let source = "fn double(n: Int) -> Int = n * 2\ntest \"t\" { assert_eq(double(2), 4) }\n";
    let module = ply_syntax::parse(SourceId(0), source).expect("parses");
    let bare = hash_ast(&module).expect("hashes");

    let mut program = Program::single(module);
    let resolved = ply_syntax::resolve(&mut program).expect("resolves");
    assert_eq!(bare, hash_program_ast(&program, &resolved).expect("hashes"));
}

#[test]
fn an_empty_program_hashes_to_nothing() {
    let mut program = Program::default();
    let resolved = ply_syntax::resolve(&mut program).expect("resolves");
    let out = hash_program_ast(&program, &resolved).expect("hashes");
    assert_eq!(out, HashOutput::default());
}

#[test]
fn a_duplicate_definition_is_still_reported_per_module() {
    let mut program = program_of(&[
        ("a", "fn f() -> Int = 1\nfn f() -> Int = 2\n"),
        ("b", "fn f() -> Int = 3\n"),
    ]);
    let resolved = ply_syntax::resolve(&mut program).expect("resolves");
    let diags = hash_program_ast(&program, &resolved)
        .expect_err("a duplicate within one module is an error");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, ply_span::codes::DUPLICATE_DEFINITION);
}

/// The entry point the CLI actually calls. It takes the check output for the
/// caller's convenience and must not consult it: a hash is a function of
/// resolved source structure alone.
#[test]
fn the_checked_entry_point_agrees_with_the_unchecked_one() {
    let mut program = program_of(&[("a", A_BEFORE), ("b", B_BEFORE), ("c", C_BEFORE)]);
    let resolved = ply_syntax::resolve(&mut program).expect("resolves");
    let check = match ply_core::check_program(&program, &resolved) {
        Ok(check) => check,
        Err(diags) => panic!("program did not typecheck: {diags:#?}"),
    };
    let out = ply_hash::hash_program(&program, &resolved, &check).expect("hashes");
    assert_eq!(out, hash_program_ast(&program, &resolved).expect("hashes"));
    assert_eq!(out.tests.len(), check.tests.len());
    assert_eq!(out.defs.len(), check.defs.len());
}

#[test]
fn hashing_a_multi_module_program_is_stable_across_runs() {
    let first = before();
    for _ in 0..8 {
        assert_eq!(first, before());
    }
}

/// `HashOutput::tests` is indexed in parallel with `CheckOutput::tests`, and
/// the two crates build that order independently. If they ever disagree, a test
/// is selected on another test's hash — a cached pass would silence an edit —
/// so the pairing is checked against a layout where the load order and the
/// dependency order are deliberately different.
#[test]
fn test_hashes_pair_with_the_checked_tests_whatever_the_load_order() {
    let paired = |files: &[(&str, &str)]| -> Vec<(String, DefHash)> {
        let mut program = program_of(files);
        let resolved = ply_syntax::resolve(&mut program).expect("resolves");
        let check = match ply_core::check_program(&program, &resolved) {
            Ok(check) => check,
            Err(diags) => panic!("program did not typecheck: {diags:#?}"),
        };
        let out = ply_hash::hash_program(&program, &resolved, &check).expect("hashes");
        assert_eq!(out.tests.len(), check.tests.len());
        check
            .tests
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), out.tests[i]))
            .collect()
    };

    // `a` imports `b`, so `a` loads first and is checked second.
    let importer =
        "import b\nfn one() -> Int = b::zero() + 1\ntest \"one\" { assert_eq(one(), 1) }\n";
    let base = "pub fn zero() -> Int = 0\ntest \"zero\" { assert_eq(zero(), 0) }\n";

    let mut inverted = paired(&[("a", importer), ("b", base)]);
    // Renaming the importing module to sort last makes the two orders agree.
    let mut agreeing = paired(&[("b", base), ("z", importer)]);

    inverted.sort();
    agreeing.sort();
    assert_eq!(
        inverted, agreeing,
        "a test was paired with another test's hash"
    );
}
