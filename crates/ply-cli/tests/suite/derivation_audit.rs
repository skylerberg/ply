//! An adversarial audit of derived codecs, written against the wire.
//!
//! The failure this file hunts is silent: a derived codec that does not
//! round-trip corrupts data with no error anywhere, and nothing else in the
//! system notices. `derivation.rs` in `ply-core` proves a generated body
//! *checks*; `json_endpoint.rs` proves one shape reaches a socket. Neither says
//! `decode(encode(x)) == x` over the space of shapes the deriver accepts, and
//! that is the claim a payload milestone lives on.
//!
//! Every assertion below runs the real binary over a real project, because the
//! bytes are the artifact. A codec that only round-trips inside the language has
//! demonstrated nothing about a payload.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn project(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    for (name, source) in files {
        std::fs::write(dir.path().join(name), source).expect("a project file");
    }
    dir
}

fn one(source: &str) -> TempDir {
    project(&[("m.ply", source)])
}

/// A `derive` writes definitions nobody authored, so `MISSING_SIGNATURE`
/// (`E0126`) exempts them — `Checker::require_written_signature` keys that on
/// `FnDef::derived`. Without the exemption every `derive` in the language stops
/// checking at once, so this is worth naming rather than leaving to be inferred
/// from the round-trip tests below happening to still pass.
#[test]
fn a_derived_definition_is_exempt_from_the_written_signature_rule() {
    let dir = one("import std.json\n\
                   pub type Box = {label: String, n: Int}\n\
                   derive json for Box\n\
                   derive eq for Box\n\
                   derive ord for Box\n");
    let out = ply(dir.path()).arg("check").output().expect("a run");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        !text.contains("E0126"),
        "a derived definition must not be asked for an annotation: {text}"
    );
    assert!(out.status.success(), "the project should check: {text}");
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the `ply` binary");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

struct Run {
    code: i32,
    text: String,
}

impl Run {
    fn of(dir: &Path, args: &[&str]) -> Run {
        let out = ply(dir).args(args).output().expect("`ply` runs");
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Run {
            code: out.status.code().unwrap_or(-1),
            text,
        }
    }
}

/// A project whose only `test` items are the assertions being made, run green.
fn passes(source: &str) {
    let dir = one(source);
    let run = Run::of(dir.path(), &["test"]);
    assert_eq!(run.code, 0, "{}", run.text);
    assert!(
        !run.text.contains("0 passed"),
        "the fixture declared no tests:\n{}",
        run.text
    );
}

fn obligations(dir: &Path) -> (i32, Value) {
    let out = ply(dir).args(["prove", "--json"]).output().expect("`ply`");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one JSON object: {e}\n---\n{text}\n---"));
    (out.status.code().unwrap_or(-1), v)
}

/// Every law in the project is sampled and holds. `prove` rather than `test`
/// because the generator is the only thing that visits a shape nobody thought
/// to write down, which is where a codec's corner cases live.
fn laws_hold(dir: &Path) {
    let (code, v) = obligations(dir);
    let list = v["obligations"].as_array().expect("an obligations array");
    assert!(!list.is_empty(), "the fixture stated no law: {v}");
    for o in list {
        assert!(
            o["gap"].is_null(),
            "a law the generator could not sample is not evidence: {o}"
        );
        assert_eq!(
            o["outcome"], "property",
            "sampled over the whole domain and holding is the strongest a codec law reaches: {o}"
        );
    }
    assert_eq!(code, 0, "{v}");
}

// ------------------------------------------------------------- the wide corpus

/// Nested records, an ADT with four variants of differing arity, a recursive
/// type, lists of ADTs, maps keyed by three different ordered types, `Option`,
/// `Result`, and every leaf the deriver admits. One type, so one law reaches all
/// of it, and the generator draws the empty collections on its own.
const CORPUS: &str = r#"import std.json

pub type Colour = Red | Green | Blue
pub type Shape =
  | Blank
  | Circle(Decimal)
  | Rect(Decimal, Decimal)
  | Poly(List<Decimal>)

pub type Tree = Leaf | Node(Tree, Tree)

pub type Leafy = {
  name: String,
  raw: Bytes,
  weight: Decimal,
  count: Int,
  flag: Bool,
  nothing: Unit,
  tags: List<String>,
  maybe: Option<Int>,
}

