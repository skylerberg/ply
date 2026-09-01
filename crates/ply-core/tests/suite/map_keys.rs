//! `Map`'s type surface: the twelve builtins' signatures, and the one rule that makes the iteration
//! order well defined — **a key type must be ordered**.

use ply_core::{CheckOutput, check_program, print_type};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

fn only_code(source: &str, code: &str) -> Diagnostic {
    let diags = errors(source);
    let matching: Vec<&Diagnostic> = diags.iter().filter(|d| d.code == code).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one {code} from:\n{source}\ngot {diags:#?}"
    );
    matching[0].clone()
}

fn sig(out: &CheckOutput, name: &str) -> String {
    let key = Symbol::new(format!("m.{name}"));
    print_type(&out.defs[&key].scheme.ty)
}

/// The whole surface at once: a signature that moved would otherwise be caught only by whichever
/// downstream test happened to use it.
#[test]
fn the_map_builtins_have_the_types_the_contract_states() {
    // The middle column is the probe's own generic list: the type and row variables the contract's
    // type mentions have to be *bound* now that the return type is written down, and `<| e>` is the
    // row namespace.
    let expected = [
        ("map_new", "<a, b>", "() -> Map<a, b>"),
        ("map_insert", "<a, b>", "(Map<a, b>, a, b) -> Map<a, b>"),
        ("map_get", "<a, b>", "(Map<a, b>, a) -> Option<b>"),
        ("map_contains", "<a, b>", "(Map<a, b>, a) -> Bool"),
        ("map_remove", "<a, b>", "(Map<a, b>, a) -> Map<a, b>"),
        ("map_len", "<a, b>", "(Map<a, b>) -> Int"),
        ("map_keys", "<a, b>", "(Map<a, b>) -> List<a>"),
        ("map_values", "<a, b>", "(Map<a, b>) -> List<b>"),
        (
            "map_entries",
            "<a, b>",
            "(Map<a, b>) -> List<{key: a, value: b}>",
        ),
        (
            "map_of_entries",
            "<a, b>",
            "(List<{key: a, value: b}>) -> Map<a, b>",
        ),
        ("map_merge", "<a, b>", "(Map<a, b>, Map<a, b>) -> Map<a, b>"),
        (
            "map_fold",
            "<a, b, c | e>",
            "(Map<a, b>, c, (c, a, b) -> c / e) -> c / e",
        ),
    ];
    // `fn probe_f() -> T = f` returns the builtin itself under a *written* return type
    // (`MISSING_SIGNATURE`), so the printed signature of the probe still carries the builtin's
    // whole type — but the builtin must now *unify* with the contract's type rather than merely
    // print as it, which is strictly stronger.
    let source: String = expected
        .iter()
        .map(|(name, generics, ty)| {
            format!("fn probe_{name}{generics}() -> {ty} where derivable(ord, a) = {name}\n")
        })
        .collect();
    let out = ok(&source);
    for (name, _, ty) in expected {
        assert_eq!(
            sig(&out, &format!("probe_{name}")),
            format!("() -> {ty}"),
            "{name}"
        );
    }
}

/// Required test: `Map<Float, v>` is `E0206`, naming `Float`.
#[test]
fn a_float_key_is_refused_where_it_is_written() {
    let d = only_code(
        "fn empty() -> Map<Float, Int> = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(d.message.contains("Float"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("NaN")),
        "the note must say why, not just that: {:?}",
        d.notes
    );
}

/// The same refusal where nothing wrote `Map<Float, _>` down: the key is a variable when the call
/// is walked, and only unification pins it.
#[test]
fn an_inferred_float_key_is_refused_too() {
    let d = only_code(
        "fn m() -> Int = map_len(map_insert(map_new(), 1.5, \"a\"))\n",
        codes::NOT_DERIVABLE,
    );
    assert!(d.message.contains("Float"), "{}", d.message);
}

