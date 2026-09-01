use crate::rules::{generated_name, snake_case};
use crate::{expand_module, preview};
use ply_span::{Diagnostic, SourceId, Span, codes};
use ply_syntax::ast::{Deriver, Item, Module, ModuleName};
use ply_syntax::parser::parse_module;

const SRC: SourceId = SourceId(0);

fn parse(source: &str) -> Module {
    match parse_module(SRC, ModuleName::from_dotted("t"), source) {
        Ok(m) => m,
        Err(d) => panic!("the fixture must parse: {d:#?}"),
    }
}

/// The generated source, having also checked that expansion itself is clean and that what it
/// produced went into `items`.
fn generated(source: &str) -> Vec<String> {
    let mut module = parse(source);
    let before = module.items.len();
    let text = preview(&module);
    let diags = expand_module(&mut module);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    assert_eq!(module.items.len(), before + text.len());
    text
}

fn one(source: &str) -> String {
    let out = generated(source);
    assert_eq!(out.len(), 1, "{out:#?}");
    out.into_iter().next().expect("length checked")
}

fn errors(source: &str) -> Vec<Diagnostic> {
    let mut module = parse(source);
    let diags = expand_module(&mut module);
    assert!(!diags.is_empty(), "expected a diagnostic");
    diags
}

const JSON: &str = "import std.json\n";

#[test]
fn snake_case_follows_the_rule_including_acronyms() {
    assert_eq!(snake_case("Order"), "order");
    assert_eq!(snake_case("OrderLine"), "order_line");
    assert_eq!(snake_case("HTTPRequest"), "http_request");
    assert_eq!(snake_case("HttpRequest"), "http_request");
    assert_eq!(snake_case("Order2Line"), "order2_line");
    assert_eq!(snake_case("A"), "a");
    assert_eq!(
        generated_name(Deriver::Json, "OrderLine"),
        "order_line_json"
    );
}

#[test]
fn a_record_encodes_its_fields_in_declaration_order() {
    let g = one(&format!(
        "{JSON}type Order = {{id: Int, name: String}}\nderive json for Order"
    ));
    assert!(
        g.starts_with("fn order_json() -> json::JsonCodec<Order> = "),
        "{g}"
    );
    let id = g.find("\"id\"").expect("encodes `id`");
    let name = g.find("\"name\"").expect("encodes `name`");
    assert!(id < name, "fields are encoded in declaration order: {g}");
}

#[test]
fn a_sum_encodes_every_variant_by_its_declared_name() {
    let g = one(&format!(
        "{JSON}type Shape = Circle(Int) | Rect(Int, Int) | Point\nderive json for Shape"
    ));
    for tag in ["\"Circle\"", "\"Rect\"", "\"Point\""] {
        assert!(g.contains(tag), "{tag} missing from {g}");
    }
    assert!(g.contains("unknown_variant"), "{g}");
}

#[test]
fn a_named_field_type_is_composed_through_rather_than_inlined() {
    let g = one(&format!(
        "{JSON}type Line = {{qty: Int}}\ntype Order = {{lines: List<Line>}}\n\
         derive json for Order"
    ));
    assert!(g.contains("list_json(line_json())"), "{g}");
    assert!(
        !g.contains("\"qty\""),
        "`Line`'s structure is not copied: {g}"
    );
}

#[test]
fn a_qualified_field_type_keeps_its_module_binder() {
    let g = one(&format!(
        "{JSON}import users\ntype Order = {{who: users::User}}\nderive json for Order"
    ));
    assert!(g.contains("users::user_json()"), "{g}");
}

#[test]
fn a_parameterized_type_takes_a_dictionary_and_a_constraint_per_parameter() {
    let g = one(&format!(
        "{JSON}type Pair<x, y> = {{left: x, right: y}}\nderive json for Pair"
    ));
    assert!(
        g.contains("<x, y>(x: json::JsonCodec<x>, y: json::JsonCodec<y>)"),
        "{g}"
    );
    assert!(
        g.contains("where derivable(json, x), derivable(json, y)"),
        "{g}"
    );
}

#[test]
fn a_generated_definition_takes_the_targets_visibility() {
    let g = one(&format!(
        "{JSON}pub type Order = {{id: Int}}\nderive json for Order"
    ));
    assert!(g.starts_with("pub fn "), "{g}");
    let g = one(&format!(
        "{JSON}type Order = {{id: Int}}\nderive json for Order"
    ));
    assert!(g.starts_with("fn "), "{g}");
}

#[test]
fn eq_and_ord_delegate_to_the_languages_own_operations() {
    let out = generated("type Order = {id: Int}\nderive eq for Order\nderive ord for Order");
    assert!(out[0].contains("da == db"), "{}", out[0]);
    // The reserved builtin rather than `compare`, which a module may shadow.
    assert!(out[1].contains("compare_values(da, db)"), "{}", out[1]);
    // Structural, so neither needs a name from a module that has to be imported.
    assert!(
        out[0].contains("{eq: (Order, Order) -> Bool}"),
        "{}",
        out[0]
    );
    assert!(
        out[1].contains("{compare: (Order, Order) -> Ordering}"),
        "{}",
        out[1]
    );
}