pub type Deep = {
  by_name: Map<String, Shape>,
  by_num: Map<Int, List<Colour>>,
  by_bytes: Map<Bytes, Tree>,
  leaf: Leafy,
  inner: List<Leafy>,
  results: List<Result<Int, String>>,
  tree: Tree,
}

derive json for Colour
derive json for Shape
derive json for Tree
derive json for Leafy
derive json for Deep

pub fn round(d: Deep) -> Bool =
  match json::decode_bytes(json::encode_bytes(d, deep_json()), deep_json()) {
    Ok(e) -> e == d,
    Err(x) -> false,
  }

// The other direction, and the one that catches a non-canonical print: a
// document re-printed from what it parsed to must be the same bytes.
pub fn reprint(j: json::Json) -> Bool =
  match json::parse(json::to_bytes(j)) {
    Ok(k) -> json::to_bytes(k) == json::to_bytes(j),
    Err(x) -> false,
  }

law "a derived codec round-trips through the wire"
  forall (d: Deep) { round(d) }

law "printing a parsed document reproduces it"
  forall (j: json::Json) { reprint(j) }
"#;

/// The headline claim, over generated values rather than over the handful of
/// shapes a human writes down.
#[test]
fn every_shape_the_deriver_accepts_round_trips_over_generated_values() {
    let dir = one(CORPUS);
    laws_hold(dir.path());
}

/// `Float` is left out of `CORPUS` for a reason worth stating: a record with a
/// `Float` field has no *unguarded* round-trip law the generator can discharge.
/// The `Float` generator draws `NaN` on purpose — a generator that never did
/// would make `property` a lie about the type — and JSON has no non-finite
/// literal, so encoding one raises and the obligation comes back `unattempted`
/// with a gap rather than held or refuted.
///
/// The codec's domain is `decimal_of_float(f) != None`, and a law that says so
/// **is** discharged, over the same generator: that is the finite-only mode a
/// law can ask for, and it is what stops a `Float` field costing a type the
/// strongest evidence M8 offers. Both halves are pinned, because the unguarded
/// verdict is only honest if the guarded one is reachable.
#[test]
fn a_float_field_leaves_a_round_trip_law_unattemptable() {
    let fixture = |law: &str| {
        format!(
            r#"import std.json

pub type Reading = {{ ratio: Float }}
derive json for Reading

pub fn encodable(f: Float) -> Bool =
  match decimal_of_float(f) {{ Some(d) -> true, None -> false }}

pub fn round(r: Reading) -> Bool =
  match json::decode_bytes(json::encode_bytes(r, reading_json()), reading_json()) {{
    Ok(s) -> s == r,
    Err(e) -> false,
  }}

{law}
"#
        )
    };

    let dir = one(&fixture(
        "law \"a derived codec round-trips\" forall (r: Reading) { round(r) }",
    ));
    let (_, v) = obligations(dir.path());
    let o = &v["obligations"].as_array().expect("an obligations array")[0];
    assert_eq!(o["outcome"], "unattempted", "{o}");
    assert!(
        o["gap"]
            .as_str()
            .is_some_and(|g| g.contains("a Float that is NaN has no JSON encoding")),
        "the gap must name the encoding that raised, not merely report one:\n{o}"
    );

    let dir = one(&fixture(
        "law \"a derived codec round-trips over the values it encodes\"\n\
         forall (r: Reading) where encodable(r.ratio) { round(r) }",
    ));
    laws_hold(dir.path());
}

// --------------------------------------------------- nominally distinct types

