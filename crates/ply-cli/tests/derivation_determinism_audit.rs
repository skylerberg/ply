//! Determinism where derivation and the stdlib touch content addressing.
//!
//! Content addressing, the result cache and simulation replay all assume that a
//! definition's bytes are a function of the definition. W2 added two things that
//! could quietly break that and produce a green result while doing it: a
//! definition **nobody wrote**, generated from a declaration, and a body of
//! source that ships **inside the compiler** rather than inside the project.
//!
//! Each test below names the specific way one of them could stop being a
//! function of the program: the import form the generated body happens to be
//! written against, the names of the type parameters it happens to bind, the
//! order a `Map` happens to have been built in, the path the project happens to
//! sit at, and a stdlib whose digest moved under a warm cache.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

fn project(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    for (name, source) in files {
        std::fs::write(dir.path().join(name), source).expect("a project file");
    }
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the `ply` binary");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn run(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = ply(dir).args(args).output().expect("`ply` runs");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

/// `ply hash`, checked for success and returned whole. A separate process every
/// time, which is the point: everything a hash could accidentally depend on that
/// is not the program — an allocator address, a hasher's seed, an iteration
/// order that survived within one process — is a difference between two of
/// these and nothing else would show it.
fn hashes(dir: &Path) -> String {
    let (code, text) = run(dir, &["hash"]);
    assert_eq!(code, 0, "{text}");
    text
}

/// The line `ply hash` prints for one definition, without its module heading.
fn hash_of(text: &str, name: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.split_whitespace().nth(1) == Some(name))
        .unwrap_or_else(|| panic!("no definition named `{name}` in:\n{text}"));
    line.split_whitespace()
        .next()
        .expect("a hash column")
        .to_string()
}

// ------------------------------------------------------------ across processes

/// Everything a derived definition is built out of, hashed twice in two
/// processes and once more from a second directory. A generated body composes
/// through other codecs by *name*, so its bytes carry the hashes of everything
/// it reaches — which makes this one comparison cover the stdlib's definitions
/// as well as the project's.
const DERIVED: &str = r#"import std.json

pub type Colour = Red | Green | Blue
pub type Shape = Blank | Circle(Decimal) | Rect(Decimal, Decimal)
pub type Pair<a, b> = { fst: a, snd: b }
pub type Tree = Leaf | Node(Tree, Tree)
pub type Doc = {
  by_name: Map<String, Shape>,
  by_num: Map<Int, List<Colour>>,
  tree: Tree,
  maybe: Option<Pair<Int, String>>,
}

derive json for Colour
derive json for Shape
derive json for Pair
derive json for Tree
derive json for Doc
derive eq for Doc
derive ord for Colour

pub fn wire(d: Doc) -> String = string_of_bytes(json::encode_bytes(d, doc_json()))
"#;

#[test]
fn a_derived_program_hashes_identically_in_two_processes_and_two_directories() {
    let a = project(&[("m.ply", DERIVED)]);
    let first = hashes(a.path());
    let second = hashes(a.path());
    assert_eq!(
        first, second,
        "two runs of one program produced two hash sets"
    );

    // A different absolute path, same file names. Module names are derived from
    // paths relative to the root, so nothing about where the project sits may
    // reach a definition.
    let b = project(&[("m.ply", DERIVED)]);
    assert_eq!(
        first,
        hashes(b.path()),
        "a project's hashes moved with the directory it sat in"
    );
}

// ------------------------------------------------- what the module happened to write

/// A generated body is written against whatever spelling the module's `import`
/// gives it — `json::object`, `j::object`, or a bare `object` under a selective
/// import. Three different strings of source, and they must be one definition:
/// a free reference contributes its *referent's* hash, so the spelling is not
/// part of what the definition means.
///
/// If this ever fails, a project that reorganised its imports would re-run every
/// test that reaches a codec while changing nothing a client could see.
#[test]
fn a_generated_definitions_hash_does_not_depend_on_the_import_that_spelled_it() {
    let body = "\npub type A = { x: Int, y: String }\npub type B = Left(Int) | Right(A)\n\
                derive json for A\nderive json for B\n";
    let forms = [
        "import std.json",
        "import std.json as j",
        "import std.json (JsonCodec, Json, Member, DecodeError, object, field, variant, \
         variant_of, variant_value, unknown_variant, decode_and_then, int_json, string_json)",
    ];
    let mut seen: Option<(String, String)> = None;
    for form in forms {
        let dir = project(&[("m.ply", &format!("{form}{body}"))]);
        let text = hashes(dir.path());
        let pair = (hash_of(&text, "a_json"), hash_of(&text, "b_json"));
        match &seen {
            None => seen = Some(pair),
            Some(first) => assert_eq!(
                *first, pair,
                "the import form `{form}` changed a generated definition's hash"
            ),
        }
    }
}

