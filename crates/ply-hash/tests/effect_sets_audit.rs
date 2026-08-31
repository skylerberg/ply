//! Adversarial audit of the property ADR 0013 §1.5 calls the headline test: an
//! `effect set` name never enters a hash.
//!
//! `effect_sets.rs` establishes the property on the shape a reader expects — a
//! row on a top-level `fn`. This file attacks the places a row can also appear
//! and the ways a set can be written, because the failure that matters is not
//! "the feature does not work" but "the feature works everywhere the obvious
//! test looked and erases nothing in one corner". A single row position that
//! kept its alias would mean writing a signature more legibly re-runs tests,
//! which is the one thing the alias exists not to cost.

use ply_hash::{DefHash, HashOutput, hash_ast, hash_program_ast};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::ModuleName;

fn hashes(source: &str) -> HashOutput {
    let module = match ply_syntax::parse(SourceId(0), source) {
        Ok(m) => m,
        Err(diags) => panic!("source did not parse: {diags:#?}\n---\n{source}"),
    };
    hash_ast(&module).expect("module should hash")
}

#[track_caller]
fn def(source: &str, name: &str) -> DefHash {
    let out = hashes(source);
    *out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition named `{name}` in\n{source}"))
}

const EFFECTS: &str = "\
effect db {
  read  all[t]() -> Int
  write save[t](n: Int) -> Int
}
effect log {
  write line(n: Int) -> Int
}
effect clock {
  read now() -> Int
}
";

fn with_effects(rest: &str) -> String {
    format!("{EFFECTS}{rest}")
}

// --- rows in positions the obvious test does not reach ----------------------

/// A row on a function *type* — a parameter's, not the definition's. Expansion
/// walks types as well as signatures, and a set surviving here would make a
/// higher-order signature the one place legibility costs a rebuild.
#[test]
fn a_set_inside_a_parameters_function_type_is_erased() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn run(f: () -> Int / {Web}) -> Int / {Web} = f()\n",
    );
    let written = with_effects(
        "fn run(f: () -> Int / {db.read[users], log.write}) -> Int \
         / {db.read[users], log.write} = f()\n",
    );
    assert_eq!(def(&named, "run"), def(&written, "run"));
}

/// A row inside a `let`'s type annotation, which is a row at an arbitrary depth
/// of a body rather than in a signature.
#[test]
fn a_set_inside_a_let_annotation_is_erased() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn go() -> Int / {Web} {\n\
         \x20 let f: () -> Int / {Web} = || db.all[users]();\n\
         \x20 f()\n\
         }\n",
    );
    let written = with_effects(
        "fn go() -> Int / {db.read[users], log.write} {\n\
         \x20 let f: () -> Int / {db.read[users], log.write} = || db.all[users]();\n\
         \x20 f()\n\
         }\n",
    );
    assert_eq!(def(&named, "go"), def(&written, "go"));
}

/// A row inside a lambda parameter's type, which is a type inside an expression
/// inside a definition — the deepest place `walk_expr` has to reach.
#[test]
fn a_set_inside_a_lambda_parameter_type_is_erased() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn go() -> Int / {Web} = (|f: () -> Int / {Web}| f())(|| db.all[users]())\n",
    );
    let written = with_effects(
        "fn go() -> Int / {db.read[users], log.write} = \
         (|f: () -> Int / {db.read[users], log.write}| f())(|| db.all[users]())\n",
    );
    assert_eq!(def(&named, "go"), def(&written, "go"));
}

/// A row inside an effect operation's own declared type. An effect declaration
/// is hashed too, and a set left behind here would move the hash of the
/// capability rather than of a caller.
#[test]
fn a_set_inside_an_effect_operations_type_is_erased() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         effect runner {\n\
         \x20 write go(f: () -> Int / {Web}) -> Int\n\
         }\n",
    );
    let written = with_effects(
        "effect runner {\n\
         \x20 write go(f: () -> Int / {db.read[users], log.write}) -> Int\n\
         }\n",
    );
    let a = hashes(&named);
    let b = hashes(&written);
    assert_eq!(
        a.decls.get(&Symbol::new("runner")),
        b.decls.get(&Symbol::new("runner")),
        "an operation's declared row is part of the effect's hash, and a set in \
         it must expand exactly as the atoms would"
    );
}

// --- ways of writing the same set -------------------------------------------

/// A set with no members. `/ {Nothing}` is the closed empty row, which is what
/// `/ {}` already is — an alias that expanded to "no row at all" instead would
/// silently turn a published bound into an inferred one.
#[test]
fn a_set_that_expands_to_nothing_is_the_empty_row() {
    let named = with_effects(
        "effect set Nothing = {}\n\
         fn pure_fn(x: Int) -> Int / {Nothing} = x + 1\n",
    );
    let written = with_effects("fn pure_fn(x: Int) -> Int / {} = x + 1\n");
    assert_eq!(def(&named, "pure_fn"), def(&written, "pure_fn"));
}