/// Derivation is **structural**, so two nominally distinct types with the same
/// shape have the same wire format and decode each other's documents. That is
/// not a defect — a JSON object carries no type name — but it is the property
/// somebody will assume the other way round, so it is pinned rather than left to
/// be discovered by a client that sent the wrong body to the right endpoint.
#[test]
fn structurally_identical_types_share_a_wire_format_and_cross_decode() {
    let dir = project(&[
        (
            "n.ply",
            "import std.json\npub type D = LeftC(Int) | RightC(String)\nderive json for D\n",
        ),
        (
            "m.ply",
            r#"import std.json
import n

pub type A = { x: Int, y: String }
pub type B = { y: String, x: Int }
derive json for A
derive json for B

pub type C = LeftC(Int) | RightC(String)
derive json for C

fn a_wire(a: A) -> String = string_of_bytes(json::encode_bytes(a, a_json()))

test "two record types of one shape write one document" {
  assert_eq(a_wire({x: 1, y: "hi"}), "{\"x\":1,\"y\":\"hi\"}");
  assert_eq(string_of_bytes(json::encode_bytes({x: 1, y: "hi"}, b_json())), a_wire({x: 1, y: "hi"}))
}

test "and each decodes the other's document" {
  match json::decode_bytes(bytes_of_string(a_wire({x: 1, y: "hi"})), b_json()) {
    Ok(b) -> assert_eq(b.x, 1),
    Err(e) -> assert_eq(json::error_to_string(e), "it should have decoded"),
  }
}

test "an ADT is distinguished only by its variant names" {
  let doc = string_of_bytes(json::encode_bytes(LeftC(3), c_json()));
  assert_eq(doc, "{\"tag\":\"LeftC\",\"values\":[3]}");
  // `n::D` is declared in another module with the same variant names, and its
  // codec reads this document. A tag is a string, and nothing carries a module.
  match json::decode_bytes(bytes_of_string(doc), n::d_json()) {
    Ok(d) -> assert(true),
    Err(e) -> assert_eq(json::error_to_string(e), "it should have decoded"),
  }
}
"#,
        ),
    ]);
    let run = Run::of(dir.path(), &["test"]);
    assert_eq!(run.code, 0, "{}", run.text);
}

// --------------------------------------------------------- the `null` overlap

/// `option_json` writes `None` as `null` and `Some(x)` as `x`, so any inner type
/// whose encoding **can be `null`** collapses the two. `Unit` encodes as `null`
/// and `Option` itself encodes `None` as `null`, so `Option<Unit>` and
/// `Option<Option<a>>` are both lossy — and the deriver accepts both.
///
/// The assertion is deliberately fix-agnostic: either the derivation is refused
/// (`E0206`, naming the field, which is what `std.json`'s own comment says
/// should happen) or the codec round-trips. What may not happen is the third
/// thing, which is what happens today: it derives, it type-checks, it runs, and
/// `Some(None)` comes back as `None` with no error anywhere.
#[test]
fn an_encoding_that_can_be_null_is_never_wrapped_in_an_option() {
    for (label, field) in [
        ("Option<Unit>", "u: Option<Unit>"),
        ("Option<Option<Int>>", "oo: Option<Option<Int>>"),
    ] {
        let source = format!(
            r#"import std.json

pub type Wrap = {{ {field} }}
derive json for Wrap

pub fn round(w: Wrap) -> Bool =
  match (wrap_json().decode)((wrap_json().encode)(w)) {{
    Ok(v) -> v == w,
    Err(e) -> false,
  }}

law "a derived codec round-trips" forall (w: Wrap) {{ round(w) }}
"#
        );
        let dir = one(&source);
        let check = Run::of(dir.path(), &["check"]);
        if check.code != 0 {
            assert!(
                check.text.contains("E0206"),
                "refusing `{label}` is a fine answer, but it has to be E0206:\n{}",
                check.text
            );
            continue;
        }
        let (_, v) = obligations(dir.path());
        let o = &v["obligations"].as_array().expect("an obligations array")[0];
        assert_ne!(
            o["outcome"], "refuted",
            "`{label}` derives a codec that loses data: the derivation is accepted and the \
             round-trip law is refuted. Either refuse it at the `derive` with E0206 naming the \
             field, or give the encoding a tag that separates `None` from a `null` payload.\n{o}"
        );
    }
}

