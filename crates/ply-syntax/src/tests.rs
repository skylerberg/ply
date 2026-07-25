//! The dumper at the bottom exists so a test asserts the *whole* shape of a
//! parse rather than poking at one field; a wrong nesting anywhere shows up as
//! a string diff.

use crate::ast::*;
use crate::parser::{parse, parse_recovering};
use ply_span::{Diagnostic, SourceId, Span, codes};

const SRC: SourceId = SourceId(0);

fn ok(src: &str) -> Module {
    match parse(SRC, src) {
        Ok(m) => m,
        Err(d) => panic!("unexpected diagnostics for {src:?}:\n{d:#?}"),
    }
}

fn dump(src: &str) -> String {
    dump_module(&ok(src))
}

fn expr(src: &str) -> String {
    let m = ok(&format!("fn t() = {src}"));
    let Item::Fn(f) = &m.items[0] else { panic!("expected a fn") };
    dump_expr(&f.body)
}

fn errs(src: &str) -> Vec<Diagnostic> {
    parse(SRC, src).expect_err("expected parse errors")
}

fn snippet(src: &str, span: Span) -> String {
    src[span.range()].to_string()
}

#[test]
fn fn_with_expression_body() {
    assert_eq!(dump("fn one() = 1"), "(fn one () 1)");
}

#[test]
fn fn_with_block_body() {
    assert_eq!(dump("fn one() { 1 }"), "(fn one () (block 1))");
}

#[test]
fn fn_with_params_return_type_and_row() {
    assert_eq!(
        dump("fn get(k: Int, d) -> Option<Row> / {db.read[users]} = d"),
        "(fn get ((k Int) (d _)) -> Option<Row> / {db.read[users]} d)"
    );
}

#[test]
fn fn_generics_split_types_from_effect_vars() {
    assert_eq!(
        dump("fn map<a, b | e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e = xs"),
        "(fn map <a, b | e> ((xs List<a>) (f (fn (a) -> b / {| e}))) -> List<b> / {| e} xs)"
    );
}

#[test]
fn fn_generics_may_be_only_effect_vars() {
    assert_eq!(dump("fn go<|e>() / e = 1"), "(fn go < | e> () / {| e} 1)");
}

#[test]
fn fn_generics_may_be_only_types() {
    assert_eq!(dump("fn id<a>(x: a) -> a = x"), "(fn id <a> ((x a)) -> a x)");
}

#[test]
fn row_may_be_a_bare_effect_variable_or_a_braced_set() {
    assert_eq!(dump("fn a() / e = 1"), "(fn a () / {| e} 1)");
    assert_eq!(dump("fn b() / {} = 1"), "(fn b () / {} 1)");
    assert_eq!(
        dump("fn c() / {db.write[orders], clock.read | e} = 1"),
        "(fn c () / {db.write[orders], clock.read | e} 1)"
    );
}

#[test]
fn empty_row_with_only_a_tail() {
    assert_eq!(dump("fn c() / {|e} = 1"), "(fn c () / {| e} 1)");
}

#[test]
fn type_alias_versus_sum() {
    assert_eq!(dump("type Id = Int"), "(type Id = Int)");
    assert_eq!(dump("type Pair = {a: Int, b: String}"), "(type Pair = {a: Int, b: String})");
    assert_eq!(
        dump("type Option<a> = None | Some(a)"),
        "(type Option <a> = (| (v None) (v Some a)))"
    );
    assert_eq!(
        dump("type Color =\n  | Red\n  | Green\n  | Blue"),
        "(type Color = (| (v Red) (v Green) (v Blue)))"
    );
    assert_eq!(dump("type Wrap = Wrap(Int)"), "(type Wrap = (| (v Wrap Int)))");
}

#[test]
fn type_param_list_followed_immediately_by_eq() {
    assert_eq!(dump("type Box<a>= List<a>"), "(type Box <a> = List<a>)");
}

#[test]
fn effect_def_with_modes_and_resource_params() {
    assert_eq!(
        dump(
            "effect db {\n  read  get[r](key: Int) -> Option<Row>\n  write put[r](key: Int, value: Row) -> Unit\n}"
        ),
        "(effect db (op read get[r] (Int) -> Option<Row>) (op write put[r] (Int, Row) -> Unit))"
    );
}

#[test]
fn nondet_effect_and_singleton_operation() {
    assert_eq!(
        dump("nondet effect clock {\n  read now() -> Int\n}"),
        "(nondet effect clock (op read now () -> Int))"
    );
}

#[test]
fn effect_with_no_operations() {
    assert_eq!(dump("effect noop {}"), "(effect noop)");
}