#[test]
fn a_binder_cannot_shadow_a_dictionary_parameter_named_like_one() {
    let g = one(&format!(
        "{JSON}type Box<d> = {{item: d}}\nderive json for Box"
    ));
    // The parameter is `d`, so the emitter's own binders move out of its way.
    assert!(g.contains("d_v: Box<d>") || g.contains("d_0"), "{g}");
    assert!(!g.contains("|dv:"), "{g}");
}

#[test]
fn the_same_type_generates_byte_identical_source_every_time() {
    let source = format!(
        "{JSON}type Bag = {{a: Int, b: List<String>, c: Map<Int, Bytes>, d: Option<Int>}}\n\
         type Shape = Circle(Int) | Rect(Int, Int)\n\
         derive json for Bag\nderive eq for Bag\nderive ord for Bag\nderive json for Shape"
    );
    let first = generated(&source);
    assert_eq!(first.len(), 4);
    for _ in 0..64 {
        assert_eq!(generated(&source), first, "generation is not deterministic");
    }
}

#[test]
fn generation_does_not_depend_on_the_order_unrelated_items_were_written_in() {
    let a = one(&format!(
        "{JSON}type Order = {{id: Int}}\ntype Other = {{x: Int}}\nderive json for Order"
    ));
    let b = one(&format!(
        "{JSON}type Other = {{x: Int}}\ntype Order = {{id: Int}}\nderive json for Order"
    ));
    assert_eq!(a, b);
}

/// The golden pin.
#[test]
fn the_generated_form_is_pinned() {
    let source = format!(
        "{JSON}type Order = {{id: Int, lines: List<Line>}}\n\
         type Line = {{sku: String}}\n\
         type Status = Placed | Shipped(Int)\n\
         derive json for Order\nderive eq for Line\nderive ord for Line\nderive json for Status"
    );
    let out = generated(&source);
    let rendered = out.join("\n");
    let expected = r#"fn order_json() -> json::JsonCodec<Order> = {encode: |dv: Order| json::object([{key: "id", value: (json::int_json().encode)(dv.id)}, {key: "lines", value: (json::list_json(line_json()).encode)(dv.lines)}]), decode: |dj: json::Json| match json::field(dj, "id", json::int_json()) {Err(de) -> Err(de), Ok(d0) -> match json::field(dj, "lines", json::list_json(line_json())) {Err(de) -> Err(de), Ok(d1) -> Ok({id: d0, lines: d1})}}}
fn line_eq() -> {eq: (Line, Line) -> Bool} = {eq: |da: Line, db: Line| da == db}
fn line_ord() -> {compare: (Line, Line) -> Ordering} = {compare: |da: Line, db: Line| compare_values(da, db)}
fn status_json() -> json::JsonCodec<Status> = {encode: |dv: Status| match dv {Placed -> json::variant("Placed", []), Shipped(d0) -> json::variant("Shipped", [(json::int_json().encode)(d0)])}, decode: |dj: json::Json| match json::variant_of(dj) {Err(de) -> Err(de), Ok(dt) -> if dt.tag == "Placed" {Ok(Placed)} else {if dt.tag == "Shipped" {match json::decode_and_then(json::variant_value(dt, 0), json::int_json().decode) {Err(de) -> Err(de), Ok(d0) -> Ok(Shipped(d0))}} else {json::unknown_variant(dt.tag, ["Placed", "Shipped"])}}}}"#;
    assert_eq!(
        rendered, expected,
        "\nthe deriver's output moved. If that is intended, update this pin AND bump \
         FRONTEND_VERSION: gate 1 keys on raw file content, so without the bump a file \
         whose generated definition changed would be skipped and the stale one reused."
    );
}

#[test]
fn every_span_in_a_generated_definition_is_the_derive_items() {
    let source = format!("{JSON}type Order = {{id: Int}}\nderive json for Order");
    let mut module = parse(&source);
    let derive_span = module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Derive(d) => Some(d.span),
            _ => None,
        })
        .expect("the fixture has a `derive`");
    assert!(expand_module(&mut module).is_empty());
    let Some(Item::Fn(def)) = module.items.last() else {
        panic!("expected a generated fn")
    };
    let mut spans = Vec::new();
    collect_spans(&def.body, &mut spans);
    spans.push(def.span);
    spans.push(def.name.span);
    assert!(!spans.is_empty());
    for span in spans {
        assert_eq!(span, derive_span, "a generated span escaped retargeting");
    }
    assert_eq!(
        &source[derive_span.range()],
        "derive json for Order",
        "the span is the line the user can edit"
    );
}