/// The same defect where it is worst: as a `Map` key. Two distinct keys encode
/// to one, so the map loses an entry rather than a field, and `map_len` changes
/// across the wire.
#[test]
fn two_map_keys_that_encode_alike_do_not_survive_the_wire() {
    let dir = one(r#"import std.json

pub type Holder = { m: Map<Option<Unit>, Int> }
derive json for Holder

pub fn round(h: Holder) -> Bool =
  match (holder_json().decode)((holder_json().encode)(h)) {
    Ok(v) -> map_len(v.m) == map_len(h.m),
    Err(e) -> false,
  }

law "a map survives its own codec" forall (h: Holder) { round(h) }
"#);
    let check = Run::of(dir.path(), &["check"]);
    if check.code != 0 {
        assert!(check.text.contains("E0206"), "{}", check.text);
        return;
    }
    let (_, v) = obligations(dir.path());
    let o = &v["obligations"].as_array().expect("an obligations array")[0];
    assert_ne!(
        o["outcome"], "refuted",
        "`Map<Option<Unit>, Int>` encodes `None` and `Some(())` to the same key, so a two-entry \
         map decodes to one.\n{o}"
    );
}

// -------------------------------------------------------------- recursive types

/// A recursive type's codec terminates, which is required test 19 — and the
/// depth it writes is the depth its own parser accepts. An ADT level costs two
/// JSON levels (an object wrapping an array), so a derived codec reaches the
/// bound at about half of `max_depth`, and it reaches it **at the encoder**.
///
/// Encoding total where decoding is partial would be a codec that writes a
/// document it cannot read: a service persists a payload and can never load it
/// back, and the failure appears at the consumer rather than at the producer.
/// The two halves now refuse the same values. The bound is pinned here so that
/// moving it is a decision rather than an accident.
#[test]
fn a_recursive_codec_refuses_to_write_what_it_cannot_read() {
    let source = r#"import std.json

pub type Tree = Leaf | Node(Tree, Tree)
derive json for Tree

fn deep(n: Int) -> Tree = if n <= 0 { Leaf } else { Node(deep(n - 1), Leaf) }
fn depth(t: Tree) -> Int = match t { Leaf -> 0, Node(l, _) -> 1 + depth(l) }

fn through_a_value(n: Int) -> String =
  match (tree_json().decode)((tree_json().encode)(deep(n))) {
    Ok(t) -> int_to_string(depth(t)),
    Err(e) -> "ERR " ++ json::error_to_string(e),
  }

fn through_the_wire(n: Int) -> String =
  match json::decode_bytes(json::encode_bytes(deep(n), tree_json()), tree_json()) {
    Ok(t) -> int_to_string(depth(t)),
    Err(e) -> "ERR " ++ json::error_to_string(e),
  }

test "a codec over a value terminates at depth 100" {
  assert_eq(through_a_value(100), "100")
}

test "and the wire round-trips up to half of max_depth" {
  assert_eq(through_the_wire(60), "60");
  assert_eq(through_the_wire(64), "64")
}
"#;
    passes(source);

    // Past the bound the *encoder* refuses, so nothing is written that `parse`
    // would later reject.
    let dir = one(&format!(
        "{source}\ntest \"past it, the encoder is the one that refuses\" \
         {{ assert_eq(through_the_wire(100), \"unreachable\") }}\n"
    ));
    let run = Run::of(dir.path(), &["test"]);
    assert_eq!(run.code, 1, "{}", run.text);
    assert!(
        run.text.contains(
            "a JSON value nested deeper than 128 levels has no encoding `parse` would read back"
        ),
        "the refusal must name the bound, at the producer:\n{}",
        run.text
    );
}

// ----------------------------------------------------------- schema evolution

/// A type that gained a field. The deriver emits `field` for every field,
/// including an `Option` one, so an added optional field is **required on the
/// wire** — `std.json` has `optional_field`, which treats absent and `null`
/// alike, and no generated body ever calls it.
///
/// Pinned rather than asserted to be right: it means adding an `Option<T>` field
/// to a response type is a breaking change for every client that does not send
/// the key, which is the opposite of what `Option` reads as.
#[test]
fn an_added_optional_field_is_still_required_on_the_wire() {
    passes(
        r#"import std.json

pub type V2 = { id: Int, note: Option<String> }
derive json for V2

fn decode(doc: String) -> String =
  match json::decode_bytes(bytes_of_string(doc), v2_json()) {
    Ok(v) -> "ok " ++ string_of_bytes(json::encode_bytes(v, v2_json())),
    Err(e) -> "ERR " ++ json::error_to_string(e),
  }

test "an old document that omits the new optional field is rejected" {
  assert_eq(decode("{\"id\":1}"), "ERR $.note: this field is missing")
}

test "an explicit null is the absent case" {
  assert_eq(decode("{\"id\":1,\"note\":null}"), "ok {\"id\":1,\"note\":null}")
}

test "an unknown field is ignored, so adding one is not breaking" {
  assert_eq(decode("{\"id\":1,\"note\":null,\"added_later\":5}"), "ok {\"id\":1,\"note\":null}")
}

test "a duplicate key resolves to the last, deterministically" {
  assert_eq(decode("{\"id\":1,\"id\":2,\"note\":null}"), "ok {\"id\":2,\"note\":null}")
}
"#,
    );
}

// ------------------------------------------------------------- numeric edges

/// `Int` at both ends of `i64`, `Decimal` at its maximum mantissa and its
/// maximum scale, and — the one a value comparison cannot see — the scale
/// itself, which `==` ignores and `decimal_to_string` does not.
#[test]
fn integers_and_decimals_survive_their_own_edges_scale_included() {
    passes(
        r#"import std.json

pub type Edge = { i: Int, d: Decimal }
derive json for Edge

fn round(i: Int, d: Decimal) -> String =
  match json::decode_bytes(json::encode_bytes({i: i, d: d}, edge_json()), edge_json()) {
    Ok(e) -> int_to_string(e.i) ++ " " ++ decimal_to_string(e.d),
    Err(x) -> "ERR " ++ json::error_to_string(x),
  }

test "i64 at both ends" {
  assert_eq(round(9223372036854775807, 0m), "9223372036854775807 0");
  assert_eq(round(-9223372036854775807 - 1, 0m), "-9223372036854775808 0")
}

test "a decimal at its bounds, and its scale with it" {
  assert_eq(round(0, 79228162514264337593543950335m), "0 79228162514264337593543950335");
  assert_eq(round(0, 0.0000000000000000000000000001m),
            "0 0.0000000000000000000000000001");
  // `1.500m == 1.5m`, so only the rendering catches a lost scale.
  assert_eq(round(0, 1.500m), "0 1.500")
}

test "a number the wire holds but a Decimal cannot names the byte offset" {
  match json::decode_bytes(bytes_of_string("{\"i\":1e100,\"d\":0}"), edge_json()) {
    Ok(e) -> assert(false),
    Err(x) -> assert_eq(json::error_to_string(x),
      "$: `1e100` is outside `Decimal`'s range or needs more than 28 significant digits (at byte 5)"),
  }
}

test "a fractional value is not silently truncated into an Int field" {
  match json::decode_bytes(bytes_of_string("{\"i\":1.5,\"d\":0}"), edge_json()) {
    Ok(e) -> assert(false),
    Err(x) -> assert_eq(json::error_to_string(x), "$.i: expected a whole number, found `1.5`"),
  }
}
"#,
    );
}

/// `Float`'s JSON encoding is **partial on finite values**, not only on the
/// non-finite ones ADR 0012 names: a number needing more than 28 significant
/// digits of scale, or larger than `Decimal`'s maximum, has no `Number` to
/// become. `1.0e-30` is an ordinary finite `f64` and it raises.
///
/// It raises rather than corrupting, which is the right shape of failure — but
/// it is a `RUNTIME_ERROR` from inside a definition the user did not write, and
/// pinning it is what keeps the reason in the message.
#[test]
fn a_float_field_encodes_partially_and_says_which_value_broke_it() {
    let dir = one(r#"import std.json

pub type F = { f: Float }
derive json for F

pub fn wire(f: Float) -> String = string_of_bytes(json::encode_bytes({f: f}, f_json()))

pub fn round(f: Float) -> Bool =
  match json::decode_bytes(json::encode_bytes({f: f}, f_json()), f_json()) {
    Ok(g) -> g.f == f,
    Err(e) -> false,
  }

test "an ordinary float round-trips" {
  assert(round(0.1));
  assert(round(3.141592653589793));
  // `-0.0` decodes as `+0.0`. The language's `==` cannot see it, `Value::cmp`
  // can, and the two are two definitions in a hash — so it is written down.
  assert(round(-0.0));
  assert_eq(wire(-0.0), "{\"f\":0}")
}

test "a finite float too small for a Decimal raises" { assert_eq(wire(1.0e-30), "unreachable") }
test "a finite float too large for a Decimal raises" { assert_eq(wire(1.0e300), "unreachable") }
test "NaN raises and names itself" { assert_eq(wire(0.0 / 0.0), "unreachable") }
test "an infinity raises and names itself" { assert_eq(wire(1.0 / 0.0), "unreachable") }
"#);
    let run = Run::of(dir.path(), &["test"]);
    assert_eq!(
        run.code, 1,
        "the four raising cases must fail:\n{}",
        run.text
    );
    assert!(
        run.text.contains("1 passed"),
        "the round-tripping case must pass:\n{}",
        run.text
    );
    for expected in [
        "a Float that is outside `Decimal`'s range has no JSON encoding",
        "a Float that is NaN has no JSON encoding",
        "a Float that is Infinity has no JSON encoding",
    ] {
        assert!(run.text.contains(expected), "{expected}\n{}", run.text);
    }
}

// ------------------------------------------------------- text and byte strings

/// `\u{0001}` is written as `CTL` and substituted in, because a literal control
/// byte in a Rust source file is a thing an editor silently rewrites. Everything
/// else — the astral-plane character included — is the bytes a client sends.
const TEXT: &str = r#"import std.json

pub type T = { s: String, b: Bytes }
derive json for T

fn wire(s: String, b: Bytes) -> String = string_of_bytes(json::encode_bytes({s: s, b: b}, t_json()))

fn round(s: String, b: Bytes) -> Bool =
  match json::decode_bytes(json::encode_bytes({s: s, b: b}, t_json()), t_json()) {
    Ok(t) -> t.s == s && t.b == b,
    Err(e) -> false,
  }

test "the empty string and the empty byte string" {
  assert(round("", b""));
  assert_eq(wire("", b""), "{\"b\":\"\",\"s\":\"\"}")
}

test "unicode passes through unescaped and round-trips" {
  assert(round("éERROR", b""));
  assert_eq(wire("éERROR", b""), "{\"b\":\"\",\"s\":\"éERROR\"}")
}

test "controls, quotes and backslashes are escaped and read back" {
  assert(round("aCTLb\n\t\"q\\", b""));
  assert_eq(wire("aCTLb\n\t\"q\\", b""), "{\"b\":\"\",\"s\":\"a\\u0001b\\n\\t\\\"q\\\\\"}")
}

test "every byte value survives base64" {
  assert(round("", b"\x00\x01\x7f\xfe\xff"));
  assert_eq(wire("", b"\x00\xff\x7f"), "{\"b\":\"AP9/\",\"s\":\"\"}")
}
"#;

#[test]
fn text_and_bytes_survive_escaping_in_both_directions() {
    let source = TEXT
        .replace("CTL", "\u{0001}")
        .replace("ERROR", "\u{1F600}");
    let dir = one(&source);
    let run = Run::of(dir.path(), &["test"]);
    assert_eq!(run.code, 0, "{}", run.text);
}

// ------------------------------------------------ the wire depends on spelling

/// `Map<String, v>` gets a JSON object and every other `Map` gets an array of
/// pairs, and the deriver decides that from the key's **type** — following this
/// module's own aliases — rather than from how the key was spelled.
///
/// An alias is transparent to the checker, so `type Key = String` makes
/// `Map<Key, Int>` and `Map<String, Int>` the *same type*: `direct_json()` and
/// `aliased_json()` substitute for each other at every call site, and two wire
/// formats would be two codecs that disagree about the protocol with nothing to
/// read at the `derive` line. That is exactly the coherence hazard ADR 0010 has
/// no resolution layer to prevent, and this is what closes it inside a module.
#[test]
fn a_map_key_written_through_an_alias_gets_the_same_wire_format() {
    passes(
        r#"import std.json

pub type Key = String
derive json for Key

pub type Direct = { m: Map<String, Int> }
pub type Aliased = { m: Map<Key, Int> }
derive json for Direct
derive json for Aliased

pub type ById = { m: Map<Int, Int> }
derive json for ById

fn entry() -> Map<String, Int> = map_insert(map_new(), "k", 1)

test "one type, one protocol, whichever way the key was spelled" {
  assert_eq(string_of_bytes(json::encode_bytes({m: entry()}, direct_json())), "{\"m\":{\"k\":1}}");
  assert_eq(string_of_bytes(json::encode_bytes({m: entry()}, aliased_json())), "{\"m\":{\"k\":1}}")
}

test "so each reads the other's document" {
  match json::decode_bytes(bytes_of_string("{\"m\":{\"k\":1}}"), aliased_json()) {
    Ok(a) -> assert_eq(map_len(a.m), 1),
    Err(e) -> assert_eq(json::error_to_string(e), "it should have decoded"),
  }
}

// The other branch is still there: a key that is not a `String` has no object
// form, because a JSON object's keys are strings.
test "a key that is not a string is an array of pairs" {
  assert_eq(string_of_bytes(json::encode_bytes({m: map_insert(map_new(), 1, 2)}, by_id_json())),
            "{\"m\":[{\"key\":1,\"value\":2}]}")
}
"#,
    );
}

// -------------------------------------------------------------- refusals hold

/// `Float` may not reach a `Map` key by any route: not directly, not through a
/// nominal ADT in another module, not through a recursive one, not through a
/// generic instantiation, not through an alias, not inside an anonymous record,
/// not inside an `Option`, and not by instantiating a constrained type parameter
/// at a call site. A `Float` key is a `NaN` key, and a `NaN` key is a lookup
/// that cannot find what it just inserted — the one way `Map`'s iteration order
/// could stop being a function of its contents.
#[test]
fn no_route_lets_a_float_become_a_map_key() {
    let dir = project(&[
        (
            "n.ply",
            "pub type Wrapper = W(Float)\n\
             pub type Recursive = Nil | Cons(Float, Recursive)\n\
             pub type Box<a> = { v: a }\n\
             pub type Alias = Float\n",
        ),
        (
            "m.ply",
            "import n\n\
             fn a() -> Map<n::Wrapper, Int> = map_new()\n\
             fn b() -> Map<n::Recursive, Int> = map_new()\n\
             fn c() -> Map<n::Box<Float>, Int> = map_new()\n\
             fn d() -> Map<n::Alias, Int> = map_new()\n\
             fn e() -> Map<{x: Float}, Int> = map_new()\n\
             fn f() -> Map<Option<Float>, Int> = map_new()\n\
             fn g() -> Map<Map<n::Wrapper, Int>, Int> = map_new()\n\
             fn keyed<k>(m: Map<k, Int>) -> Int where derivable(ord, k) = map_len(m)\n\
             fn h() -> Int = keyed(map_insert(map_new(), 1.0, 1))\n",
        ),
    ]);
    let run = Run::of(dir.path(), &["check"]);
    assert_ne!(run.code, 0, "{}", run.text);
    // One diagnostic per route, each naming the key type it walked into rather
    // than only the `Float` at the bottom of it.
    for named in [
        "`n.Wrapper`",
        "`n.Recursive`",
        "`{v: Float}`",
        "`{x: Float}`",
        "`Option<Float>`",
        "`Map<n.Wrapper, Int>`",
    ] {
        assert!(
            run.text.contains(named),
            "no refusal named {named}:\n{}",
            run.text
        );
    }
    assert!(
        run.text.contains("`Float` cannot be derived for `ord`"),
        "instantiating a `derivable(ord, k)` parameter at `Float` must be refused at the call \
         site:\n{}",
        run.text
    );
    assert!(
        run.text.matches("E0206").count() >= 9,
        "one refusal per route:\n{}",
        run.text
    );
}

/// The other refusals the wire depends on, in one place: a function field names
/// the field rather than producing a partial encoder, a `derive` for another
/// module's type is an orphan, and a second `derive` of one deriver for one type
/// is a duplicate.
#[test]
fn the_refusals_that_keep_a_codec_total_all_fire() {
    let cases: [(&str, &str); 3] = [
        (
            "E0206",
            "import std.json\n\
             pub type Order = { id: Int, on_complete: (Int) -> Unit }\n\
             derive json for Order\n",
        ),
        (
            "E0208",
            "import std.json\nimport n\nderive json for Thing\n",
        ),
        (
            "E0105",
            "import std.json\n\
             pub type Order = { id: Int }\n\
             derive json for Order\nderive json for Order\n",
        ),
    ];
    for (code, source) in cases {
        let dir = project(&[
            ("m.ply", source),
            ("n.ply", "pub type Thing = { id: Int }\n"),
        ]);
        let run = Run::of(dir.path(), &["check"]);
        assert_ne!(run.code, 0, "{source}\n{}", run.text);
        assert!(run.text.contains(code), "{source}\n{}", run.text);
    }
}