/// The emitter picks its own binder prefix by walking away from the type
/// parameters' names, so `Pair<a, b>` and `Pair<d, e>` generate *different
/// source*. Normalization erases binders to de Bruijn levels, so they must be
/// one definition — renaming a type parameter is not a change to the program.
#[test]
fn renaming_a_type_parameter_does_not_move_a_generated_hash() {
    let with = |params: &str, fst: &str, snd: &str| {
        format!(
            "import std.json\n\
             pub type Pair<{params}> = {{ fst: {fst}, snd: {snd} }}\n\
             derive json for Pair\n"
        )
    };
    let plain = project(&[("m.ply", &with("a, b", "a", "b"))]);
    // `d` is the prefix the emitter reaches for first, so this fixture forces it
    // to walk to `d_` and rename every binder in the generated body.
    let shadowing = project(&[("m.ply", &with("d, e", "d", "e"))]);
    assert_eq!(
        hash_of(&hashes(plain.path()), "pair_json"),
        hash_of(&hashes(shadowing.path()), "pair_json"),
        "a type parameter's name reached a generated definition's hash"
    );
}

// ---------------------------------------------------------- fields and variants

/// Reordering a record's fields moves the generated definition's hash. ADR 0012
/// justifies that with "JSON object order is observable", and that reason is
/// **not** the true one: a JSON object is a `Map<String, Json>`, so the wire is
/// ascending by key whatever order the fields were declared in. What actually
/// moved is the order the decoder visits fields in, and therefore which of two
/// bad fields is reported first.
///
/// Both halves are pinned, because a reader who trusts the ADR's sentence would
/// conclude that a field reorder is a protocol change and it is not.
#[test]
fn reordering_two_fields_moves_the_hash_and_leaves_the_wire_alone() {
    let source = |fields: &str| {
        format!(
            "import std.json\n\
             pub type A = {{ {fields} }}\n\
             derive json for A\n\
             pub fn wire() -> String = \
               string_of_bytes(json::encode_bytes({{x: 1, y: \"hi\"}}, a_json()))\n\
             test \"the wire\" {{ assert_eq(wire(), \"{{\\\"x\\\":1,\\\"y\\\":\\\"hi\\\"}}\") }}\n"
        )
    };
    let declared = project(&[("m.ply", &source("x: Int, y: String"))]);
    let reordered = project(&[("m.ply", &source("y: String, x: Int"))]);

    assert_ne!(
        hash_of(&hashes(declared.path()), "a_json"),
        hash_of(&hashes(reordered.path()), "a_json"),
        "a field reorder changes the decoder's visiting order, so it has to move the hash"
    );
    for dir in [&declared, &reordered] {
        let (code, text) = run(dir.path(), &["test"]);
        assert_eq!(code, 0, "the wire is ascending by key either way:\n{text}");
    }
}

