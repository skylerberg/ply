//! An adversarial audit of `derive` and of `where derivable(D, a)`.

use assert_cmd::prelude::*;
use ply_span::codes;
use std::path::Path;
use std::process::Command;

fn write(dir: &Path, rel: &str, text: &str) {
    std::fs::write(dir.join(rel), text).unwrap();
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn output(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// `ply run` over a one-module project, returning what `main` printed.
fn run_main(source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", source);
    let out = ply(dir.path()).arg("run").output().unwrap();
    let text = output(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the fixture did not run:\n{text}"
    );
    text.trim().to_string()
}

fn codes_of(source: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", source);
    let loaded = ply_cli::load::load(dir.path());
    match loaded {
        Ok(_) => Vec::new(),
        Err(e) => e.diagnostics.iter().map(|d| d.code.to_string()).collect(),
    }
}

// --- Constraints: does the boundary hold? -----------------------------------

/// The claim in one table: every one of these instantiates a constraint with a type that does not
/// satisfy it, and every one must be `E0206` — at the call site or at the signature, but
/// *statically*, and never as something that checks here and fails deeper.
#[test]
fn no_unsatisfiable_constraint_is_accepted_at_a_boundary() {
    let attempts: &[&str] = &[
        // Directly.
        "fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs(1.5)",
        // Under a container the signature writes.
        "fn needs<a>(xs: List<a>) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs([1.5])",
        // Through a first-class use, where the constraint could have been lost with the function's
        // name.
        "fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn apply(f: (Float) -> Int, x: Float) -> Int = f(x)\n\
         fn go() -> Int = apply(needs, 1.5)",
        // Through a lambda, same question with a different binder.
        "fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = { let f = |x: Float| needs(x); f(1.5) }",
        // A body that assumes one constraint and calls something needing another: the error belongs
        // in the body, against the inner call.
        "fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn caller<a>(x: a) -> Int where derivable(json, a) = needs(x)",
        // A `Float` behind a nominal declaration.
        "type Money = Cents(Float)\n\
         fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs(Cents(1.5))",
        // Behind a recursive declaration.
        "type Chain = Nil | Link(Float, Chain)\n\
         fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs(Nil)",
        // Behind a type parameter of a declaration.
        "type Box<a> = B(a)\n\
         fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs(B(1.5))",
        // Two constrained hops, so the failure has to travel.
        "type Money = Cents(Float)\n\
         fn inner<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn outer<a>(x: a) -> Int where derivable(ord, a) = inner(x)\n\
         fn go() -> Int = outer(Cents(1.5))",
    ];
    for source in attempts {
        let codes = codes_of(source);
        assert!(
            codes.iter().any(|c| c == codes::NOT_DERIVABLE),
            "accepted a constraint it cannot satisfy (codes: {codes:?}) in:\n{source}"
        );
    }
}

/// The other half: a constraint that *is* satisfiable must not be reported, and a body must be able
/// to assume its own.
#[test]
fn a_satisfiable_constraint_is_assumed_inside_the_body() {
    for source in [
        "fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs(1)",
        "fn keyed<k>(x: k) -> Map<k, Int> where derivable(ord, k) = map_insert(map_new(), x, 1)\n\
         fn go() -> Int = map_len(keyed(\"a\"))",
        "fn inner<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn outer<a>(x: a) -> Int where derivable(ord, a) = inner(x)\n\
         fn go() -> Int = outer(1)",
        "type Money = Cents(Decimal)\n\
         fn needs<a>(x: a) -> Int where derivable(ord, a) = 1\n\
         fn go() -> Int = needs(Cents(1.5m))",
    ] {
        assert!(
            codes_of(source).is_empty(),
            "a satisfiable constraint was refused in:\n{source}"
        );
    }
}

/// A `Map` key is checked wherever the type is inferred rather than written, which is where a check
/// hung off the surface syntax would miss it.
#[test]
fn an_inferred_map_key_is_checked_like_a_written_one() {
    for source in [
        "fn go() -> Int = map_len(map_insert(map_new(), 1.5, 1))",
        "type Money = Cents(Float)\n\
         fn go() -> Int = map_len(map_insert(map_new(), Cents(1.5), 1))",
        "fn go() -> Int = map_len(map_of_entries([{key: 1.5, value: 1}]))",
        "fn go() -> Int = map_len(map_insert(map_new(), {rate: 1.5}, 1))",
        "fn go() -> Int = map_len(map_insert(map_new(), [1.5], 1))",
    ] {
        let codes = codes_of(source);
        assert!(
            codes.iter().any(|c| c == codes::NOT_DERIVABLE),
            "a `Float` reached a map key (codes: {codes:?}) in:\n{source}"
        );
    }

    // The control: a `Float` *value* is fine, because only the key is ordered.
    assert!(
        codes_of("fn go() -> Int = map_len(map_insert(map_new(), \"k\", 1.5))").is_empty(),
        "a `Float` value was refused"
    );
}

/// Adding a `where` clause narrows what a signature admits, so it must move the definition's hash —
/// otherwise a caller already checked against the unconstrained form is never rechecked and stays
/// accepted.
#[test]
fn a_constraint_is_in_the_hash_and_its_spelling_is_not() {
    let hashes = |source: &str| -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "m.ply", source);
        let loaded = ply_cli::load::load(dir.path()).expect("it checks");
        loaded.hashes.defs.values().map(|h| h.to_hex()).collect()
    };
    let bare = hashes("fn f<a>(x: a) -> Int = 1\n");
    let constrained = hashes("fn f<a>(x: a) -> Int where derivable(ord, a) = 1\n");
    assert_ne!(bare, constrained, "adding a `where` moved no hash");

    let two = hashes("fn f<a>(x: a) -> Int where derivable(ord, a), derivable(json, a) = 1\n");
    let swapped = hashes("fn f<a>(x: a) -> Int where derivable(json, a), derivable(ord, a) = 1\n");
    assert_eq!(two, swapped, "reordering two `where` clauses moved a hash");

    let renamed = hashes("fn f<t>(x: t) -> Int where derivable(ord, t) = 1\n");
    assert_eq!(
        constrained, renamed,
        "renaming the constrained type parameter moved a hash"
    );
}