#[test]
fn test_items_carry_their_name_and_determinism() {
    assert_eq!(dump("test \"a name\" { 1 }"), "(test \"a name\" (block 1))");
    assert_eq!(dump("test/nondet \"b\" { 1 }"), "(test/nondet \"b\" (block 1))");
}

#[test]
fn test_name_span_covers_only_the_string() {
    let src = "test \"hello\" { 1 }";
    let m = ok(src);
    let Item::Test(t) = &m.items[0] else { panic!() };
    assert_eq!(snippet(src, t.name_span), "\"hello\"");
    assert_eq!(t.name, "hello");
}

#[test]
fn literals() {
    assert_eq!(expr("1_000"), "1000");
    assert_eq!(expr("true"), "true");
    assert_eq!(expr("\"hi\\n\""), "\"hi\\n\"");
    assert_eq!(expr("()"), "unit");
}

#[test]
fn arithmetic_binds_tighter_than_comparison_which_binds_tighter_than_logic() {
    assert_eq!(expr("a || b && c"), "(|| a (&& b c))");
    assert_eq!(expr("a && b || c"), "(|| (&& a b) c)");
    assert_eq!(expr("a == b + c"), "(== a (+ b c))");
    assert_eq!(expr("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(expr("1 * 2 + 3"), "(+ (* 1 2) 3)");
    assert_eq!(expr("a ++ b == c"), "(== (++ a b) c)");
    assert_eq!(expr("a + b ++ c + d"), "(++ (+ a b) (+ c d))");
}

#[test]
fn binary_operators_are_left_associative() {
    assert_eq!(expr("1 - 2 - 3"), "(- (- 1 2) 3)");
    assert_eq!(expr("1 / 2 / 3"), "(/ (/ 1 2) 3)");
    assert_eq!(expr("a ++ b ++ c"), "(++ (++ a b) c)");
    assert_eq!(expr("a || b || c"), "(|| (|| a b) c)");
}

#[test]
fn unary_binds_tighter_than_arithmetic_and_nests() {
    assert_eq!(expr("-a * b"), "(* (neg a) b)");
    assert_eq!(expr("-(a * b)"), "(neg (* a b))");
    assert_eq!(expr("!a && b"), "(&& (not a) b)");
    assert_eq!(expr("--a"), "(neg (neg a))");
    assert_eq!(expr("!f(x)"), "(not (call f x))");
}

#[test]
fn parentheses_override_precedence() {
    assert_eq!(expr("(1 + 2) * 3"), "(* (+ 1 2) 3)");
}

#[test]
fn application_and_field_access_bind_tightest() {
    assert_eq!(expr("f(x) + g(y)"), "(+ (call f x) (call g y))");
    assert_eq!(expr("f(x)(y)"), "(call (call f x) y)");
    assert_eq!(expr("f()"), "(call f)");
    assert_eq!(expr("-r.x"), "(neg (field r x))");
}

#[test]
fn perform_versus_field_versus_qualified_name() {
    assert_eq!(expr("db.get[users](k)"), "(perform db.get[users] k)");
    assert_eq!(expr("clock.now()"), "(perform clock.now)");
    assert_eq!(expr("record.field"), "(field record field)");
    assert_eq!(expr("Mod.thing"), "(field Mod thing)");
    assert_eq!(expr("a.b.c"), "(field (field a b) c)");
    assert_eq!(expr("f(x).load(y)"), "(call (field (call f x) load) y)");
    assert_eq!(expr("db.get[users](k).name"), "(field (perform db.get[users] k) name)");
}

#[test]
fn lambdas_with_zero_one_and_many_parameters() {
    assert_eq!(expr("|| 1"), "(lam () 1)");
    assert_eq!(expr("|x| x + 1"), "(lam ((x _)) (+ x 1))");
    assert_eq!(expr("|x: Int, y| x"), "(lam ((x Int) (y _)) x)");
    assert_eq!(expr("f(|| 1, |x| x)"), "(call f (lam () 1) (lam ((x _)) x))");
}

#[test]
fn a_type_annotation_ending_in_an_angle_bracket_still_finds_its_eq() {
    assert_eq!(
        expr("{ let x: List<Int>= []; x }"),
        "(block (let x List<Int> (list)) x)"
    );
    assert_eq!(
        expr("{ let x: Map<K, List<V>> = m; x }"),
        "(block (let x Map<K, List<V>> m) x)"
    );
}

#[test]
fn comparison_operators_are_not_mistaken_for_type_arguments() {
    assert_eq!(expr("a < b"), "(< a b)");
    assert_eq!(expr("a >= b"), "(>= a b)");
}

#[test]
fn a_lambda_on_the_right_of_or_is_not_confused_with_the_operator() {
    assert_eq!(expr("a || || b"), "(|| a (lam () b))");
    assert_eq!(expr("a || |x| x"), "(|| a (lam ((x _)) x))");
}

#[test]
fn if_else_and_else_if_chains() {
    assert_eq!(expr("if a { 1 } else { 2 }"), "(if a (block 1) (block 2))");
    assert_eq!(
        expr("if a { 1 } else if b { 2 } else { 3 }"),
        "(if a (block 1) (if b (block 2) (block 3)))"
    );
}

#[test]
fn if_without_else_yields_unit() {
    assert_eq!(expr("if a { 1 }"), "(if a (block 1) unit)");
}

#[test]
fn an_if_condition_does_not_swallow_the_then_block() {
    assert_eq!(expr("if x == 1 { 2 } else { 3 }"), "(if (== x 1) (block 2) (block 3))");
    assert_eq!(expr("if f(x) { 2 } else { 3 }"), "(if (call f x) (block 2) (block 3))");
    assert_eq!(
        expr("if f({a: 1}) { 2 } else { 3 }"),
        "(if (call f (rec (a 1))) (block 2) (block 3))"
    );
}

#[test]
fn blocks_are_statements_plus_an_optional_tail() {
    assert_eq!(expr("{ }"), "(block)");
    assert_eq!(expr("{ 1 }"), "(block 1)");
    assert_eq!(expr("{ f(); }"), "(block (call f))");
    assert_eq!(expr("{ f(); 2 }"), "(block (call f) 2)");
    assert_eq!(
        expr("{ let x = 1; let y: Int = 2; x + y }"),
        "(block (let x 1) (let y Int 2) (+ x y))"
    );
}

#[test]
fn a_block_like_statement_needs_no_semicolon() {
    assert_eq!(
        expr("{ if a { f(); } else { g(); } 1 }"),
        "(block (if a (block (call f)) (block (call g))) 1)"
    );
}

#[test]
fn record_literals_versus_blocks() {
    assert_eq!(expr("{a: 1, b: 2}"), "(rec (a 1) (b 2))");
    assert_eq!(expr("{a,}"), "(rec (a a))");
    assert_eq!(expr("{a}"), "(block a)");
    assert_eq!(expr("{a: 1}.a"), "(field (rec (a 1)) a)");
}

#[test]
fn lists() {
    assert_eq!(expr("[]"), "(list)");
    assert_eq!(expr("[1, 2, 3]"), "(list 1 2 3)");
    assert_eq!(expr("[[1], [2]]"), "(list (list 1) (list 2))");
}

#[test]
fn handle_with_clauses_and_a_return_clause() {
    assert_eq!(
        expr(
            "handle body() with {\n  db.get[users](k) -> lookup(k),\n  db.put[users](k, v) -> set(k, v),\n  return x -> x,\n}"
        ),
        "(handle (call body) (clause db.get[users] (k) (call lookup k)) \
         (clause db.put[users] (k v) (call set k v)) (ret x x))"
    );
}

#[test]
fn handle_over_a_block_body() {
    assert_eq!(
        expr("handle { f() } with { clock.now() -> 0 }"),
        "(handle (block (call f)) (clause clock.now () 0))"
    );
}

#[test]
fn handler_clause_commas_are_optional() {
    assert_eq!(
        expr("handle f() with { a.b() -> 1 c.d() -> 2 }"),
        "(handle (call f) (clause a.b () 1) (clause c.d () 2))"
    );
}

#[test]
fn a_return_clause_is_recognised_by_the_binder_that_follows() {
    assert_eq!(expr("handle f() with { return x -> x }"), "(handle (call f) (ret x x))");
}

#[test]
fn with_cell_binds_a_region_scoped_cell() {
    assert_eq!(
        expr(
            "with_cell[users](seed) { cell -> handle body() with { db.get[users](k) -> ref(cell) } }"
        ),
        "(with_cell [users] seed cell (handle (call body) (clause db.get[users] (k) (call ref cell))))"
    );
}

#[test]
fn with_cell_is_only_special_before_a_bracket() {
    assert_eq!(expr("with_cell"), "with_cell");
    assert_eq!(expr("with_cell(1)"), "(call with_cell 1)");
}

#[test]
fn match_arms_cover_every_pattern_form() {
    assert_eq!(
        expr(
            "match v {\n  0 -> a,\n  -1 -> b,\n  \"s\" -> c,\n  true -> d,\n  None -> e,\n  Some(x) -> x,\n  Pair(Some(x), _) -> x,\n  {a: p, b, ..} -> p,\n  [x, y] -> x,\n  [x, ..rest] -> rest,\n  [x, ..] -> x,\n  other -> other,\n  _ -> z,\n}"
        ),
        "(match v (arm 0 a) (arm -1 b) (arm \"s\" c) (arm true d) (arm (ctor None) e) \
         (arm (ctor Some x) x) (arm (ctor Pair (ctor Some x) _) x) (arm (prec (a p) (b b) ..) p) \
         (arm (plist x y) x) (arm (plist x .. rest) rest) (arm (plist x .. _) x) (arm other other) (arm _ z))"
    );
}

#[test]
fn match_arm_guards() {
    assert_eq!(
        expr("match v { Some(x) if x > 0 -> x, _ -> 0 }"),
        "(match v (arm (ctor Some x) (guard (> x 0)) x) (arm _ 0))"
    );
}

#[test]
fn a_match_scrutinee_does_not_swallow_the_arm_block() {
    assert_eq!(expr("match f(x) { _ -> 1 }"), "(match (call f x) (arm _ 1))");
}

#[test]
fn match_arms_with_block_bodies_need_no_comma() {
    assert_eq!(
        expr("match v { A -> { 1 } B -> { 2 } }"),
        "(match v (arm (ctor A) (block 1)) (arm (ctor B) (block 2)))"
    );
}

#[test]
fn function_types_nest_to_the_right_and_carry_rows() {
    assert_eq!(
        dump("fn f(g: (Int, Int) -> Bool / {db.read}) -> () -> Unit = g"),
        "(fn f ((g (fn (Int, Int) -> Bool / {db.read}))) -> (fn () -> Unit) g)"
    );
    assert_eq!(
        dump("fn f() -> (Int) -> (Int) -> Int = f"),
        "(fn f () -> (fn (Int) -> (fn (Int) -> Int)) f)"
    );
}

#[test]
fn a_parenthesized_type_is_not_a_tuple() {
    assert_eq!(dump("fn f(x: (Int)) -> Unit = x"), "(fn f ((x Int)) -> Unit x)");
}

#[test]
fn nested_type_arguments_close_with_two_angle_brackets() {
    assert_eq!(
        dump("fn f(x: List<List<Int>>) -> Unit = x"),
        "(fn f ((x List<List<Int>>)) -> Unit x)"
    );
}

#[test]
fn lowercase_type_names_are_variables_and_uppercase_are_constructors() {
    assert_eq!(dump("fn f(x: a, y: A) -> Unit = x"), "(fn f ((x a) (y A)) -> Unit x)");
}

#[test]
fn the_unit_type_is_written_with_empty_parens() {
    assert_eq!(dump("fn f(x: ()) -> () = x"), "(fn f ((x ())) -> () x)");
}

#[test]
fn a_binary_expression_spans_both_operands() {
    let src = "fn f() = 1 + 2 * 3";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(snippet(src, f.body.span), "1 + 2 * 3");
    let ExprKind::Binary { rhs, .. } = &f.body.kind else { panic!() };
    assert_eq!(snippet(src, rhs.span), "2 * 3");
}

#[test]
fn a_perform_spans_from_the_effect_to_the_closing_paren() {
    let src = "fn f() = db.get[users](k)";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(snippet(src, f.body.span), "db.get[users](k)");
    let ExprKind::Perform { effect, op, resource, .. } = &f.body.kind else { panic!() };
    assert_eq!(snippet(src, effect.span), "db");
    assert_eq!(snippet(src, op.span), "get");
    assert_eq!(snippet(src, resource.as_ref().unwrap().span), "users");
}

#[test]
fn an_item_spans_its_whole_definition() {
    let src = "fn f() {\n  1\n}\n\nfn g() = 2";
    let m = ok(src);
    assert_eq!(snippet(src, m.items[0].span()), "fn f() {\n  1\n}");
    assert_eq!(snippet(src, m.items[1].span()), "fn g() = 2");
}

#[test]
fn a_parenthesized_expression_span_includes_its_parens() {
    let src = "fn f() = (1 + 2)";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(snippet(src, f.body.span), "(1 + 2)");
}

#[test]
fn a_let_statement_spans_through_its_semicolon() {
    let src = "fn f() { let x = 1; x }";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let ExprKind::Block { stmts, .. } = &f.body.kind else { panic!() };
    let Stmt::Let { span, .. } = &stmts[0] else { panic!() };
    assert_eq!(snippet(src, *span), "let x = 1;");
}

#[test]
fn a_synthesized_else_branch_gets_an_empty_span_at_the_end_of_the_then_block() {
    let src = "fn f() = if a { 1 }";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let ExprKind::If { then_branch, else_branch, .. } = &f.body.kind else { panic!() };
    assert!(!else_branch.span.is_dummy());
    assert_eq!(else_branch.span.start, else_branch.span.end);
    assert_eq!(else_branch.span.start, then_branch.span.end);
}

#[test]
fn every_diagnostic_carries_a_code_and_a_real_span() {
    for src in [
        "fn",
        "fn f",
        "fn f(",
        "fn f()",
        "fn f() =",
        "fn f() = 1 +",
        "fn f() { let",
        "fn f() { let x = }",
        "fn f() { 1 2 }",
        "fn f() { ",
        "type",
        "type T",
        "type T =",
        "type T = |",
        "effect",
        "effect e {",
        "effect e { get() -> Int }",
        "effect e { read get() }",
        "nondet fn f() = 1",
        "test",
        "test \"a\"",
        "test/ \"a\" { }",
        "test \"a\" { ",
        "fn f() / = 1",
        "fn f() -> = 1",
        "fn f<a() = 1",
        "fn f() = match",
        "fn f() = handle",
        "fn f() = handle x with {",
        "fn f() = with_cell[r]",
        "fn f() = |x",
        "fn f() = [1,",
        "fn f() = f(1,",
        "fn f() = db.get[](k)",
        "fn f() = .x",
        "fn f() = x.",
        "fn f() = {a: }",
        "?",
        "\"unterminated",
    ] {
        let ds = errs(src);
        assert!(!ds.is_empty(), "expected a diagnostic for {src:?}");
        for d in &ds {
            assert!(!d.code.is_empty(), "empty code for {src:?}");
            let span = d.primary_span().unwrap_or_else(|| panic!("no span for {src:?}"));
            assert!(!span.is_dummy(), "dummy span for {src:?}");
            assert!(span.end as usize <= src.len(), "span past end for {src:?}");
            assert!(span.start <= span.end, "inverted span for {src:?}");
        }
    }
}

#[test]
fn a_bad_item_does_not_stop_later_items_from_parsing() {
    let (module, diags) = parse_recovering(SRC, "fn a() = ;\nfn b() = 1\nfn c() = 2");
    assert_eq!(diags.len(), 1);
    assert_eq!(dump_module(&module), "(fn b () 1)\n(fn c () 2)");
}

#[test]
fn several_independent_errors_are_reported_in_one_run() {
    let (module, diags) =
        parse_recovering(SRC, "fn a() = ;\ntype T = ;\neffect e { bogus }\ntest \"t\" { 1 }");
    assert!(diags.len() >= 3, "expected several diagnostics, got {diags:#?}");
    assert_eq!(module.items.len(), 1, "the well-formed test should still parse");
    assert!(matches!(&module.items[0], Item::Test(_)));
}

#[test]
fn recovery_skips_over_braces_inside_the_broken_item() {
    let (module, diags) = parse_recovering(SRC, "fn a() { let = 1; }\nfn b() = 3");
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(dump_module(&module), "(fn b () 3)");
}

#[test]
fn an_unexpected_token_names_what_was_expected_and_what_was_found() {
    let ds = errs("fn f() = 1 fn");
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].code, codes::UNEXPECTED_TOKEN);
    assert!(ds[0].message.contains("end of file"), "{}", ds[0].message);
}