/// The headline invariant, now covering derivation, end to end at the level a
/// user sees: **renaming the type re-runs no test; renaming a variant re-runs
/// exactly the tests that reach it.** One corpus, one test, three runs.
///
/// The pair is the sharpest demonstration available that the hash tracks
/// meaning rather than text: a type's name is not in its encoding and a
/// variant's name is.
#[test]
fn renaming_the_type_re_runs_no_test_and_renaming_a_variant_re_runs_its_own() {
    let source = |ty: &str, codec: &str, variant: &str| {
        format!(
            "import std.json\n\
             \n\
             pub type Status = {variant} | Shipped(Int)\n\
             pub type {ty} = {{ id: Int, status: Status }}\n\
             derive json for Status\n\
             derive json for {ty}\n\
             \n\
             fn wire(o: {ty}) -> String = string_of_bytes(json::encode_bytes(o, {codec}()))\n\
             \n\
             test \"an order encodes\" {{\n  \
               assert_eq(wire({{id: 1, status: {variant}}}), \
                 \"{{\\\"id\\\":1,\\\"status\\\":\\\"{variant}\\\"}}\")\n\
             }}\n\
             \n\
             test \"an unrelated arithmetic fact\" {{ assert_eq(1 + 1, 2) }}\n"
        )
    };
    let dir = project(&[("m.ply", &source("Order", "order_json", "Placed"))]);

    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("selected 2 of 2 (0 cached)"), "{text}");

    // The type's name is not in the encoding, so nothing it reaches moved.
    std::fs::write(
        dir.path().join("m.ply"),
        source("Purchase", "purchase_json", "Placed"),
    )
    .unwrap();
    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("selected 0 of 2 (2 cached)"),
        "renaming the derived type re-ran a test:\n{text}"
    );

    // The variant's name *is* the tag, so exactly the test that reaches it moves.
    std::fs::write(
        dir.path().join("m.ply"),
        source("Purchase", "purchase_json", "Created"),
    )
    .unwrap();
    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("selected 1 of 2 (1 cached)"),
        "renaming a variant must re-run its test and no other:\n{text}"
    );
}

// ------------------------------------------------------------------ maps

/// A derived encoding containing a `Map` must be a function of the map's
/// contents and not of the history that built it — in one process, across
/// processes, and under both engines. A hash-ordered map would make the first
/// assertion flaky, the second wrong and the third an `E0503` on a correct
/// program, and every one of those failures reads as somebody else's bug.
#[test]
fn a_map_in_a_derived_encoding_is_byte_identical_however_it_was_built() {
    let dir = project(&[(
        "m.ply",
        r#"import std.json

pub type Doc = { m: Map<String, Int>, n: Map<Int, List<String>> }
derive json for Doc

fn build(ks: List<String>) -> Map<String, Int> =
  fold(ks, map_new(), |acc, k| map_insert(acc, k, string_len(k)))

fn nums(ks: List<Int>) -> Map<Int, List<String>> =
  fold(ks, map_new(), |acc, k| map_insert(acc, k, ["a", "b"]))

pub fn wire(ss: List<String>, ns: List<Int>) -> String =
  string_of_bytes(json::encode_bytes({m: build(ss), n: nums(ns)}, doc_json()))

fn ascending() -> List<String> = ["a", "bb", "ccc", "dddd"]
fn descending() -> List<String> = ["dddd", "ccc", "bb", "a"]
fn shuffled() -> List<String> = ["ccc", "a", "dddd", "bb"]

test "one map, one document" {
  assert_eq(wire(ascending(), [1, 2, 3]), wire(descending(), [3, 2, 1]));
  assert_eq(wire(ascending(), [1, 2, 3]), wire(shuffled(), [2, 3, 1]));
  assert_eq(wire(shuffled(), [3, 1, 2]),
            "{\"m\":{\"a\":1,\"bb\":2,\"ccc\":3,\"dddd\":4},\"n\":[{\"key\":1,\"value\":[\"a\",\"b\"]},{\"key\":2,\"value\":[\"a\",\"b\"]},{\"key\":3,\"value\":[\"a\",\"b\"]}]}")
}
"#,
    )]);

    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("0 failed, 1 passed"), "{text}");

    // Cached under one order and read back under another: the value is the same
    // value, so the second run selects nothing.
    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("selected 0 of 1 (1 cached)"), "{text}");

    let (code, text) = run(dir.path(), &["test", "--engine", "both", "--no-cache"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        !text.contains("E0503"),
        "the two engines disagreed about a derived encoding of a map:\n{text}"
    );
}

// ------------------------------------------------------------------ the stdlib