/// An empty set beside written atoms contributes nothing and moves nothing.
#[test]
fn an_empty_set_beside_atoms_contributes_nothing() {
    let named = with_effects(
        "effect set Nothing = {}\n\
         fn f() -> Int / {Nothing, db.read[users]} = db.all[users]()\n",
    );
    let written = with_effects("fn f() -> Int / {db.read[users]} = db.all[users]()\n");
    assert_eq!(def(&named, "f"), def(&written, "f"));
}

/// The row and the set naming one atom between them. A set is a set: writing an
/// atom the set already holds is not a second atom.
#[test]
fn an_atom_written_beside_the_set_that_already_holds_it_moves_no_hash() {
    let both = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web, db.read[users]} = db.all[users]()\n",
    );
    let alias_only = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    assert_eq!(def(&both, "f"), def(&alias_only, "f"));
}

/// One set named twice in one row.
#[test]
fn naming_one_set_twice_in_a_row_moves_no_hash() {
    let twice = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web, Web} = db.all[users]()\n",
    );
    let once = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    assert_eq!(def(&twice, "f"), def(&once, "f"));
}

/// A set declared below the definition that names it. Expansion runs over the
/// whole file after it is parsed, so declaration order is not a rule anyone has
/// to learn — and if it were, moving a declaration would move a hash.
#[test]
fn a_set_declared_after_its_use_expands_the_same() {
    let after = with_effects(
        "fn f() -> Int / {Web} = db.all[users]()\n\
         effect set Web = {db.read[users], log.write}\n",
    );
    let before = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    assert_eq!(def(&after, "f"), def(&before, "f"));
}

/// Two hundred sets, each naming the one below it. The expander carries an
/// explicit stack precisely so a file cannot decide whether the parser
/// overflows; this is the test that says so, and it also says the expansion is
/// transitive rather than one level deep.
#[test]
fn a_two_hundred_deep_chain_of_sets_expands_to_the_written_row() {
    let mut source = String::from("effect set S0 = {db.read[users], log.write}\n");
    for i in 1..200 {
        source.push_str(&format!("effect set S{i} = {{S{}}}\n", i - 1));
    }
    source.push_str("fn f() -> Int / {S199} = db.all[users]()\n");
    let written = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&with_effects(&source), "f"), def(&written, "f"));
}

/// A diamond: two sets that both reach a third. The shared set's atoms are
/// spliced once, not twice, and the deduplication is by written form rather
/// than by identity of the include edge.
#[test]
fn a_diamond_of_sets_splices_the_shared_atoms_once() {
    let diamond = with_effects(
        "effect set Base = {db.read[users]}\n\
         effect set Left = {Base, log.write}\n\
         effect set Right = {Base, clock.read}\n\
         fn f() -> Int / {Left, Right} = db.all[users]()\n",
    );
    let written =
        with_effects("fn f() -> Int / {clock.read, db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&diamond, "f"), def(&written, "f"));
}

/// A set whose name is also an effect's name. The set lives in no namespace
/// `resolve` knows about, so `{db}` is the set and `db.read[users]` is the
/// atom, decided by the `.` and by nothing else.
#[test]
fn a_set_named_after_an_effect_is_still_only_a_set_in_a_row() {
    let shadowing = with_effects(
        "effect set db = {db.read[users], log.write}\n\
         fn f() -> Int / {db} = db.all[users]()\n",
    );
    let written = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&shadowing, "f"), def(&written, "f"));
}

/// A row that names a set *and* carries a row variable. The tail is part of the
/// row's meaning and the set is not, so the two must compose without either one
/// disturbing the other's encoding.
#[test]
fn a_set_beside_a_row_variable_is_erased() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn run<a | e>(f: () -> a / e) -> a / {Web | e} { log.line(1); f() }\n",
    );
    let written = with_effects(
        "fn run<a | e>(f: () -> a / e) -> a / {db.read[users], log.write | e} \
         { log.line(1); f() }\n",
    );
    assert_eq!(def(&named, "run"), def(&written, "run"));
}

// --- across modules ---------------------------------------------------------

fn program(files: &[(&str, &str)]) -> HashOutput {
    let inputs = files
        .iter()
        .enumerate()
        .map(|(i, (name, source))| (SourceId(i as u32), ModuleName::from_dotted(name), *source));
    let mut program = match ply_syntax::parse_program(inputs) {
        Ok(p) => p,
        Err(diags) => panic!("program did not parse: {diags:#?}"),
    };
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(r) => r,
        Err(diags) => panic!("program did not resolve: {diags:#?}"),
    };
    hash_program_ast(&program, &resolved).expect("program should hash")
}