#[test]
fn an_unclosed_delimiter_points_at_where_it_opened() {
    let src = "fn f() { 1 ";
    let ds = errs(src);
    let opened = ds[0].labels.iter().find(|l| !l.primary).expect("a secondary label");
    assert_eq!(snippet(src, opened.span), "{");
}

#[test]
fn a_missing_function_body_says_what_to_write() {
    let ds = errs("fn f() 1");
    assert_eq!(ds.len(), 1);
    assert!(ds[0].message.contains("`=` or `{`"), "{}", ds[0].message);
}

#[test]
fn an_operation_without_a_mode_says_read_or_write() {
    let ds = errs("effect db { get() -> Int }");
    assert!(ds[0].message.contains("`read` or `write`"), "{}", ds[0].message);
}

#[test]
fn a_tuple_type_is_rejected_with_a_note() {
    let ds = errs("fn f(x: (Int, Bool)) = x");
    assert!(ds[0].notes.iter().any(|n| n.contains("record")), "{:#?}", ds[0]);
}

#[test]
fn a_duplicate_return_clause_is_reported_once() {
    let ds = errs("fn f() = handle g() with { return x -> x, return y -> y }");
    assert_eq!(ds.len(), 1);
    assert!(ds[0].message.contains("only one `return`"), "{}", ds[0].message);
}