/// The stdlib digest is **in no cache key**, and this is the assertion that says
/// so from outside: a cache written under a digest that no longer matches warns
/// and re-runs nothing. A digest in a key would invalidate a project on an edit
/// to a `std` module it never imports, which is exactly the conservative
/// selection the whole design exists to beat.
#[test]
fn a_stdlib_digest_that_moved_invalidates_nothing() {
    let dir = project(&[(
        "m.ply",
        "import std.json\n\
         pub type A = { x: Int }\n\
         derive json for A\n\
         test \"a\" { assert_eq(string_of_bytes(json::encode_bytes({x: 1}, a_json())), \
           \"{\\\"x\\\":1}\") }\n\
         test \"b\" { assert_eq(2, 2) }\n",
    )]);
    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("selected 2 of 2 (0 cached)"), "{text}");

    std::fs::write(dir.path().join(".ply-cache/stdlib"), "b3:000000000000\n")
        .expect("the cache records a digest");

    let (code, text) = run(dir.path(), &["test"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("W0605") || text.contains("the modules that ship with `ply` moved"),
        "the run has to say the stdlib moved:\n{text}"
    );
    assert!(
        text.contains("selected 0 of 2 (2 cached)"),
        "a digest that moved re-ran a test, so it is in a cache key after all:\n{text}"
    );
}

/// Adding a stdlib module to a *program* — which is what an `import std.json` in
/// one file does to the whole run — must change no hash in a module that does
/// not import it. Nothing outside a definition's own reachable graph may enter
/// its hash, and the `std` prefix is the easiest thing in W2 to leak into one by
/// accident.
#[test]
fn pulling_the_stdlib_into_a_program_moves_no_hash_outside_it() {
    const PLAIN: &str = "pub fn total(xs: List<Int>) -> Int = fold(xs, 0, |a, x| a + x)\n\
                         pub fn label(n: Int) -> String = \"n=\" ++ int_to_string(n)\n";
    const USER: &str = "import std.json\n\
                        pub type A = { x: Int }\n\
                        derive json for A\n";

    let alone = project(&[("m.ply", PLAIN)]);
    let beside = project(&[("m.ply", PLAIN), ("other.ply", USER)]);

    let a = hashes(alone.path());
    let b = hashes(beside.path());
    assert!(
        !a.contains("std.json"),
        "a program importing nothing from `std` loaded a shipped module:\n{a}"
    );
    assert!(b.contains("std.json"), "{b}");
    for name in ["total", "label"] {
        assert_eq!(
            hash_of(&a, name),
            hash_of(&b, name),
            "`{name}` imports nothing from `std`, so loading the stdlib may not move it"
        );
    }
}

/// A `std` module's source is source: copying it into the project produces the
/// same definitions, keyed under a real path rather than under `<std>/json.ply`,
/// and neither key may reach the bytes.
///
/// The second half is the limit on that, and it is worth writing down: `derive
/// json` composes against the module spelled **`std.json`** and nothing else, so
/// a project holding a copy under any other name cannot host a derivation
/// against it. The codecs are the same definitions; the `derive` is not portable
/// with them.
#[test]
fn a_copied_stdlib_is_the_same_definitions_but_cannot_host_a_derivation() {
    let shipped = project(&[(
        "m.ply",
        "import std.json\npub type A = { x: Int, y: String }\nderive json for A\n",
    )]);
    let copied = project(&[
        ("json.ply", ply_std::JSON),
        (
            "m.ply",
            "import json\npub fn one() -> json::Json = json::Number(1m)\n",
        ),
    ]);

    let a = hashes(shipped.path());
    let b = hashes(copied.path());
    for name in ["object", "field", "int_json", "string_json", "parse"] {
        assert_eq!(
            hash_of(&a, name),
            hash_of(&b, name),
            "`{name}` moved between the shipped module and a copy of its source, so the `std` \
             prefix or the pseudo-path reached a hash"
        );
    }

    let derived_against_the_copy = project(&[
        ("json.ply", ply_std::JSON),
        (
            "m.ply",
            "import json\npub type A = { x: Int }\nderive json for A\n",
        ),
    ]);
    let (code, text) = run(derived_against_the_copy.path(), &["check"]);
    assert_ne!(code, 0, "{text}");
    assert!(
        text.contains("this module does not import `std.json`"),
        "a `derive json` against a copy has to say which module it wanted:\n{text}"
    );
}
