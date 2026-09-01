//! Derivation end to end: expansion, then resolution, then inference.
//!
//! The `std.json` module below is a **stub of the protocol a generated codec
//! calls**, not a copy of the real one: every body is `panic`, and the signatures
//! are the whole content. If the shipped `std.json` ever stops matching it,
//! every derivation in the language stops checking — so this file is the
//! contract between the deriver and the codec, written where a compiler failure
//! reports it.

use ply_core::{CheckOutput, check_program};
use ply_span::{Diagnostic, SourceId, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

/// The signatures a generated `json` dictionary references, and nothing else.
/// Leaf codecs are `<snake(Type)>_json()`, which is the same rule the deriver
/// names a user type's codec by.
const JSON: &str = r#"
pub type Json = Null | Bool(Bool) | Number(Decimal) | Str(String)
              | Array(List<Json>) | Object(Map<String, Json>)
pub type DecodeError = {path: String, message: String}
pub type JsonCodec<a> = {encode: (a) -> Json, decode: (Json) -> Result<a, DecodeError>}

pub fn int_json() -> JsonCodec<Int> = panic("stub")
pub fn bool_json() -> JsonCodec<Bool> = panic("stub")
pub fn string_json() -> JsonCodec<String> = panic("stub")
pub fn bytes_json() -> JsonCodec<Bytes> = panic("stub")
pub fn float_json() -> JsonCodec<Float> = panic("stub")
pub fn decimal_json() -> JsonCodec<Decimal> = panic("stub")
pub fn unit_json() -> JsonCodec<Unit> = panic("stub")

pub fn list_json<a>(a: JsonCodec<a>) -> JsonCodec<List<a>>
  where derivable(json, a) = panic("stub")
pub fn option_json<a>(a: JsonCodec<a>) -> JsonCodec<Option<a>>
  where derivable(json, a) = panic("stub")
pub fn result_json<a, e>(a: JsonCodec<a>, e: JsonCodec<e>) -> JsonCodec<Result<a, e>>
  where derivable(json, a), derivable(json, e) = panic("stub")
pub fn map_json<k, v>(k: JsonCodec<k>, v: JsonCodec<v>) -> JsonCodec<Map<k, v>>
  where derivable(ord, k), derivable(json, k), derivable(json, v) = panic("stub")

pub fn string_map_json<v>(value: JsonCodec<v>) -> JsonCodec<Map<String, v>> = panic("stub")

pub type Member = {key: String, value: Json}
pub type Tagged = {tag: String, values: List<Json>}

pub fn object(fields: List<Member>) -> Json = panic("stub")
pub fn field<a>(j: Json, name: String, codec: JsonCodec<a>) -> Result<a, DecodeError>
  where derivable(json, a) = panic("stub")
pub fn variant(tag: String, values: List<Json>) -> Json = panic("stub")
pub fn variant_of(j: Json) -> Result<Tagged, DecodeError> = panic("stub")
pub fn variant_value(v: Tagged, index: Int) -> Result<Json, DecodeError> = panic("stub")
pub fn unknown_variant<a>(tag: String, expected: List<String>) -> Result<a, DecodeError> =
  panic("stub")
pub fn decode_and_then<a, b>(r: Result<a, DecodeError>, f: (a) -> Result<b, DecodeError>)
  -> Result<b, DecodeError> = panic("stub")
"#;

fn compile(modules: &[(&str, &str)]) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs: Vec<_> = modules
        .iter()
        .enumerate()
        .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
        .collect();
    let mut program = ply_syntax::parse_program(inputs)?;
    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        return Err(diags);
    }
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

/// One project module, with the protocol stub beside it.
fn with_json(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    compile(&[("std.json", JSON), ("m", source)])
}