#[test]
fn lexer_diagnostics_reach_the_parse_result() {
    let ds = errs("fn f() = \"oops");
    assert!(ds.iter().any(|d| d.code == codes::UNTERMINATED_STRING), "{ds:#?}");
}

#[test]
fn parsing_terminates_on_pathological_input() {
    for src in ["{{{{{{{{", "((((((((", "fn f(((((", "|||||", "....", "<<<<", "-----", "fn fn fn"] {
        let (_, diags) = parse_recovering(SRC, src);
        assert!(!diags.is_empty(), "expected a diagnostic for {src:?}");
    }
}

#[test]
fn pathological_nesting_is_a_diagnostic_rather_than_a_stack_overflow() {
    for (open, close) in [("(", ")"), ("[", "]"), ("-", ""), ("!", "")] {
        let src = format!("fn f() = {}x{}", open.repeat(20_000), close.repeat(20_000));
        let ds = errs(&src);
        assert!(
            ds.iter().any(|d| d.message.contains("nested too deeply")),
            "expected a depth diagnostic for {open:?}: {:?}",
            ds.first().map(|d| &d.message)
        );
    }

    let ty = format!("fn f(x: {}Int{}) = x", "List<".repeat(20_000), ">".repeat(20_000));
    assert!(errs(&ty).iter().any(|d| d.message.contains("nested too deeply")));

    let pat = format!(
        "fn f() = match v {{ {}x{} -> 1 }}",
        "Some(".repeat(20_000),
        ")".repeat(20_000)
    );
    assert!(errs(&pat).iter().any(|d| d.message.contains("nested too deeply")));
}

