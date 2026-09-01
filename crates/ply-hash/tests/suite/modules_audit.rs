//! Adversarial audit of content addressing *under modules*.

use ply_hash::{DefHash, HashOutput, hash_ast, hash_program_ast};
use ply_span::{SourceId, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};

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

#[track_caller]
fn hashes(files: &[(&str, &str)]) -> HashOutput {
    let mut program = program_of(files);
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(resolved) => resolved,
        Err(diags) => panic!("program did not resolve: {diags:#?}"),
    };
    hash_program_ast(&program, &resolved).expect("program should hash")
}

/// The codes reported by resolution, or by inference when resolution is happy.
fn errors(files: &[(&str, &str)]) -> Vec<&'static str> {
    let mut program = program_of(files);
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(resolved) => resolved,
        Err(diags) => return diags.iter().map(|d| d.code).collect(),
    };
    match ply_core::check_program(&program, &resolved) {
        Ok(_) => Vec::new(),
        Err(diags) => diags.iter().map(|d| d.code).collect(),
    }
}

#[track_caller]
fn def(out: &HashOutput, name: &str) -> DefHash {
    *out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition named `{name}`; have {:?}", out.defs.keys()))
}

/// A module whose effect declaration is byte-identical to every other one built this way, so that
/// only the disambiguator can tell two of them apart.
fn look_alike(effect: &str) -> String {
    format!(
        "pub effect {effect} {{\n  write emit[r](v: Int) -> Int\n}}\n\
         pub fn log(v: Int) -> Int / {{{effect}.write[audit]}} = {effect}.emit[audit](v)\n"
    )
}

// False negatives: a real change that must move a hash.

/// Redirecting a call from `a::log` to `b::log` moves nothing *by itself*: `a` and `b` are
/// byte-identical, so the two programs differ by a consistent renaming of the two modules and
/// denote the same computation.
#[test]
fn redirecting_a_call_moves_a_definition_that_sees_both_look_alikes() {
    let forms = [
        (
            "qualified",
            "import a\nimport b\n\
             pub fn go(v: Int) -> Int =\n\
               handle a::log(v) + b::log(v) with { M::db.emit[audit](x) -> x, }\n",
            ("a", "b"),
        ),
        (
            "alias",
            "import a as p\nimport b as q\n\
             pub fn go(v: Int) -> Int =\n\
               handle p::log(v) + q::log(v) with { M::db.emit[audit](x) -> x, }\n",
            ("p", "q"),
        ),
    ];
    let (a, b) = (look_alike("db"), look_alike("db"));
    for (what, template, (first, second)) in forms {
        let via_a = hashes(&[("a", &a), ("b", &b), ("c", &template.replace('M', first))]);
        let via_b = hashes(&[("a", &a), ("b", &b), ("c", &template.replace('M', second))]);
        assert_ne!(
            def(&via_a, "c.go"),
            def(&via_b, "c.go"),
            "{what}: the caller did not move"
        );
    }
}

/// A selective import reaches the same definition by a different route, and the route is not part
/// of the hash.
#[test]
fn a_selective_import_and_a_qualified_path_reach_one_hash() {
    let a = look_alike("db");
    let qualified = hashes(&[
        ("a", &a),
        ("c", "import a\npub fn go(v: Int) -> Int = a::log(v)\n"),
    ]);
    let selective = hashes(&[
        ("a", &a),
        ("c", "import a (log)\npub fn go(v: Int) -> Int = log(v)\n"),
    ]);
    assert_eq!(def(&qualified, "c.go"), def(&selective, "c.go"));
}

/// Injectivity is the property that stops a stale pass, and it is owed to every definition that can
/// *observe* which look-alike it got.
#[test]
fn many_look_alike_effects_never_alias_where_a_definition_can_tell_them_apart() {
    for named in [true, false] {
        let mut files: Vec<(String, String)> = (0..8)
            .map(|i| {
                let effect = if named {
                    "db".to_string()
                } else {
                    format!("e{i}")
                };
                (format!("m{i}"), look_alike(&effect))
            })
            .collect();

        let imports: String = (0..8).map(|i| format!("import m{i}\n")).collect();
        let performs: Vec<String> = (0..8).map(|i| format!("m{i}::log(v)")).collect();
        for pick in 0..8 {
            files.push((
                format!("c{pick}"),
                format!(
                    "{imports}pub fn go(v: Int) -> Int =\n  handle {} with {{ m{pick}::db.emit[audit](x) -> x, }}\n",
                    performs.join(" + ")
                ),
            ));
        }

        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(m, s)| (m.as_str(), s.as_str()))
            .collect();
        let out = hashes(&borrowed);

        let performers: std::collections::BTreeSet<DefHash> =
            (0..8).map(|i| def(&out, &format!("m{i}.log"))).collect();
        assert_eq!(
            performers.len(),
            1,
            "the performers are one definition (named: {named})"
        );

        let observers: std::collections::BTreeSet<DefHash> =
            (0..8).map(|i| def(&out, &format!("c{i}.go"))).collect();
        assert_eq!(
            observers.len(),
            8,
            "two observers aliased (shared effect name: {named})"
        );
    }
}

