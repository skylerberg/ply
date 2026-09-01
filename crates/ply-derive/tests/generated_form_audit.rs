//! A second golden pin, over the shapes the first one does not reach.

use ply_derive::preview;
use ply_span::SourceId;
use ply_syntax::ast::ModuleName;
use ply_syntax::parser::parse_module;

const JSON: &str = "import std.json\n";

fn module(source: &str) -> ply_syntax::ast::Module {
    parse_module(SourceId(0), ModuleName::from_dotted("t"), source)
        .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}"))
}

fn generated(source: &str) -> Vec<String> {
    preview(&module(source))
}

fn one(source: &str) -> String {
    let out = generated(source);
    assert_eq!(out.len(), 1, "{out:#?}");
    out.into_iter().next().expect("length checked")
}

const MOVED: &str = "the deriver's output moved. If that is intended, update this pin AND bump \
                     FRONTEND_VERSION: gate 1 keys on raw file content, so without the bump a \
                     file whose generated definition changed would be skipped and the stale one \
                     reused.";

/// `Map<String, v>` is a JSON object and anything else is an array of pairs, and the deriver
/// decides that syntactically.
#[test]
fn the_two_map_encodings_are_pinned() {
    let g = one(&format!(
        "{JSON}type Doc = {{by_name: Map<String, Int>, by_num: Map<Int, List<String>>}}\n\
         derive json for Doc"
    ));
    let expected = r#"fn doc_json() -> json::JsonCodec<Doc> = {encode: |dv: Doc| json::object([{key: "by_name", value: (json::string_map_json(json::int_json()).encode)(dv.by_name)}, {key: "by_num", value: (json::map_json(json::int_json(), json::list_json(json::string_json())).encode)(dv.by_num)}]), decode: |dj: json::Json| match json::field(dj, "by_name", json::string_map_json(json::int_json())) {Err(de) -> Err(de), Ok(d0) -> match json::field(dj, "by_num", json::map_json(json::int_json(), json::list_json(json::string_json()))) {Err(de) -> Err(de), Ok(d1) -> Ok({by_name: d0, by_num: d1})}}}"#;
    assert_eq!(g, expected, "{MOVED}");
}

/// One dictionary parameter per type parameter, in declaration order, with the `where
/// derivable(json, ·)` clauses that make the body's use of them sound.
#[test]
fn a_parameterized_types_dictionary_form_is_pinned() {
    let g = one(&format!(
        "{JSON}type Pair<a, b> = {{fst: a, snd: b}}\nderive json for Pair"
    ));
    let expected = r#"fn pair_json<a, b>(a: json::JsonCodec<a>, b: json::JsonCodec<b>) -> json::JsonCodec<Pair<a, b>> where derivable(json, a), derivable(json, b) = {encode: |dv: Pair<a, b>| json::object([{key: "fst", value: (a.encode)(dv.fst)}, {key: "snd", value: (b.encode)(dv.snd)}]), decode: |dj: json::Json| match json::field(dj, "fst", a) {Err(de) -> Err(de), Ok(d0) -> match json::field(dj, "snd", b) {Err(de) -> Err(de), Ok(d1) -> Ok({fst: d0, snd: d1})}}}"#;
    assert_eq!(g, expected, "{MOVED}");
}

/// A type parameter called `d` collides with the prefix the emitter reaches for first, so it walks
/// to `d_` and every binder in the body moves with it.
#[test]
fn a_type_parameter_that_shadows_the_binder_prefix_is_pinned() {
    let g = one(&format!(
        "{JSON}type Pair<d, e> = {{fst: d, snd: e}}\nderive json for Pair"
    ));
    assert!(
        g.contains("|d_v: Pair<d, e>|") && g.contains("Ok(d_0)") && g.contains("Err(d_e)"),
        "the binder prefix must step past a type parameter it would shadow:\n{g}\n{MOVED}"
    );
    assert!(
        !g.contains("|dv:"),
        "a generated binder shadowed the dictionary parameter `d`:\n{g}"
    );
}