#[test]
fn a_long_flat_expression_does_not_hit_the_depth_limit() {
    let terms: Vec<String> = (0..5_000).map(|i| i.to_string()).collect();
    ok(&format!("fn f() = {}", terms.join(" + ")));
}

#[test]
fn token_soup_never_panics_or_hangs() {
    const ALPHABET: [&str; 28] = [
        "fn", "type", "effect", "nondet", "test", "let", "if", "else", "match", "handle", "with",
        "x", "X", "\"s\"", "1", "(", ")", "{", "}", "[", "]", "|", "||", ",", ";", ".", "->", "=",
    ];
    // xorshift so the corpus is reproducible without a dependency.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..3_000 {
        let len = (next() % 24) as usize;
        let src: Vec<&str> =
            (0..len).map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize]).collect();
        let src = src.join(" ");
        let (_, diags) = parse_recovering(SRC, &src);
        for d in &diags {
            let span = d.primary_span().expect("every diagnostic has a span");
            assert!(span.end as usize <= src.len(), "span past end of {src:?}");
        }
    }
}

#[test]
fn an_empty_module_parses() {
    assert_eq!(dump(""), "");
    assert_eq!(dump("// just a comment\n"), "");
}

#[test]
fn the_design_document_example_parses() {
    let src = r#"
// Effects are declared with modes and resource parameters.
effect db {
  read  get[r](key: Int) -> Option<Row>
  write put[r](key: Int, value: Row) -> Unit
}

nondet effect clock {
  read now() -> Int
}

type Row = { id: Int, active: Bool }
type Option<a> = None | Some(a)

fn active_users() -> List<User> / {db.read[users]} = filter(all_users(), |u| u.active)

fn map<a, b | e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e = {
  fold(xs, [], |acc, x| push(acc, f(x)))
}

test "active_users excludes inactive" {
  with_cell[users](seed) { cell ->
    handle {
      assert_eq(len(active_users()), 2)
    } with { db.get[users](k) -> map_lookup(cell_get(cell), k) }
  }
}

test/nondet "clock moves forward" {
  let a = clock.now();
  let b = clock.now();
  assert(b >= a)
}
"#;
    let m = ok(src);
    assert_eq!(m.items.len(), 8);
    assert!(matches!(&m.items[1], Item::Effect(e) if e.nondet));
    assert!(matches!(&m.items[6], Item::Test(t) if !t.nondet));
    assert!(matches!(&m.items[7], Item::Test(t) if t.nondet));
}