fn ok(source: &str) -> CheckOutput {
    match with_json(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match with_json(source) {
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

fn has(d: &Diagnostic, text: &str) -> bool {
    d.message.contains(text)
        || d.notes.iter().any(|n| n.contains(text))
        || d.labels.iter().any(|l| l.message.contains(text))
}

// ---------------------------------------------------------------- json

#[test]
fn a_record_codec_checks() {
    let out = ok("import std.json
type Order = {id: Int, name: String, total: Decimal}
derive json for Order");
    assert!(
        out.defs
            .contains_key(&ply_span::Symbol::new("m.order_json"))
    );
}

#[test]
fn an_adt_codec_checks_including_a_nullary_variant() {
    ok("import std.json
type Shape = Circle(Int) | Rect(Int, Int) | Point
derive json for Shape");
}

#[test]
fn lists_maps_options_and_results_check() {
    ok(r#"import std.json
type Bag = {
  items: List<Int>,
  index: Map<String, Bool>,
  maybe: Option<Bytes>,
  outcome: Result<Int, String>,
  nested: List<Map<Int, List<Float>>>
}
derive json for Bag"#);
}

#[test]
fn a_parameterized_type_takes_one_dictionary_per_parameter() {
    let out = ok("import std.json
type Pair<x, y> = {left: x, right: y}
derive json for Pair");
    let def = &out.defs[&ply_span::Symbol::new("m.pair_json")];
    assert_eq!(def.constraints.len(), 2, "{:?}", def.constraints);
    assert_eq!(def.scheme.ty_vars.len(), 2);
}

#[test]
fn a_recursive_type_derives_and_composes_through_itself() {
    ok("import std.json
type Tree = Leaf(Int) | Node(List<Tree>)
derive json for Tree");
}

#[test]
fn derivation_composes_through_another_module_by_name() {
    compile(&[
        ("std.json", JSON),
        (
            "users",
            "import std.json\npub type User = {name: String}\nderive json for User",
        ),
        (
            "m",
            "import std.json\nimport users\n\
             type Order = {who: users::User}\nderive json for Order",
        ),
    ])
    .expect("a field of another module's type calls that module's codec");
}

#[test]
fn a_field_whose_type_has_no_derivation_is_reported_against_the_derive() {
    let diags = compile(&[
        ("std.json", JSON),
        ("users", "pub type User = {name: String}"),
        (
            "m",
            "import std.json\nimport users\n\
             type Order = {who: users::User}\nderive json for Order",
        ),
    ])
    .expect_err("`users` never derived a codec");
    let d = diags
        .iter()
        .find(|d| d.code == codes::NOT_DERIVABLE)
        .unwrap_or_else(|| panic!("expected E0206, got {diags:#?}"));
    assert!(has(d, "user_json"), "{d:#?}");
    assert!(has(d, "derive json for"), "{d:#?}");
}

#[test]
fn a_module_that_does_not_import_std_json_is_told_to() {
    let d = only_code(
        "type Order = {id: Int}\nderive json for Order",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "import std.json"), "{d:#?}");
}

// ---------------------------------------------------------------- eq and ord

#[test]
fn eq_and_ord_need_no_import() {
    let out = compile(&[(
        "m",
        "type Order = {id: Int, name: String}\nderive eq for Order\nderive ord for Order",
    )])
    .expect("`eq` and `ord` are language-level");
    assert!(out.defs.contains_key(&ply_span::Symbol::new("m.order_eq")));
    assert!(out.defs.contains_key(&ply_span::Symbol::new("m.order_ord")));
}

#[test]
fn ord_refuses_a_float_field_and_json_accepts_one() {
    ok("import std.json\ntype P = {x: Float}\nderive json for P\nderive eq for P");
    let diags = compile(&[("m", "type P = {x: Float}\nderive ord for P")])
        .expect_err("`Float` has no total order");
    let d = &diags[0];
    assert_eq!(d.code, codes::NOT_DERIVABLE);
    assert!(has(d, "Float"), "{d:#?}");
}

#[test]
fn a_derived_dictionary_is_callable_at_its_declared_shape() {
    ok("import std.json
type Order = {id: Int}
derive eq for Order
derive ord for Order
fn same(a: Order, b: Order) -> Bool = order_eq().eq(a, b)
fn rank(a: Order, b: Order) -> Ordering = order_ord().compare(a, b)");
}

// ---------------------------------------------------------------- refusals

#[test]
fn a_function_field_names_the_field() {
    let d = only_code(
        "import std.json
type Order = {id: Int, on_complete: (Int) -> Unit}
derive json for Order",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "function"), "{d:#?}");
    assert!(has(&d, "every field to be derivable"), "{d:#?}");
}

#[test]
fn a_function_inside_a_variant_names_the_variant() {
    let d = only_code(
        "import std.json
type Handler = Sync(Int) | Async((Int) -> Int)
derive json for Handler",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "Async"), "{d:#?}");
}

#[test]
fn a_task_field_is_refused() {
    let d = only_code(
        "import std.json\ntype Job = {t: Task<Int>}\nderive json for Job",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "Task"), "{d:#?}");
}

#[test]
fn a_map_keyed_by_float_is_refused_at_the_derive() {
    let d = only_code(
        "import std.json\ntype Prices = {by: Map<Float, Int>}\nderive json for Prices",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "ordered"), "{d:#?}");
}

#[test]
fn a_derive_for_another_modules_type_is_an_orphan() {
    let diags = compile(&[
        ("std.json", JSON),
        ("users", "pub type User = {name: String}"),
        ("m", "import std.json\nimport users\nderive json for User"),
    ])
    .expect_err("a `derive` may only name a type its own module declares");
    assert_eq!(diags[0].code, codes::ORPHAN_DERIVE);
}

#[test]
fn two_derivations_generating_one_name_are_a_duplicate() {
    let diags = compile(&[(
        "m",
        "type Order = {id: Int}\nderive eq for Order\nderive eq for Order",
    )])
    .expect_err("one name, generated twice");
    assert_eq!(diags[0].code, codes::DUPLICATE_DEFINITION);

    let diags = compile(&[(
        "m",
        "type HTTPRequest = {id: Int}\ntype HttpRequest = {id: Int}\n\
         derive eq for HTTPRequest\nderive eq for HttpRequest",
    )])
    .expect_err("snake_case is total, so it can collide");
    assert_eq!(diags[0].code, codes::DUPLICATE_DEFINITION);
    assert!(has(&diags[0], "rename one of the types"), "{:#?}", diags[0]);
}

// ---------------------------------------------------------------- constraints

#[test]
fn a_call_site_instantiating_a_constrained_parameter_is_checked_there() {
    let d = only_code(
        "import std.json
fn ship<a>(x: a, c: json::JsonCodec<a>) -> json::Json where derivable(json, a) = (c.encode)(x)
fn go(f: (Int) -> Int, c: json::JsonCodec<(Int) -> Int>) -> json::Json = ship(f, c)",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "ship"), "{d:#?}");
    assert!(
        d.labels.iter().filter(|l| !l.primary).count() >= 1,
        "the signature is a secondary label: {d:#?}"
    );
}

#[test]
fn a_body_may_assume_its_own_constraint() {
    ok("import std.json
fn ship<a>(x: a, c: json::JsonCodec<a>) -> json::Json where derivable(json, a) = (c.encode)(x)
fn twice<a>(x: a, c: json::JsonCodec<a>) -> json::Json where derivable(json, a) = ship(x, c)");
}

#[test]
fn a_body_that_does_not_declare_the_constraint_is_told_which_clause_to_add() {
    let d = only_code(
        "import std.json
fn ship<a>(x: a, c: json::JsonCodec<a>) -> json::Json where derivable(json, a) = (c.encode)(x)
fn twice<a>(x: a, c: json::JsonCodec<a>) -> json::Json = ship(x, c)",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "where derivable(json, a)"), "{d:#?}");
}

#[test]
fn a_constraint_naming_an_unbound_parameter_is_an_unknown_type() {
    let d = only_code(
        "fn f<a>(x: a) -> a where derivable(eq, b) = x",
        codes::UNKNOWN_TYPE,
    );
    assert!(has(&d, "`a`"), "{d:#?}");
}

#[test]
fn compare_refuses_a_float_at_the_call_site() {
    let d = only_code(
        "fn worst(a: Float, b: Float) -> Ordering = compare(a, b)",
        codes::NOT_DERIVABLE,
    );
    assert!(has(&d, "Float"), "{d:#?}");
}

#[test]
fn a_concrete_instantiation_that_is_derivable_is_accepted() {
    ok("import std.json
fn ship<a>(x: a, c: json::JsonCodec<a>) -> json::Json where derivable(json, a) = (c.encode)(x)
type Order = {id: Int}
derive json for Order
fn go(o: Order) -> json::Json = ship(o, order_json())");
}

/// The stub above is a claim about `std.json`. This is the claim being checked:
/// the same derivations, against the module that actually ships. When this fails
/// and the stub tests pass, `std.json` moved out from under the deriver.
#[test]
fn the_shipped_std_json_satisfies_the_protocol() {
    // Read from the source tree rather than from `ply_std::MODULES`, so that
    // this holds whether or not `std.json` has been added to the embedded table
    // yet: what is being checked is the module the deriver targets.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-std/ply/json.ply");
    let json = std::fs::read_to_string(&path).expect("`std.json` ships with the compiler");
    let mut modules: Vec<(&str, &str)> = ply_std::sources()
        .filter(|(name, _)| *name != "std.json")
        .collect();
    modules.push(("std.json", &json));
    modules.push((
        "m",
        "import std.json
type Line = {sku: String, qty: Int}
type Order = {id: Int, lines: List<Line>, labels: Map<String, String>, note: Option<String>}
type Status = Placed | Shipped(Int) | Cancelled(String, Bool)
derive json for Line
derive json for Order
derive json for Status
derive eq for Order
derive ord for Line",
    ));
    if let Err(d) = compile(&modules) {
        panic!("derivation against the shipped `std.json` failed: {d:#?}");
    }
}