/// A recursive type composes through itself by name.
#[test]
fn a_recursive_types_self_reference_is_pinned() {
    let g = one(&format!(
        "{JSON}type Tree = Leaf | Node(Tree, Tree)\nderive json for Tree"
    ));
    let expected = r#"fn tree_json() -> json::JsonCodec<Tree> = {encode: |dv: Tree| match dv {Leaf -> json::variant("Leaf", []), Node(d0, d1) -> json::variant("Node", [(tree_json().encode)(d0), (tree_json().encode)(d1)])}, decode: |dj: json::Json| match json::variant_of(dj) {Err(de) -> Err(de), Ok(dt) -> if dt.tag == "Leaf" {Ok(Leaf)} else {if dt.tag == "Node" {match json::decode_and_then(json::variant_value(dt, 0), tree_json().decode) {Err(de) -> Err(de), Ok(d0) -> match json::decode_and_then(json::variant_value(dt, 1), tree_json().decode) {Err(de) -> Err(de), Ok(d1) -> Ok(Node(d0, d1))}}} else {json::unknown_variant(dt.tag, ["Leaf", "Node"])}}}}"#;
    assert_eq!(g, expected, "{MOVED}");
}

/// An alias is transparent to the checker, so a key spelled through one is the same type and must
/// reach the same codec.
#[test]
fn a_map_key_is_classified_by_its_type_and_not_by_its_spelling() {
    let g = one(&format!(
        "{JSON}type Key = Name\ntype Name = String\n\
         type Doc = {{by_key: Map<Key, Int>, by_other: Map<other::Key, Int>}}\n\
         derive json for Doc"
    ));
    assert!(g.contains("json::string_map_json(json::int_json())"), "{g}");
    assert!(
        g.contains("json::map_json(other::key_json(), json::int_json())"),
        "a key the deriver cannot resolve keeps the total form: {g}"
    );
}

/// `Option` and `Result` are structural, so they compose through the codecs `std.json` ships rather
/// than through a generated one — and an `Option` whose payload also writes `null` is **refused**
/// rather than emitted, because `option_json` would write `Some` and `None` as the same document.
#[test]
fn the_structural_codecs_are_composed_by_name() {
    let g = one(&format!(
        "{JSON}type Wrap = {{a: Option<Int>, b: Result<Int, String>}}\nderive json for Wrap"
    ));
    assert!(g.contains("json::option_json(json::int_json())"), "{g}");
    assert!(
        g.contains("json::result_json(json::int_json(), json::string_json())"),
        "{g}"
    );

    for field in ["c: Option<Option<Int>>", "c: Option<Unit>"] {
        let refused = ply_derive::expand_module(&mut module(&format!(
            "{JSON}type Wrap = {{a: Option<Int>, {field}}}\nderive json for Wrap"
        )));
        assert!(
            refused
                .iter()
                .any(|d| d.code == ply_span::codes::NOT_DERIVABLE),
            "`{field}` composes a codec that writes `Some` and `None` alike: {refused:?}"
        );
    }
}

/// Byte-identical across repeated expansions of one declaration, and across two declarations that
/// differ only in a name the encoding does not contain.
#[test]
fn expansion_is_a_function_of_the_declaration() {
    let of = |name: &str| {
        one(&format!(
            "{JSON}type {name} = {{id: Int, lines: List<String>, tags: Map<String, Int>}}\n\
             derive json for {name}"
        ))
    };
    let first = of("Order");
    for _ in 0..8 {
        assert_eq!(first, of("Order"), "expansion is not a pure function");
    }
    assert_eq!(
        first
            .replace("order_json", "purchase_json")
            .replace("Order", "Purchase"),
        of("Purchase"),
        "the type's name reaches the generated body somewhere other than its own signature"
    );
}