#[test]
fn reformatting_does_not_change_the_parse() {
    let dense = "fn f(x:Int)->Int/{db.read[users]}={let y=x+1;y*2}";
    let loose = "fn f(x: Int) -> Int / { db.read[users] } = {\n  // a comment\n  let y = x + 1;\n  y * 2\n}";
    assert_eq!(dump(dense), dump(loose));
}

fn dump_module(m: &Module) -> String {
    m.items.iter().map(dump_item).collect::<Vec<_>>().join("\n")
}

fn dump_item(i: &Item) -> String {
    match i {
        Item::Fn(f) => {
            let mut s = format!("(fn {}", f.name.name);
            if !f.generics.types.is_empty() || !f.generics.effects.is_empty() {
                s.push_str(&format!(" {}", dump_generics(&f.generics)));
            }
            s.push_str(&format!(" ({})", dump_params(&f.params)));
            if let Some(r) = &f.ret {
                s.push_str(&format!(" -> {}", dump_ty(r)));
            }
            if let Some(r) = &f.effects {
                s.push_str(&format!(" / {}", dump_row(r)));
            }
            format!("{s} {})", dump_expr(&f.body))
        }
        Item::Type(t) => {
            let mut s = format!("(type {}", t.name.name);
            if !t.params.is_empty() {
                let ps: Vec<_> = t.params.iter().map(|p| p.name.to_string()).collect();
                s.push_str(&format!(" <{}>", ps.join(", ")));
            }
            let body = match &t.body {
                TypeDefBody::Alias(a) => dump_ty(a),
                TypeDefBody::Sum(vs) => {
                    let vs: Vec<_> = vs
                        .iter()
                        .map(|v| {
                            let fs: Vec<_> = v.fields.iter().map(dump_ty).collect();
                            if fs.is_empty() {
                                format!("(v {})", v.name.name)
                            } else {
                                format!("(v {} {})", v.name.name, fs.join(" "))
                            }
                        })
                        .collect();
                    format!("(| {})", vs.join(" "))
                }
            };
            format!("{s} = {body})")
        }
        Item::Effect(e) => {
            let head = if e.nondet { "nondet effect" } else { "effect" };
            let mut s = format!("({head} {}", e.name.name);
            for op in &e.ops {
                let ps: Vec<_> = op.params.iter().map(dump_ty).collect();
                let res = if op.resource_param { "[r]" } else { "" };
                s.push_str(&format!(
                    " (op {} {}{} ({}) -> {})",
                    op.mode.as_str(),
                    op.name.name,
                    res,
                    ps.join(", "),
                    dump_ty(&op.ret)
                ));
            }
            format!("{s})")
        }
        Item::Test(t) => {
            let head = if t.nondet { "test/nondet" } else { "test" };
            format!("({head} {:?} {})", t.name, dump_expr(&t.body))
        }
    }
}