/// The same test text in two modules is two different computations when its bare names denote
/// different definitions.
#[test]
fn identically_worded_tests_that_mean_different_things_do_not_share_a_hash() {
    let out = hashes(&[
        ("a", "fn f() -> Int = 1\ntest \"t\" { assert_eq(f(), 1) }\n"),
        ("b", "fn f() -> Int = 2\ntest \"t\" { assert_eq(f(), 1) }\n"),
    ]);
    assert_ne!(out.tests[0], out.tests[1]);
}

/// A bare name that meant a prelude builtin means the import once one is added, and the prelude's
/// `len` is not this `len`.
#[test]
fn a_selective_import_that_captures_a_prelude_name_moves_the_hash() {
    let provider = "pub fn len(xs: List<Int>) -> Int = 0\n";
    let before = hashes(&[
        ("a", provider),
        ("b", "fn count(xs: List<Int>) -> Int = len(xs)\n"),
    ]);
    let after = hashes(&[
        ("a", provider),
        (
            "b",
            "import a (len)\nfn count(xs: List<Int>) -> Int = len(xs)\n",
        ),
    ]);
    assert_ne!(def(&before, "b.count"), def(&after, "b.count"));
}

/// Import order must never decide what a name means, because nothing downstream of resolution can
/// see the order.
#[test]
fn two_imports_of_one_name_are_rejected_in_either_order() {
    let a = "pub fn f(x: Int) -> Int = x + 1\n";
    let b = "pub fn f(x: Int) -> Int = x - 1\n";
    for order in [
        "import a (f)\nimport b (f)\n",
        "import b (f)\nimport a (f)\n",
    ] {
        let c = format!("{order}fn g(x: Int) -> Int = f(x)\n");
        assert!(
            errors(&[("a", a), ("b", b), ("c", &c)]).contains(&codes::DUPLICATE_IMPORT),
            "one import silently won: {order:?}"
        );
    }
}

/// A local definition shadowing an import of the same name is the case where a silent winner would
/// be invisible in the source, so it is an error — and qualifying the reference both fixes it and
/// names a different definition.
#[test]
fn a_local_definition_colliding_with_an_import_is_ambiguous_and_qualifying_fixes_it() {
    let a = "pub fn f(x: Int) -> Int = x + 1\n";
    let ambiguous = "import a (f)\nfn f(x: Int) -> Int = x - 1\nfn g(x: Int) -> Int = f(x)\n";
    assert!(errors(&[("a", a), ("c", ambiguous)]).contains(&codes::AMBIGUOUS_IMPORT));

    let local = "import a\nfn f(x: Int) -> Int = x - 1\nfn g(x: Int) -> Int = f(x)\n";
    let imported = "import a\nfn f(x: Int) -> Int = x - 1\nfn g(x: Int) -> Int = a::f(x)\n";
    assert!(errors(&[("a", a), ("c", local)]).is_empty());
    assert_ne!(
        def(&hashes(&[("a", a), ("c", local)]), "c.g"),
        def(&hashes(&[("a", a), ("c", imported)]), "c.g"),
        "the qualified and the bare reference collapsed onto one hash"
    );
}

/// Hashing resolves names without consulting `pub`, which is only safe while a reference to a
/// private name is rejected before a hash is ever used.
#[test]
fn a_reference_to_a_private_name_is_rejected() {
    let a = "fn hidden(x: Int) -> Int = x + 1\n";
    let by_path = "import a\nfn g(x: Int) -> Int = a::hidden(x)\n";
    let by_name = "import a (hidden)\nfn g(x: Int) -> Int = hidden(x)\n";
    for c in [by_path, by_name] {
        assert!(
            errors(&[("a", a), ("c", c)]).contains(&codes::PRIVATE_NAME),
            "a private name was reachable from another module"
        );
    }
}