fn collect_spans(e: &ply_syntax::ast::Expr, out: &mut Vec<Span>) {
    use ply_syntax::ast::ExprKind;
    out.push(e.span);
    match &e.kind {
        ExprKind::Lambda { params, body } => {
            for p in params {
                out.push(p.span);
            }
            collect_spans(body, out);
        }
        ExprKind::App { func, args, .. } => {
            collect_spans(func, out);
            for a in args {
                collect_spans(a, out);
            }
        }
        ExprKind::Field { base, .. } => collect_spans(base, out),
        ExprKind::Record { fields } => {
            for (n, e) in fields {
                out.push(n.span);
                collect_spans(e, out);
            }
        }
        ExprKind::List { items } => items.iter().for_each(|i| collect_spans(i, out)),
        ExprKind::Match { scrutinee, arms } => {
            collect_spans(scrutinee, out);
            for arm in arms {
                out.push(arm.span);
                out.push(arm.pat.span);
                collect_spans(&arm.body, out);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_spans(cond, out);
            collect_spans(then_branch, out);
            collect_spans(else_branch, out);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_spans(lhs, out);
            collect_spans(rhs, out);
        }
        _ => {}
    }
}

#[test]
fn a_function_field_is_refused_naming_the_field() {
    let d = errors(&format!(
        "{JSON}type Order = {{id: Int, on_complete: (Int) -> Unit}}\nderive json for Order"
    ));
    assert_eq!(d[0].code, codes::NOT_DERIVABLE);
    let span = d[0].primary_span().expect("a primary label");
    let source = format!(
        "{JSON}type Order = {{id: Int, on_complete: (Int) -> Unit}}\nderive json for Order"
    );
    assert_eq!(
        &source[span.range()],
        "on_complete: (Int) -> Unit",
        "the whole field is blamed, not the arrow inside it"
    );
}

#[test]
fn a_cell_field_is_refused() {
    let d = errors(&format!(
        "{JSON}type S = {{c: Cell<Int>}}\nderive json for S"
    ));
    assert_eq!(d[0].code, codes::NOT_DERIVABLE);
}

#[test]
fn ord_refuses_float_where_json_and_eq_accept_it() {
    let source = "type P = {x: Float}\nderive eq for P";
    assert_eq!(generated(source).len(), 1);
    let d = errors("type P = {x: Float}\nderive ord for P");
    assert_eq!(d[0].code, codes::NOT_DERIVABLE);
    assert!(d[0].labels[0].message.contains("Float"), "{:#?}", d[0]);
}

#[test]
fn a_map_key_must_be_ordered_whatever_deriver_is_walking() {
    let d = errors(&format!(
        "{JSON}type Prices = {{by: Map<Float, Int>}}\nderive json for Prices"
    ));
    assert_eq!(d[0].code, codes::NOT_DERIVABLE);
    assert!(d[0].labels[0].message.contains("ordered"), "{:#?}", d[0]);
}

#[test]
fn a_derive_naming_a_type_this_module_does_not_declare_is_an_orphan() {
    let d = errors(&format!("{JSON}derive json for Order"));
    assert_eq!(d[0].code, codes::ORPHAN_DERIVE);
}

#[test]
fn two_derivations_that_generate_one_name_are_reported_against_both() {
    let d = errors("type Order = {id: Int}\nderive eq for Order\nderive eq for Order");
    assert_eq!(d[0].code, codes::DUPLICATE_DEFINITION);
    assert_eq!(d[0].labels.len(), 2, "both `derive` lines are labelled");
}

#[test]
fn a_module_without_the_deriver_s_runtime_module_is_told_to_import_it() {
    let d = errors("type Order = {id: Int}\nderive json for Order");
    assert_eq!(d[0].code, codes::NOT_DERIVABLE);
    assert!(d[0].notes.iter().any(|n| n.contains("import std.json")));
}

/// A selective import binds no module name, so expansion adds one and the generated body writes
/// through it.
#[test]
fn a_selective_import_of_std_json_still_writes_a_module_binder() {
    let source = "import std.json (JsonCodec, Json, object, field, int_json)\n\
                  type Order = {id: Int}\nderive json for Order";
    let g = one(source);
    assert!(g.contains("-> json::JsonCodec<Order>"), "{g}");
    assert!(g.contains("json::int_json()"), "{g}");

    let mut module = parse(source);
    let before = module.imports.len();
    assert!(expand_module(&mut module).is_empty());
    assert_eq!(module.imports.len(), before + 1, "no binder was added");
    assert_eq!(
        module.imports[before].binder().map(|b| b.to_string()),
        Some(String::from("json"))
    );
}

/// The binder expansion adds cannot collide with one the file already bound, and which name it
/// picks is a function of the file's own imports.
#[test]
fn a_synthesized_binder_steps_around_the_names_the_file_already_binds() {
    let g = one(
        "import other.json\nimport std.json (JsonCodec, Json, object, field, int_json)\n\
         type Order = {id: Int}\nderive json for Order",
    );
    assert!(g.contains("-> json_::JsonCodec<Order>"), "{g}");
}

#[test]
fn an_aliased_import_uses_the_alias() {
    let g = one("import std.json as j\ntype Order = {id: Int}\nderive json for Order");
    assert!(g.contains("-> j::JsonCodec<Order>"), "{g}");
}

#[test]
fn a_module_with_no_derive_is_untouched() {
    let mut module = parse("type Order = {id: Int}\nfn f() = 1");
    let before = module.items.len();
    assert!(expand_module(&mut module).is_empty());
    assert_eq!(module.items.len(), before);
}