fn dump_generics(g: &Generics) -> String {
    let ts: Vec<_> = g.types.iter().map(|i| i.name.to_string()).collect();
    let es: Vec<_> = g.effects.iter().map(|i| i.name.to_string()).collect();
    if es.is_empty() {
        format!("<{}>", ts.join(", "))
    } else {
        format!("<{} | {}>", ts.join(", "), es.join(", "))
    }
}

fn dump_params(ps: &[Param]) -> String {
    ps.iter()
        .map(|p| match &p.ty {
            Some(t) => format!("({} {})", p.name.name, dump_ty(t)),
            None => format!("({} _)", p.name.name),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn dump_ty(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Var(i) => i.name.to_string(),
        TypeExpr::Con { name, args, .. } if args.is_empty() => name.name.to_string(),
        TypeExpr::Con { name, args, .. } => {
            let args: Vec<_> = args.iter().map(dump_ty).collect();
            format!("{}<{}>", name.name, args.join(", "))
        }
        TypeExpr::Fn { params, ret, effects, .. } => {
            let ps: Vec<_> = params.iter().map(dump_ty).collect();
            let eff = effects.as_ref().map(|r| format!(" / {}", dump_row(r))).unwrap_or_default();
            format!("(fn ({}) -> {}{})", ps.join(", "), dump_ty(ret), eff)
        }
        TypeExpr::Record { fields, .. } => {
            let fs: Vec<_> =
                fields.iter().map(|(n, t)| format!("{}: {}", n.name, dump_ty(t))).collect();
            format!("{{{}}}", fs.join(", "))
        }
        TypeExpr::Unit { .. } => "()".to_string(),
    }
}

fn dump_row(r: &RowExpr) -> String {
    let atoms: Vec<_> = r
        .atoms
        .iter()
        .map(|a| {
            let res = a.resource.as_ref().map(|r| format!("[{}]", r.name)).unwrap_or_default();
            format!("{}.{}{}", a.effect.name, a.mode.as_str(), res)
        })
        .collect();
    match &r.tail {
        None => format!("{{{}}}", atoms.join(", ")),
        Some(t) if atoms.is_empty() => format!("{{| {}}}", t.name),
        Some(t) => format!("{{{} | {}}}", atoms.join(", "), t.name),
    }
}

fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::Concat => "++",
    }
}

fn dump_lit(l: &Lit) -> String {
    match l {
        Lit::Int(v) => v.to_string(),
        Lit::Bool(b) => b.to_string(),
        Lit::Str(s) => format!("{s:?}"),
        Lit::Unit => "unit".to_string(),
    }
}