#[test]
fn moving_a_definition_while_editing_it_still_moves_its_hash() {
    let before = hashes(&[
        ("a", "pub fn shared(x: Int) -> Int = x + 1\n"),
        ("b", "import a\nfn use_it(x: Int) -> Int = a::shared(x)\n"),
    ]);
    let after = hashes(&[
        ("b", "import z\nfn use_it(x: Int) -> Int = z::shared(x)\n"),
        ("z", "pub fn shared(x: Int) -> Int = x + 2\n"),
    ]);
    assert_ne!(def(&before, "a.shared"), def(&after, "z.shared"));
    assert_ne!(def(&before, "b.use_it"), def(&after, "b.use_it"));
}

// False positives: a no-op edit that must not move a hash.

/// Adding a module changes the namespace and nothing else.
#[test]
fn adding_an_unrelated_module_changes_no_hash() {
    let base: &[(&str, &str)] = &[("m", &look_alike("db"))];
    let before = hashes(base);
    let with_extra = hashes(&[
        ("m", &look_alike("db")),
        (
            "aaa",
            "pub effect other {\n  read peek() -> Int\n}\npub fn f(x: Int) -> Int = x\n",
        ),
    ]);
    assert_eq!(def(&before, "m.log"), def(&with_extra, "m.log"));
}

#[test]
fn reordering_the_files_of_a_program_changes_no_hash() {
    let a = "pub fn shared(x: Int) -> Int = x + 1\n";
    let b = "import a\npub fn use_b(x: Int) -> Int = a::shared(x) * 2\n\
             test \"t\" { assert_eq(use_b(1), 4) }\n";
    let forward = hashes(&[("a", a), ("b", b)]);
    let backward = hashes(&[("b", b), ("a", a)]);
    assert_eq!(def(&forward, "a.shared"), def(&backward, "a.shared"));
    assert_eq!(def(&forward, "b.use_b"), def(&backward, "b.use_b"));
    assert_eq!(forward.tests, backward.tests);
}

// The regressions the disambiguator used to cause.

/// Moving a definition between modules changes no hash, and that has to survive two
/// identically-declared effects of the same simple name — the arrangement where a name-ordered rank
/// swaps the pair and drifts a module nobody edited.
#[test]
fn moving_a_look_alike_effect_of_the_same_name_changes_no_hash() {
    let before = hashes(&[("a", &look_alike("db")), ("b", &look_alike("db"))]);
    let after = hashes(&[("b", &look_alike("db")), ("z", &look_alike("db"))]);
    assert_eq!(
        def(&before, "b.log"),
        def(&after, "b.log"),
        "an untouched module drifted"
    );
    assert_eq!(
        def(&before, "a.log"),
        def(&after, "z.log"),
        "the moved definition drifted"
    );
}

/// Adding a file is not an edit to any existing definition, so it may not invalidate one — not even
/// when the new file declares an effect whose operations match an existing one exactly.
#[test]
fn adding_a_module_that_declares_a_look_alike_effect_changes_no_hash() {
    let before = hashes(&[("m", &look_alike("db"))]);
    let after = hashes(&[("m", &look_alike("db")), ("n", &look_alike("aaa"))]);
    assert_eq!(def(&before, "m.log"), def(&after, "m.log"));
}

/// The mirror image: deleting a module leaves the survivors alone.
#[test]
fn deleting_a_module_that_declares_a_look_alike_effect_changes_no_other_hash() {
    let before = hashes(&[("m", &look_alike("aaa")), ("n", &look_alike("zzz"))]);
    let after = hashes(&[("n", &look_alike("zzz"))]);
    assert_eq!(def(&before, "n.log"), def(&after, "n.log"));
}

/// Renaming is the property the whole language is sold on, and it has to hold for an effect renamed
/// *past* one of its look-alikes: `aaa` -> `zzz` crosses `mmm`, and `n.log`, which nobody touched,
/// must not move.
#[test]
fn renaming_an_effect_changes_no_hash_in_another_module() {
    let before = hashes(&[("m", &look_alike("aaa")), ("n", &look_alike("mmm"))]);
    let after = hashes(&[("m", &look_alike("zzz")), ("n", &look_alike("mmm"))]);
    assert_eq!(def(&before, "n.log"), def(&after, "n.log"));
}

/// `alpha` returns `Foo` and `beta` returns `Bar`; they are not look-alikes by any reading and
/// nothing may couple them.
#[test]
fn effects_whose_operations_differ_in_type_are_not_look_alikes() {
    let module = |effect: &str, ty: &str| {
        format!(
            "pub type Foo = {{ id: Int }}\n\
             pub type Bar = {{ tag: String }}\n\
             pub effect {effect} {{\n  read get[r](k: Int) -> {ty}\n}}\n\
             pub fn use_it(k: Int) -> {ty} / {{{effect}.read[x]}} = {effect}.get[x](k)\n"
        )
    };
    let before = hashes(&[
        ("m", &module("alpha", "Foo")),
        ("n", &module("beta", "Bar")),
    ]);
    let after = hashes(&[
        ("m", &module("zebra", "Foo")),
        ("n", &module("beta", "Bar")),
    ]);
    assert_eq!(def(&before, "n.use_it"), def(&after, "n.use_it"));
}