// --- Derivation: is a derived dictionary canonical? -------------------------

/// A control, so the two failures below are about hijacking rather than about derivation being
/// broken.
#[test]
fn a_derivation_composed_through_the_module_binder_is_canonical() {
    let printed = run_main(
        "import std.json\n\
         pub type Order = { id: Int, note: String }\n\
         derive json for Order\n\
         pub fn main() -> String = json::to_string((order_json().encode)({id: 1, note: \"hi\"}))\n",
    );
    assert!(
        printed.contains("\\\"id\\\":1") && printed.contains("\\\"note\\\":\\\"hi\\\""),
        "the canonical encoding is not what came out: {printed}"
    );
}

/// `derive ord for T` emits `compare(a, b)` as a **bare** name, and ADR 0001 says a module's own
/// items shadow the prelude.
#[test]
fn a_derived_ord_is_the_languages_order_and_not_the_modules() {
    let printed = run_main(
        "pub type Point = P(Int)\n\
         derive ord for Point\n\
         fn compare<a>(x: a, y: a) -> Ordering = Greater\n\
         pub fn main() -> Ordering = (point_ord().compare)(P(1), P(1))\n",
    );
    assert!(
        printed.contains("Equal"),
        "a local `compare` replaced the language's order inside a derived \
         dictionary: `point_ord().compare(P(1), P(1))` is {printed}"
    );
}

/// The same hazard on the `json` deriver, reached through the import form rather than through the
/// prelude.
#[test]
fn a_derived_json_codec_cannot_be_supplied_by_the_deriving_module() {
    let printed = run_main(
        "import std.json (Json, JsonCodec, Str, object, field, decode_error, to_string)\n\
         pub type Order = { id: Int, note: String }\n\
         derive json for Order\n\
         fn string_json() -> JsonCodec<String> = {\n\
        \x20 encode: |s: String| Str(\"HIJACKED\"),\n\
        \x20 decode: |j: Json| Err(decode_error(\"no\")),\n\
         }\n\
         fn int_json() -> JsonCodec<Int> = {\n\
        \x20 encode: |i: Int| Str(\"ALSO HIJACKED\"),\n\
        \x20 decode: |j: Json| Err(decode_error(\"no\")),\n\
         }\n\
         pub fn main() -> String = to_string((order_json().encode)({id: 1, note: \"hi\"}))\n",
    );
    assert!(
        !printed.contains("HIJACKED"),
        "the deriving module supplied the leaf codecs its own derivation used: {printed}"
    );
}

/// `derive eq` is the shape that is safe, and knowing why is what says how the two above should be
/// fixed: it emits the `==` **operator**, which is a token rather than a name, so nothing in scope
/// can stand in for it.
#[test]
fn a_derived_eq_uses_an_operator_and_so_cannot_be_shadowed() {
    let printed = run_main(
        "pub type Point = P(Int)\n\
         derive eq for Point\n\
         fn eq<a>(x: a, y: a) -> Bool = false\n\
         pub fn main() -> Bool = (point_eq().eq)(P(1), P(1))\n",
    );
    assert!(
        printed.contains("true"),
        "a derived `eq` was shadowed too: {printed}"
    );
}