#[track_caller]
fn in_program(out: &HashOutput, name: &str) -> DefHash {
    *out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition named `{name}`; have {:?}", out.defs.keys()))
}

const CAPABILITY: &str = "\
pub effect db {
  read  all[t]() -> Int
  write save[t](n: Int) -> Int
}
pub effect log {
  write line(n: Int) -> Int
}
";

/// A set holding atoms of an *imported* effect. The atoms are written with this
/// module's binder for the other module, so the hash has to come from what the
/// name denotes rather than from how it was spelled.
#[test]
fn a_set_over_an_imported_effect_hashes_as_the_written_row() {
    let named = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a\n\
             effect set Web = {a::db.read[users], a::log.write}\n\
             pub fn f() -> Int / {Web} = a::db.all[users]()\n",
        ),
    ]);
    let written = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a\n\
             pub fn f() -> Int / {a::db.read[users], a::log.write} = a::db.all[users]()\n",
        ),
    ]);
    assert_eq!(in_program(&named, "b.f"), in_program(&written, "b.f"));
}

/// The same set, reached through an import alias. Renaming the *module binder*
/// a set's atoms are written with is a namespace edit like any other.
#[test]
fn a_set_written_through_an_import_alias_hashes_the_same() {
    let plain = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a\n\
             effect set Web = {a::db.read[users], a::log.write}\n\
             pub fn f() -> Int / {Web} = a::db.all[users]()\n",
        ),
    ]);
    let aliased = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a as alpha\n\
             effect set Web = {alpha::db.read[users], alpha::log.write}\n\
             pub fn f() -> Int / {Web} = alpha::db.all[users]()\n",
        ),
    ]);
    assert_eq!(in_program(&plain, "b.f"), in_program(&aliased, "b.f"));
}

/// Moving an annotated definition to another module, with the set moved beside
/// it because a set is module-local. The definition's hash may not notice — the
/// M3 property, restated over the W3 feature that is most likely to break it,
/// since a set is the one piece of a signature that lives outside the
/// definition.
#[test]
fn moving_an_aliased_definition_between_modules_moves_no_hash() {
    let here = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a\n\
             effect set Web = {a::db.read[users], a::log.write}\n\
             pub fn f() -> Int / {Web} = a::db.all[users]()\n",
        ),
        ("c", "pub fn unrelated(x: Int) -> Int = x + 1\n"),
    ]);
    let there = program(&[
        ("a", CAPABILITY),
        ("b", "import a\npub fn placeholder(x: Int) -> Int = x\n"),
        (
            "c",
            "import a\n\
             effect set Web = {a::db.read[users], a::log.write}\n\
             pub fn f() -> Int / {Web} = a::db.all[users]()\n\
             pub fn unrelated(x: Int) -> Int = x + 1\n",
        ),
    ]);
    assert_eq!(in_program(&here, "b.f"), in_program(&there, "c.f"));
    assert_eq!(
        in_program(&here, "c.unrelated"),
        in_program(&there, "c.unrelated")
    );
}

/// Two modules that declare a set of the same name over *different* atoms. A
/// set is module-local, so neither expansion may leak into the other — the
/// failure this would produce is a published row that under-reports, in a file
/// whose bytes never moved.
#[test]
fn two_modules_may_declare_one_set_name_over_different_atoms() {
    let out = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a\n\
             effect set Web = {a::db.read[users]}\n\
             pub fn narrow() -> Int / {Web} = a::db.all[users]()\n",
        ),
        (
            "c",
            "import a\n\
             effect set Web = {a::db.read[users], a::log.write}\n\
             pub fn wide() -> Int / {Web} = a::db.all[users]()\n",
        ),
    ]);
    let narrow = program(&[
        ("a", CAPABILITY),
        (
            "b",
            "import a\npub fn narrow() -> Int / {a::db.read[users]} = a::db.all[users]()\n",
        ),
    ]);
    let wide = program(&[
        ("a", CAPABILITY),
        (
            "c",
            "import a\n\
             pub fn wide() -> Int / {a::db.read[users], a::log.write} = a::db.all[users]()\n",
        ),
    ]);
    assert_eq!(
        in_program(&out, "b.narrow"),
        in_program(&narrow, "b.narrow")
    );
    assert_eq!(in_program(&out, "c.wide"), in_program(&wide, "c.wide"));
    assert_ne!(in_program(&out, "b.narrow"), in_program(&out, "c.wide"));
}