/// `hash_module` / `hash_ast` hash one module with no project around it, so every qualified
/// reference falls through to a free name.
#[test]
fn single_module_hashing_does_not_alias_qualified_references() {
    let of = |source: &str| {
        let module = ply_syntax::parse(SourceId(0), source).expect("parses");
        hash_ast(&module).expect("hashes").defs[&Symbol::new("go")]
    };
    assert_ne!(
        of("pub fn go(v: Int) -> Int = a::log(v)\n"),
        of("pub fn go(v: Int) -> Int = b::log(v)\n"),
    );
}

/// The one-module entry point may not accept a module whose references resolve only against other
/// modules: with nothing to resolve against it would write the binder the file happened to spell,
/// and a name is exactly what content addressing refuses to key on.
#[test]
fn a_lone_module_that_imports_is_refused_rather_than_hashed_by_name() {
    let module = ply_syntax::parse(SourceId(0), "import a.b\npub fn go() -> Int = b::log()\n")
        .expect("parses");
    let diags = hash_ast(&module).expect_err("a lone module cannot resolve an import");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, codes::UNKNOWN_MODULE);
    assert!(
        !diags[0].primary_span().expect("a real span").is_dummy(),
        "the import declaration is the thing to point at"
    );
    assert!(
        diags[0].notes.iter().any(|n| n.contains("hash_program")),
        "the diagnostic has to say what to do instead: {:?}",
        diags[0].notes
    );
}

/// Two files importing *different* modules under one binder are token-identical below the import,
/// so hashing by binder gives them one hash and one cache entry.
#[test]
fn two_modules_aliased_by_one_binder_are_not_hashed_alike() {
    let of = |source: &str| {
        let module = ply_syntax::parse(SourceId(0), source).expect("parses");
        hash_ast(&module)
    };
    assert!(of("import x.y as m\npub fn go(v: Int) -> Int = m::log(v)\n").is_err());
    assert!(of("import p.q as m\npub fn go(v: Int) -> Int = m::log(v)\n").is_err());
}

/// A selective import binds a *bare* name, so the reference does not even carry a qualifier that
/// could have kept the two apart.
#[test]
fn two_selectively_imported_names_are_not_hashed_alike() {
    let of = |source: &str| {
        let module = ply_syntax::parse(SourceId(0), source).expect("parses");
        hash_ast(&module)
    };
    assert!(of("import x.y (log)\npub fn go(v: Int) -> Int = log(v)\n").is_err());
    assert!(of("import p.q (log)\npub fn go(v: Int) -> Int = log(v)\n").is_err());
}

/// The failure that contradicts a required invariant outright: `as`-renaming an import must change
/// no hash, and hashing by binder makes it change one.
#[test]
fn an_as_rename_cannot_move_a_hash_because_neither_form_is_hashed() {
    let of = |source: &str| {
        let module = ply_syntax::parse(SourceId(0), source).expect("parses");
        hash_ast(&module)
    };
    assert!(of("import x.y\npub fn go(v: Int) -> Int = y::log(v)\n").is_err());
    assert!(of("import x.y as m\npub fn go(v: Int) -> Int = m::log(v)\n").is_err());

    // Through the entry point that has the namespace, the rename is free.
    let before = hashes(&[
        ("x.y", "pub fn log(v: Int) -> Int = v\n"),
        ("app", "import x.y\npub fn go(v: Int) -> Int = y::log(v)\n"),
    ]);
    let after = hashes(&[
        ("x.y", "pub fn log(v: Int) -> Int = v\n"),
        (
            "app",
            "import x.y as m\npub fn go(v: Int) -> Int = m::log(v)\n",
        ),
    ]);
    assert_eq!(def(&before, "app.go"), def(&after, "app.go"));
}

#[test]
fn every_import_of_a_lone_module_is_reported() {
    let module = ply_syntax::parse(
        SourceId(0),
        "import a\nimport b (f)\npub fn go() -> Int = f()\n",
    )
    .expect("parses");
    let diags = hash_ast(&module).expect_err("both imports are unresolvable here");
    assert_eq!(diags.len(), 2);
    assert!(diags.iter().all(|d| d.code == codes::UNKNOWN_MODULE));
}