/// A `Float` nested inside a key is named as the field that blocks it, not the key as a whole.
#[test]
fn a_float_inside_a_key_is_refused_and_named() {
    let d = only_code(
        "type Point = P(Float, Float)\nfn m() -> Map<Point, Int> = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(d.message.contains("Float"), "{}", d.message);
}

/// Required test: `Map<k, v>` under an unconstrained `k` is `E0206` naming the clause to add, and
/// adding it fixes it.
#[test]
fn an_unconstrained_type_parameter_is_refused_at_the_signature() {
    let d = only_code(
        "fn index<k, v>(xs: List<v>, key: (v) -> k) -> Map<k, v> = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("where derivable(ord, k)")),
        "the diagnostic must spell the clause to add: {:?}",
        d.notes
    );

    ok(
        "fn index<k, v>(xs: List<v>, key: (v) -> k) -> Map<k, v>\n  where derivable(ord, k)\n= map_new()\n",
    );
}

/// Inside the body the constraint is assumed, so a nested map built from the parameter needs no
/// second clause.
#[test]
fn a_body_may_assume_its_own_constraint() {
    ok(
        "fn wrap<k, v>(k: k, v: v) -> Map<k, Map<k, v>>\n  where derivable(ord, k)\n\
        = map_insert(map_new(), k, map_insert(map_new(), k, v))\n",
    );
}

/// A constraint for another deriver does not make a key ordered.
#[test]
fn a_json_constraint_does_not_order_a_key() {
    let d = only_code(
        "fn m<k>() -> Map<k, Int> where derivable(json, k) = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("where derivable(ord, k)")),
        "{:?}",
        d.notes
    );
}

#[test]
fn a_function_and_a_cell_are_refused_as_keys() {
    let f = only_code(
        "fn m() -> Map<(Int) -> Int, Int> = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(
        f.notes.iter().any(|n| n.contains("no order")),
        "{:?}",
        f.notes
    );
    let c = only_code(
        "fn m() -> Map<Cell<Int>, Int> = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(c.message.contains("Cell"), "{}", c.message);
}

/// Everything the contract lists as ordered, in one program, so a leaf dropped from the predicate
/// is caught here rather than by whatever used it.
#[test]
fn the_ordered_types_are_accepted() {
    ok("type Colour = Red | Green\n\
        type Pair<a> = P(a, a)\n\
        fn a() -> Map<Int, Int> = map_new()\n\
        fn b() -> Map<Bool, Int> = map_new()\n\
        fn c() -> Map<String, Int> = map_new()\n\
        fn d() -> Map<Bytes, Int> = map_new()\n\
        fn e() -> Map<Unit, Int> = map_new()\n\
        fn f() -> Map<Decimal, Int> = map_new()\n\
        fn g() -> Map<List<Int>, Int> = map_new()\n\
        fn h() -> Map<{x: Int, y: String}, Int> = map_new()\n\
        fn i() -> Map<Colour, Int> = map_new()\n\
        fn j() -> Map<Pair<Int>, Int> = map_new()\n\
        fn k() -> Map<Map<Int, Int>, Int> = map_new()\n\
        fn l() -> Map<Option<Int>, Int> = map_new()\n");
}

/// A recursive key type terminates rather than looping: the predicate refuses to enter a type it is
/// already inside, which is the same rule derivation needs.
#[test]
fn a_recursive_key_type_terminates() {
    ok("type Tree = Leaf | Node(Tree, Tree)\nfn m() -> Map<Tree, Int> = map_new()\n");
    let d = only_code(
        "type Bad = Leaf(Float) | Node(Bad)\nfn m() -> Map<Bad, Int> = map_new()\n",
        codes::NOT_DERIVABLE,
    );
    assert!(d.message.contains("Float"), "{}", d.message);
}

/// A map whose key nothing pinned is not an error.
#[test]
fn an_unsolved_key_is_not_reported() {
    ok("fn m() -> Int = map_len(map_new())\n");
}

/// The *value* type is unconstrained: only the key has to be ordered.
#[test]
fn the_value_type_carries_no_constraint() {
    ok("fn m() -> Map<Int, Float> = map_new()\n");
    ok("fn m<v>(v: v) -> Map<Int, v> = map_insert(map_new(), 1, v)\n");
}

/// The fixture `tests/fixtures/` owes for `E0206`'s map-key shape.
#[test]
fn the_map_key_fixture_produces_the_code_it_is_named_for() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/not_derivable_map_key.ply");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is part of the repository: {e}", path.display()));
    let diags = errors(&text);
    let found = diags
        .iter()
        .filter(|d| d.code == codes::NOT_DERIVABLE)
        .count();
    assert_eq!(found, 2, "expected both shapes to fire, got {diags:#?}");
}