fn dump_expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Lit(l) => dump_lit(l),
        ExprKind::Var(i) => i.name.to_string(),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", op_str(*op), dump_expr(lhs), dump_expr(rhs))
        }
        ExprKind::Unary { op, operand } => {
            let n = match op {
                UnOp::Neg => "neg",
                UnOp::Not => "not",
            };
            format!("({n} {})", dump_expr(operand))
        }
        ExprKind::Lambda { params, body } => {
            format!("(lam ({}) {})", dump_params(params), dump_expr(body))
        }
        ExprKind::App { func, args } => {
            let mut s = format!("(call {}", dump_expr(func));
            for a in args {
                s.push_str(&format!(" {}", dump_expr(a)));
            }
            format!("{s})")
        }
        ExprKind::If { cond, then_branch, else_branch } => format!(
            "(if {} {} {})",
            dump_expr(cond),
            dump_expr(then_branch),
            dump_expr(else_branch)
        ),
        ExprKind::Match { scrutinee, arms } => {
            let mut s = format!("(match {}", dump_expr(scrutinee));
            for a in arms {
                let g = a
                    .guard
                    .as_ref()
                    .map(|g| format!(" (guard {})", dump_expr(g)))
                    .unwrap_or_default();
                s.push_str(&format!(" (arm {}{} {})", dump_pat(&a.pat), g, dump_expr(&a.body)));
            }
            format!("{s})")
        }
        ExprKind::Block { stmts, tail } => {
            let mut s = "(block".to_string();
            for st in stmts {
                match st {
                    Stmt::Let { pat, ty, value, .. } => {
                        let t = ty.as_ref().map(|t| format!(" {}", dump_ty(t))).unwrap_or_default();
                        s.push_str(&format!(" (let {}{} {})", dump_pat(pat), t, dump_expr(value)));
                    }
                    Stmt::Expr(e) => s.push_str(&format!(" {}", dump_expr(e))),
                }
            }
            if let Some(t) = tail {
                s.push_str(&format!(" {}", dump_expr(t)));
            }
            format!("{s})")
        }
        ExprKind::Record { fields } => {
            let fs: Vec<_> =
                fields.iter().map(|(n, v)| format!("({} {})", n.name, dump_expr(v))).collect();
            format!("(rec {})", fs.join(" "))
        }
        ExprKind::Field { base, field } => format!("(field {} {})", dump_expr(base), field.name),
        ExprKind::List { items } => {
            let is: Vec<_> = items.iter().map(dump_expr).collect();
            if is.is_empty() { "(list)".to_string() } else { format!("(list {})", is.join(" ")) }
        }
        ExprKind::Perform { effect, op, resource, args } => {
            let res = resource.as_ref().map(|r| format!("[{}]", r.name)).unwrap_or_default();
            let mut s = format!("(perform {}.{}{}", effect.name, op.name, res);
            for a in args {
                s.push_str(&format!(" {}", dump_expr(a)));
            }
            format!("{s})")
        }
        ExprKind::Handle { body, clauses, return_clause } => {
            let mut s = format!("(handle {}", dump_expr(body));
            for c in clauses {
                let res = c.resource.as_ref().map(|r| format!("[{}]", r.name)).unwrap_or_default();
                let ps: Vec<_> = c.params.iter().map(|p| p.name.to_string()).collect();
                s.push_str(&format!(
                    " (clause {}.{}{} ({}) {})",
                    c.effect.name,
                    c.op.name,
                    res,
                    ps.join(" "),
                    dump_expr(&c.body)
                ));
            }
            if let Some(r) = return_clause {
                s.push_str(&format!(" (ret {} {})", r.binder.name, dump_expr(&r.body)));
            }
            format!("{s})")
        }
        ExprKind::WithCell { resource, init, binder, body } => format!(
            "(with_cell [{}] {} {} {})",
            resource.name,
            dump_expr(init),
            binder.name,
            dump_expr(body)
        ),
    }
}

fn dump_pat(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Var(i) => i.name.to_string(),
        PatternKind::Lit(l) => dump_lit(l),
        PatternKind::Ctor { name, args } => {
            let mut s = format!("(ctor {}", name.name);
            for a in args {
                s.push_str(&format!(" {}", dump_pat(a)));
            }
            format!("{s})")
        }
        PatternKind::Record { fields, rest } => {
            let mut fs: Vec<_> =
                fields.iter().map(|(n, p)| format!("({} {})", n.name, dump_pat(p))).collect();
            if *rest {
                fs.push("..".to_string());
            }
            format!("(prec {})", fs.join(" "))
        }
        PatternKind::List { items, rest } => {
            let mut is: Vec<_> = items.iter().map(dump_pat).collect();
            if let Some(r) = rest {
                is.push(format!(".. {}", dump_pat(r)));
            }
            format!("(plist {})", is.join(" "))
        }
    }
}
