//! The dumper at the bottom exists so a test asserts the *whole* shape of a parse rather than
//! poking at one field; a wrong nesting anywhere shows up as a string diff.

use crate::ast::*;
use crate::parser::{parse, parse_program, parse_recovering};
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use std::path::Path;

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
    let Item::Fn(f) = &m.items[0] else {
        panic!("expected a fn")
    };
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
    assert_eq!(
        dump("fn id<a>(x: a) -> a = x"),
        "(fn id <a> ((x a)) -> a x)"
    );
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
    assert_eq!(
        dump("type Pair = {a: Int, b: String}"),
        "(type Pair = {a: Int, b: String})"
    );
    assert_eq!(
        dump("type Option<a> = None | Some(a)"),
        "(type Option <a> = (| (v None) (v Some a)))"
    );
    assert_eq!(
        dump("type Color =\n  | Red\n  | Green\n  | Blue"),
        "(type Color = (| (v Red) (v Green) (v Blue)))"
    );
    assert_eq!(
        dump("type Wrap = Wrap(Int)"),
        "(type Wrap = (| (v Wrap Int)))"
    );
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
    assert_eq!(
        dump("test/nondet \"b\" { 1 }"),
        "(test/nondet \"b\" (block 1))"
    );
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
fn byte_literals_parse_in_expression_and_pattern_position() {
    assert_eq!(expr("b\"GET\""), "b[71, 69, 84]");
    assert_eq!(expr("b\"\""), "b[]");
    assert_eq!(expr("b\"\\r\\n\""), "b[13, 10]");
    assert_eq!(
        expr("match m { b\"GET\" -> 1, _ -> 0 }"),
        "(match m (arm b[71, 69, 84] 1) (arm _ 0))"
    );
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

/// operator precedence opened four levels between the comparisons and `++`. The
/// numbers in `bin_op` all moved; what these assert is the thing that made that
/// safe, which is the *order* — and the order of the bit operators is Rust's.
#[test]
fn the_bit_operators_sit_between_comparison_and_concatenation() {
    assert_eq!(expr("a | b ^ c"), "(| a (^ b c))");
    assert_eq!(expr("a ^ b & c"), "(^ a (& b c))");
    assert_eq!(expr("a & b << c"), "(& a (<< b c))");
    assert_eq!(expr("a << b ++ c"), "(<< a (++ b c))");
    assert_eq!(expr("a == b | c"), "(== a (| b c))");
    assert_eq!(expr("a | b == c"), "(== (| a b) c)");
    assert_eq!(expr("a && b | c"), "(&& a (| b c))");
    assert_eq!(expr("a & b + c"), "(& a (+ b c))");
    assert_eq!(expr("a & b & c"), "(& (& a b) c)");
    assert_eq!(expr("a | b | c"), "(| (| a b) c)");
    assert_eq!(expr("a << b << c"), "(<< (<< a b) c)");
    assert_eq!(expr("a >> b >> c"), "(>> (>> a b) c)");
}

/// A lambda's parameters end in a `|` and a parameter may carry a default, so the default's
/// expression must not read that closing pipe as bit-or and swallow the body.
///
/// The parser spike's differential is what caught this: making `|` infix changed the recovery on
/// `fixtures/35-err-named-arguments-and-updates.ply` from ten items to seven.
#[test]
fn a_lambda_parameter_default_does_not_swallow_the_closing_pipe() {
    // A default is refused (`E0120`) and parsed anyway, so the closing `|` is what the parser
    // meets next. Reading it as bit-or consumed the body and the errors multiplied.
    let d = errs("fn t() = (|x = 1| x)(2)");
    assert_eq!(
        d.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["E0120".to_string()],
        "the closing pipe was read as an operator: {d:#?}"
    );
    // And a `|` in the body is still bit-or, because the flag ends with the parameter list.
    assert_eq!(
        expr("(|a: Int| a | 1)(2)"),
        "(call (lam ((a Int)) (| a 1)) 2)"
    );
}

/// `>>` is not a token: it is two `Gt` the *expression* parser reads as
/// adjacent. So the join has to be exactly as tight as the spans are, and
/// `a > > b` — the same two tokens, written apart — stays the error it was.
#[test]
fn a_shift_is_adjacent_angle_brackets_and_a_space_still_separates_them() {
    assert_eq!(expr("a >> b"), "(>> a b)");
    assert_eq!(expr("a >>> b"), "(>>> a b)");
    assert_eq!(expr("a << b"), "(<< a b)");
    assert_eq!(expr("a > b"), "(> a b)");
    for src in ["fn t() = a > > b", "fn t() = a < < b", "fn t() = a > >> b"] {
        let d = errs(src);
        assert_eq!(d[0].code, codes::UNEXPECTED_TOKEN, "{src}");
    }
}

/// The reason `>>` is not lexed. A type's arguments close on `>` tokens that
/// have to stay separate, and the join lives in a parser the types never enter.
#[test]
fn joining_angle_brackets_leaves_nested_type_arguments_alone() {
    assert_eq!(
        dump("fn f(m: Map<Int, List<Int>>) -> Int = 1"),
        "(fn f ((m Map<Int, List<Int>>)) -> Int 1)"
    );
    assert_eq!(
        expr("{ let m: Map<Int, List<Int>> = q; m }"),
        "(block (let m Map<Int, List<Int>> q) m)"
    );
    assert_eq!(
        dump("fn f(x: List<List<List<Int>>>) -> Int = 1"),
        "(fn f ((x List<List<List<Int>>>)) -> Int 1)"
    );
    // The `>>=` that `expect_gt` splits, behind a second `>`: still three
    // separate closings and an `=`, and never a shift.
    assert_eq!(
        dump("fn f() -> Map<Int, List<Int>>= 1"),
        "(fn f () -> Map<Int, List<Int>> 1)"
    );
}

#[test]
fn bitwise_not_is_a_prefix_operator_like_the_other_two() {
    assert_eq!(expr("~a & b"), "(& (bnot a) b)");
    assert_eq!(expr("~a + b"), "(+ (bnot a) b)");
    assert_eq!(expr("~~a"), "(bnot (bnot a))");
    assert_eq!(expr("~f(x)"), "(bnot (call f x))");
    assert_eq!(expr("~(a | b)"), "(bnot (| a b))");
}

/// An infix `|` is read only where an operator can appear, and none of the
/// other three `|` — a lambda's parameters, a sum type's variants, a row's tail
/// — can reach that position. `||` still munches first, so a nullary lambda is
/// untouched.
#[test]
fn an_infix_pipe_disturbs_none_of_the_other_pipes() {
    assert_eq!(expr("|x| x | 1"), "(lam ((x _)) (| x 1))");
    assert_eq!(expr("|| 1 | 2"), "(lam () (| 1 2))");
    assert_eq!(
        dump("type Color = Red | Green"),
        "(type Color = (| (v Red) (v Green)))"
    );
    assert_eq!(dump("fn go<|e>() / e = 1"), "(fn go < | e> () / {| e} 1)");
    assert_eq!(
        dump("fn a() / {db.read[users] | e} = 1"),
        "(fn a () / {db.read[users] | e} 1)"
    );
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
    assert_eq!(
        expr("db.get[users](k).name"),
        "(field (perform db.get[users] k) name)"
    );
}

#[test]
fn lambdas_with_zero_one_and_many_parameters() {
    assert_eq!(expr("|| 1"), "(lam () 1)");
    assert_eq!(expr("|x| x + 1"), "(lam ((x _)) (+ x 1))");
    assert_eq!(expr("|x: Int, y| x"), "(lam ((x Int) (y _)) x)");
    assert_eq!(
        expr("f(|| 1, |x| x)"),
        "(call f (lam () 1) (lam ((x _)) x))"
    );
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
    assert_eq!(
        expr("if x == 1 { 2 } else { 3 }"),
        "(if (== x 1) (block 2) (block 3))"
    );
    assert_eq!(
        expr("if f(x) { 2 } else { 3 }"),
        "(if (call f x) (block 2) (block 3))"
    );
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
fn a_clause_may_bind_its_continuation() {
    assert_eq!(
        expr("handle f() with { amb.flip[coin]() resume k -> k(true) + k(false) }"),
        "(handle (call f) (clause amb.flip[coin] () resume k \
         (+ (call k true) (call k false))))"
    );
    assert_eq!(
        expr("handle f() with { st.put(v) resume k -> k(()) }"),
        "(handle (call f) (clause st.put (v) resume k (call k unit)))"
    );
}

/// `resume` is a keyword only between a clause's `)` and its `->`.
#[test]
fn resume_is_contextual_and_stays_an_ordinary_identifier_elsewhere() {
    assert!(crate::lexer::is_ident("resume"));
    assert_eq!(expr("resume(1)"), "(call resume 1)");
    assert_eq!(
        expr("handle f() with { st.get() -> resume }"),
        "(handle (call f) (clause st.get () resume))"
    );
    assert_eq!(
        expr("handle f() with { st.get(resume) -> resume }"),
        "(handle (call f) (clause st.get (resume) resume))"
    );
    let m = ok("fn resume(x: Int) -> Int = x");
    assert!(matches!(&m.items[0], Item::Fn(f) if f.name.name.as_str() == "resume"));
}

#[test]
fn a_clause_that_says_resume_without_a_binder_is_reported_there() {
    let ds = errs("fn f() = handle g() with { st.get() resume -> 1 }");
    assert_eq!(ds[0].code, codes::UNEXPECTED_TOKEN);
    assert!(
        ds[0].message.contains("a name to bind the continuation to"),
        "{}",
        ds[0].message
    );
}

#[test]
fn a_return_clause_is_recognised_by_the_binder_that_follows() {
    assert_eq!(
        expr("handle f() with { return x -> x }"),
        "(handle (call f) (ret x x))"
    );
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
fn with_region_brands_a_block() {
    assert_eq!(
        expr("with_region[r] { with_cell[r](0) { c -> cell_get(c) } }"),
        "(with_region [r] (block (with_cell [r] 0 c (call cell_get c))))"
    );
}

#[test]
fn regions_nest() {
    assert_eq!(
        expr("with_region[a] { with_region[b] { 1 } }"),
        "(with_region [a] (block (with_region [b] (block 1))))"
    );
}

/// Contextual exactly as `with_cell` is, so a program that already binds `with_region` as an
/// ordinary name is unaffected.
#[test]
fn with_region_is_only_special_before_a_bracket() {
    assert_eq!(expr("with_region"), "with_region");
    assert_eq!(expr("with_region(1)"), "(call with_region 1)");
    assert_eq!(expr("with_region + 1"), "(+ with_region 1)");
}

/// The body is a block, and `{` after `]` opens it even where a bare `{` would have opened the
/// enclosing construct's.
#[test]
fn a_region_body_is_a_block_in_no_brace_position() {
    assert_eq!(
        expr("if with_region[r] { true } { 1 } else { 2 }"),
        "(if (with_region [r] (block true)) (block 1) (block 2))"
    );
}

#[test]
fn simulate_takes_a_block_and_carries_no_seed() {
    assert_eq!(
        expr("simulate { let a = task.spawn(|| f()); task.join(a) }"),
        "(simulate (block (let a (perform task.spawn (lam () (call f)))) (perform task.join a)))"
    );
}

#[test]
fn simulate_is_only_special_before_a_brace() {
    assert_eq!(expr("simulate"), "simulate");
    assert_eq!(expr("simulate(1)"), "(call simulate 1)");
    assert_eq!(expr("simulate.now()"), "(perform simulate.now)");
}

/// Where a `{` opens the enclosing construct rather than an expression, `simulate` is an ordinary
/// name — otherwise a variable of that name would silently swallow the `if`'s branch.
#[test]
fn simulate_is_a_name_again_where_a_brace_cannot_start_an_expression() {
    assert_eq!(
        expr("if simulate { 1 } else { 2 }"),
        "(if simulate (block 1) (block 2))"
    );
    assert_eq!(
        expr("if (simulate { true }) { 1 } else { 2 }"),
        "(if (simulate (block true)) (block 1) (block 2))"
    );
}

#[test]
fn a_simulate_region_is_a_statement_without_a_semicolon() {
    assert_eq!(
        dump("fn t() { simulate { f() } 1 }"),
        "(fn t () (block (simulate (block (call f))) 1))"
    );
}

#[test]
fn a_handle_body_may_be_a_region_without_parentheses() {
    assert_eq!(
        expr("handle simulate { f() } with { return x -> x }"),
        "(handle (simulate (block (call f))) (ret x x))"
    );
}

#[test]
fn simulate_nests_inside_a_handler_and_a_region() {
    assert_eq!(
        expr("with_cell[users](0) { c -> simulate { handle f() with { db.get[users](k) -> k } } }"),
        "(with_cell [users] 0 c (simulate (block (handle (call f) \
         (clause db.get[users] (k) k)))))"
    );
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
    assert_eq!(
        expr("match f(x) { _ -> 1 }"),
        "(match (call f x) (arm _ 1))"
    );
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
    assert_eq!(
        dump("fn f(x: (Int)) -> Unit = x"),
        "(fn f ((x Int)) -> Unit x)"
    );
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
    assert_eq!(
        dump("fn f(x: a, y: A) -> Unit = x"),
        "(fn f ((x a) (y A)) -> Unit x)"
    );
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
    let ExprKind::Binary { rhs, .. } = &f.body.kind else {
        panic!()
    };
    assert_eq!(snippet(src, rhs.span), "2 * 3");
}

#[test]
fn a_perform_spans_from_the_effect_to_the_closing_paren() {
    let src = "fn f() = db.get[users](k)";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(snippet(src, f.body.span), "db.get[users](k)");
    let ExprKind::Perform {
        effect,
        op,
        resource,
        ..
    } = &f.body.kind
    else {
        panic!()
    };
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
    let ExprKind::Block { stmts, .. } = &f.body.kind else {
        panic!()
    };
    let Stmt::Let { span, .. } = &stmts[0] else {
        panic!()
    };
    assert_eq!(snippet(src, *span), "let x = 1;");
}

#[test]
fn a_synthesized_else_branch_gets_an_empty_span_at_the_end_of_the_then_block() {
    let src = "fn f() = if a { 1 }";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let ExprKind::If {
        then_branch,
        else_branch,
        ..
    } = &f.body.kind
    else {
        panic!()
    };
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
        "fn f() = with_region[r]",
        "fn f() = with_region[] { 1 }",
        "fn f() = with_region[r] 1",
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
            let span = d
                .primary_span()
                .unwrap_or_else(|| panic!("no span for {src:?}"));
            assert!(!span.is_dummy(), "dummy span for {src:?}");
            assert!(span.end as usize <= src.len(), "span past end for {src:?}");
            assert!(span.start <= span.end, "inverted span for {src:?}");
        }
    }
}

#[test]
fn a_bad_item_does_not_stop_later_items_from_parsing() {
    let (module, diags) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "fn a() = ;\nfn b() = 1\nfn c() = 2",
    );
    assert_eq!(diags.len(), 1);
    assert_eq!(dump_module(&module), "(fn b () 1)\n(fn c () 2)");
}

#[test]
fn several_independent_errors_are_reported_in_one_run() {
    let (module, diags) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "fn a() = ;\ntype T = ;\neffect e { bogus }\ntest \"t\" { 1 }",
    );
    assert!(
        diags.len() >= 3,
        "expected several diagnostics, got {diags:#?}"
    );
    assert_eq!(
        module.items.len(),
        1,
        "the well-formed test should still parse"
    );
    assert!(matches!(&module.items[0], Item::Test(_)));
}

#[test]
fn recovery_skips_over_braces_inside_the_broken_item() {
    let (module, diags) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "fn a() { let = 1; }\nfn b() = 3",
    );
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
    let opened = ds[0]
        .labels
        .iter()
        .find(|l| !l.primary)
        .expect("a secondary label");
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
    assert!(
        ds[0].message.contains("`read` or `write`"),
        "{}",
        ds[0].message
    );
}

#[test]
fn a_tuple_type_is_rejected_with_a_note() {
    let ds = errs("fn f(x: (Int, Bool)) = x");
    assert!(
        ds[0].notes.iter().any(|n| n.contains("record")),
        "{:#?}",
        ds[0]
    );
}

#[test]
fn a_duplicate_return_clause_is_reported_once() {
    let ds = errs("fn f() = handle g() with { return x -> x, return y -> y }");
    assert_eq!(ds.len(), 1);
    assert!(
        ds[0].message.contains("only one `return`"),
        "{}",
        ds[0].message
    );
}

#[test]
fn lexer_diagnostics_reach_the_parse_result() {
    let ds = errs("fn f() = \"oops");
    assert!(
        ds.iter().any(|d| d.code == codes::UNTERMINATED_STRING),
        "{ds:#?}"
    );
}

#[test]
fn parsing_terminates_on_pathological_input() {
    for src in [
        "{{{{{{{{",
        "((((((((",
        "fn f(((((",
        "|||||",
        "....",
        "<<<<",
        "-----",
        "fn fn fn",
        "import import import",
        "import",
        "import.import",
        "import a as as as",
        "import a ((((",
        "pub pub pub",
        "pub import a",
        "::::::",
        "a::::b",
    ] {
        let (_, diags) = parse_recovering(SRC, ModuleName::anonymous(), src);
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

    let ty = format!(
        "fn f(x: {}Int{}) = x",
        "List<".repeat(20_000),
        ">".repeat(20_000)
    );
    assert!(
        errs(&ty)
            .iter()
            .any(|d| d.message.contains("nested too deeply"))
    );

    let pat = format!(
        "fn f() = match v {{ {}x{} -> 1 }}",
        "Some(".repeat(20_000),
        ")".repeat(20_000)
    );
    assert!(
        errs(&pat)
            .iter()
            .any(|d| d.message.contains("nested too deeply"))
    );
}

#[test]
fn a_long_flat_expression_does_not_hit_the_depth_limit() {
    let terms: Vec<String> = (0..5_000).map(|i| i.to_string()).collect();
    ok(&format!("fn f() = {}", terms.join(" + ")));
}

#[test]
fn token_soup_never_panics_or_hangs() {
    const ALPHABET: [&str; 32] = [
        "fn", "type", "effect", "nondet", "test", "let", "if", "else", "match", "handle", "with",
        "x", "X", "\"s\"", "1", "(", ")", "{", "}", "[", "]", "|", "||", ",", ";", ".", "->", "=",
        "import", "pub", "as", "::",
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
        let src: Vec<&str> = (0..len)
            .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
            .collect();
        let src = src.join(" ");
        let (_, diags) = parse_recovering(SRC, ModuleName::anonymous(), &src);
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
fn derive_item_parses() {
    assert_eq!(
        dump("type Order = {id: Int}\nderive json for Order"),
        "(type Order = {id: Int})\n(derive json Order)"
    );
}

#[test]
fn derive_is_contextual_and_a_function_may_still_be_named_derive() {
    assert_eq!(dump("fn derive(x) = x"), "(fn derive ((x _)) x)");
}

#[test]
fn a_reuse_fn_parses_before_and_after_pub_and_keeps_its_marker() {
    assert_eq!(dump("reuse fn f(xs) = xs"), "(reuse fn f ((xs _)) xs)");
    assert_eq!(
        dump("pub reuse fn f(xs) = xs"),
        "(pub reuse fn f ((xs _)) xs)"
    );
    let m = ok("reuse fn f(xs) = xs");
    let Item::Fn(f) = &m.items[0] else {
        panic!("expected a fn")
    };
    assert!(
        f.reuse.is_some(),
        "the marker's span is kept for the diagnostic"
    );
}

#[test]
fn reuse_is_contextual_and_stays_a_name_everywhere_else() {
    assert_eq!(dump("fn reuse(x) = x"), "(fn reuse ((x _)) x)");
    assert_eq!(dump("fn f(reuse) = reuse"), "(fn f ((reuse _)) reuse)");
    ok("fn f(reuse: Int) -> Int = { let x = reuse; x }");
}

#[test]
fn an_unknown_deriver_is_reported_with_the_whole_list() {
    let d = errs("derive toml for Order");
    assert_eq!(d[0].code, codes::UNKNOWN_DERIVER);
    assert!(d[0].notes.iter().any(|n| n.contains("`json`")));
}

#[test]
fn a_derive_cannot_be_pub() {
    let d = errs("pub derive json for Order");
    assert_eq!(d[0].code, codes::UNEXPECTED_TOKEN);
}

#[test]
fn where_clauses_sit_between_the_row_and_the_spec() {
    assert_eq!(
        dump(
            "fn respond<a>(v: a, c: Codec<a>) -> Response / {} \
             where derivable(json, a), derivable(ord, a) requires true = v"
        ),
        "(fn respond <a> ((v a) (c Codec<a>)) -> Response / {} \
         (derivable json a) (derivable ord a) (requires true) v)"
    );
}

#[test]
fn a_where_clause_naming_something_that_is_not_a_deriver_is_reported() {
    let d = errs("fn f<a>(x: a) -> a where derivable(toml, a) = x");
    assert_eq!(d[0].code, codes::UNKNOWN_DERIVER);
}

#[test]
fn reformatting_does_not_change_the_parse() {
    let dense = "fn f(x:Int)->Int/{db.read[users]}={let y=x+1;y*2}";
    let loose = "fn f(x: Int) -> Int / { db.read[users] } = {\n  // a comment\n  let y = x + 1;\n  y * 2\n}";
    assert_eq!(dump(dense), dump(loose));
}

fn dump_module(m: &Module) -> String {
    m.imports
        .iter()
        .map(dump_import)
        .chain(m.items.iter().map(dump_item))
        .collect::<Vec<_>>()
        .join("\n")
}

fn dump_import(i: &ImportDecl) -> String {
    let path: Vec<_> = i.path.iter().map(|s| s.name.to_string()).collect();
    match &i.kind {
        ImportKind::Module => format!("(import {})", path.join(".")),
        ImportKind::Alias(a) => format!("(import {} as {})", path.join("."), a.name),
        ImportKind::Names(ns) => {
            let ns: Vec<_> = ns.iter().map(|n| n.name.to_string()).collect();
            format!("(import {} ({}))", path.join("."), ns.join(", "))
        }
    }
}

fn dump_item(i: &Item) -> String {
    let vis = if i.visibility().is_public() {
        "pub "
    } else {
        ""
    };
    let body = dump_item_body(i);
    format!("({vis}{}", &body[1..])
}

fn dump_item_body(i: &Item) -> String {
    match i {
        Item::Fn(f) => {
            let reuse = if f.reuse.is_some() { "reuse " } else { "" };
            let mut s = format!("({reuse}fn {}", f.name.name);
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
            for c in &f.constraints {
                s.push_str(&format!(" (derivable {} {})", c.deriver, c.param.name));
            }
            for clause in &f.spec {
                s.push_str(&format!(
                    " ({} {})",
                    clause.kind.as_str(),
                    dump_expr(&clause.expr)
                ));
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
        Item::Law(l) => {
            let mut s = format!("(law {:?}", l.name);
            if !l.binders.is_empty() {
                let bs: Vec<_> = l
                    .binders
                    .iter()
                    .map(|b| format!("({} {})", b.name.name, dump_ty(&b.ty)))
                    .collect();
                s.push_str(&format!(" (forall {})", bs.join(" ")));
            }
            if let Some(g) = &l.guard {
                s.push_str(&format!(" (where {})", dump_expr(g)));
            }
            format!("{s} {})", dump_expr(&l.body))
        }
        Item::Derive(d) => format!("(derive {} {})", d.deriver, d.target.name),
        // The expansion, not the members as written: the expansion is what every row naming this
        // set was given, and the members are a way of spelling it.
        Item::EffectSet(d) => format!("(effect-set {} {})", d.name.name, dump_atoms(&d.expansion)),
    }
}

fn dump_atoms(atoms: &[AtomExpr]) -> String {
    let atoms: Vec<_> = atoms
        .iter()
        .map(|a| {
            let res = a
                .resource
                .as_ref()
                .map(|r| format!("[{}]", r.name))
                .unwrap_or_default();
            format!("{}.{}{}", a.effect, a.mode.as_str(), res)
        })
        .collect();
    format!("{{{}}}", atoms.join(", "))
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
        TypeExpr::Con { name, args, .. } if args.is_empty() => name.to_string(),
        TypeExpr::Con { name, args, .. } => {
            let args: Vec<_> = args.iter().map(dump_ty).collect();
            format!("{}<{}>", name, args.join(", "))
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => {
            let ps: Vec<_> = params.iter().map(dump_ty).collect();
            let eff = effects
                .as_ref()
                .map(|r| format!(" / {}", dump_row(r)))
                .unwrap_or_default();
            format!("(fn ({}) -> {}{})", ps.join(", "), dump_ty(ret), eff)
        }
        TypeExpr::Record { fields, .. } => {
            let fs: Vec<_> = fields
                .iter()
                .map(|(n, t)| format!("{}: {}", n.name, dump_ty(t)))
                .collect();
            format!("{{{}}}", fs.join(", "))
        }
        TypeExpr::Unit { .. } => "()".to_string(),
    }
}

fn dump_row(r: &RowExpr) -> String {
    let atoms = dump_atoms(&r.atoms);
    let atoms = &atoms[1..atoms.len() - 1];
    match &r.tail {
        None => format!("{{{atoms}}}"),
        Some(t) if r.atoms.is_empty() => format!("{{| {}}}", t.name),
        Some(t) => format!("{{{atoms} | {}}}", t.name),
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
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::Ushr => ">>>",
    }
}

fn dump_lit(l: &Lit) -> String {
    match l {
        Lit::Int(v) => v.to_string(),
        Lit::Bool(b) => b.to_string(),
        Lit::Str(s) => format!("{s:?}"),
        Lit::Bytes(b) => format!("b{b:?}"),
        Lit::Float(v) => format!("{v:?}f"),
        Lit::Decimal { mantissa, scale } => format!("{mantissa}e-{scale}m"),
        Lit::Unit => "unit".to_string(),
    }
}

fn dump_expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Lit(l) => dump_lit(l),
        ExprKind::Var(q) => q.to_string(),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", op_str(*op), dump_expr(lhs), dump_expr(rhs))
        }
        ExprKind::Unary { op, operand } => {
            let n = match op {
                UnOp::Neg => "neg",
                UnOp::Not => "not",
                UnOp::BitNot => "bnot",
            };
            format!("({n} {})", dump_expr(operand))
        }
        ExprKind::Lambda { params, body } => {
            format!("(lam ({}) {})", dump_params(params), dump_expr(body))
        }
        ExprKind::App { func, args, .. } => {
            let mut s = format!("(call {}", dump_expr(func));
            for a in args {
                s.push_str(&format!(" {}", dump_expr(a)));
            }
            format!("{s})")
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => format!(
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
                s.push_str(&format!(
                    " (arm {}{} {})",
                    dump_pat(&a.pat),
                    g,
                    dump_expr(&a.body)
                ));
            }
            format!("{s})")
        }
        ExprKind::Block { stmts, tail } => {
            let mut s = "(block".to_string();
            for st in stmts {
                match st {
                    Stmt::Let { pat, ty, value, .. } => {
                        let t = ty
                            .as_ref()
                            .map(|t| format!(" {}", dump_ty(t)))
                            .unwrap_or_default();
                        s.push_str(&format!(
                            " (let {}{} {})",
                            dump_pat(pat),
                            t,
                            dump_expr(value)
                        ));
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
            let fs: Vec<_> = fields
                .iter()
                .map(|(n, v)| format!("({} {})", n.name, dump_expr(v)))
                .collect();
            format!("(rec {})", fs.join(" "))
        }
        // Only reachable from `dump_expr` on a tree taken before `record_update::expand` ran, which
        // the parser's own tests do on purpose to check what was parsed rather than what it became.
        ExprKind::RecordUpdate { base, fields } => {
            let fs: Vec<_> = fields
                .iter()
                .map(|(n, v)| format!("({} {})", n.name, dump_expr(v)))
                .collect();
            format!("(update {} {})", dump_expr(base), fs.join(" "))
        }
        ExprKind::Field { base, field } => format!("(field {} {})", dump_expr(base), field.name),
        // Only reachable on a tree taken before `try_op::expand` ran, which the parser's own tests
        // do on purpose.
        ExprKind::Try { operand } => format!("(try {})", dump_expr(operand)),
        ExprKind::List { items } => {
            let is: Vec<_> = items.iter().map(dump_expr).collect();
            if is.is_empty() {
                "(list)".to_string()
            } else {
                format!("(list {})", is.join(" "))
            }
        }
        ExprKind::Perform {
            effect,
            op,
            resource,
            args,
        } => {
            let res = resource
                .as_ref()
                .map(|r| format!("[{}]", r.name))
                .unwrap_or_default();
            let mut s = format!("(perform {}.{}{}", effect, op.name, res);
            for a in args {
                s.push_str(&format!(" {}", dump_expr(a)));
            }
            format!("{s})")
        }
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            let mut s = format!("(handle {}", dump_expr(body));
            for c in clauses {
                let res = c
                    .resource
                    .as_ref()
                    .map(|r| format!("[{}]", r.name))
                    .unwrap_or_default();
                let ps: Vec<_> = c.params.iter().map(|p| p.name.to_string()).collect();
                let k = c
                    .resume
                    .as_ref()
                    .map(|k| format!(" resume {}", k.name))
                    .unwrap_or_default();
                s.push_str(&format!(
                    " (clause {}.{}{} ({}){} {})",
                    c.effect,
                    c.op.name,
                    res,
                    ps.join(" "),
                    k,
                    dump_expr(&c.body)
                ));
            }
            if let Some(r) = return_clause {
                s.push_str(&format!(" (ret {} {})", r.binder.name, dump_expr(&r.body)));
            }
            format!("{s})")
        }
        ExprKind::WithCell {
            resource,
            init,
            binder,
            body,
        } => format!(
            "(with_cell [{}] {} {} {})",
            resource.name,
            dump_expr(init),
            binder.name,
            dump_expr(body)
        ),
        ExprKind::WithRegion { region, body } => {
            format!("(with_region [{}] {})", region.name, dump_expr(body))
        }
        ExprKind::Simulate { body } => format!("(simulate {})", dump_expr(body)),
    }
}

fn dump_pat(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Var(i) => i.name.to_string(),
        PatternKind::Lit(l) => dump_lit(l),
        PatternKind::Ctor { name, args } => {
            let mut s = format!("(ctor {}", name);
            for a in args {
                s.push_str(&format!(" {}", dump_pat(a)));
            }
            format!("{s})")
        }
        PatternKind::Record { fields, rest } => {
            let mut fs: Vec<_> = fields
                .iter()
                .map(|(n, p)| format!("({} {})", n.name, dump_pat(p)))
                .collect();
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

// --- Modules ----------------------------------------------------------------

#[test]
fn a_module_name_is_its_path_with_separators_turned_into_dots() {
    let name = ModuleName::from_relative_path(Path::new("store/orders.ply")).unwrap();
    assert_eq!(name.as_str(), "store.orders");
    assert_eq!(name.default_binder().as_str(), "orders");
    assert_eq!(
        name.qualify(&Symbol::new("place")).as_str(),
        "store.orders.place"
    );
    assert_eq!(name.segments().collect::<Vec<_>>(), ["store", "orders"]);
}

#[test]
fn a_top_level_file_is_a_single_segment_module() {
    let name = ModuleName::from_relative_path(Path::new("ledger.ply")).unwrap();
    assert_eq!(name.as_str(), "ledger");
    assert_eq!(name.default_binder().as_str(), "ledger");
}

#[test]
fn a_path_segment_that_is_not_an_identifier_is_a_diagnostic() {
    for bad in ["my-crate/a.ply", "a/b c.ply", "a.b.ply", "9lives/x.ply"] {
        let err = ModuleName::from_relative_path(Path::new(bad))
            .expect_err(&format!("expected `{bad}` to be rejected"));
        assert_eq!(err.code, codes::INVALID_MODULE_PATH);
        assert!(
            !err.notes.is_empty(),
            "{bad} should say what to do about it"
        );
    }
}

#[test]
fn the_anonymous_module_leaves_names_bare() {
    let anon = ModuleName::anonymous();
    assert!(anon.is_anonymous());
    assert_eq!(anon.qualify(&Symbol::new("place")).as_str(), "place");
}

#[test]
fn a_qualified_name_never_collides_with_one_a_module_could_declare() {
    // `.` cannot be lexed inside an identifier, so no source-writable name can equal a qualified
    // one.
    let qualified = ModuleName::from_dotted("store.orders").qualify(&Symbol::new("place"));
    assert!(!crate::lexer::is_ident(qualified.as_str()));
}

#[test]
fn items_are_private_until_marked_pub() {
    let m = ok(
        "fn a() = 1\npub fn b() = 2\ntype T = Int\npub type U = Int\n\
                effect e { read r() -> Int }\npub effect f { read r() -> Int }",
    );
    let vis: Vec<bool> = m.items.iter().map(|i| i.visibility().is_public()).collect();
    assert_eq!(vis, [false, true, false, true, false, true]);
}

#[test]
fn pub_survives_on_the_definition_itself() {
    let m = ok("pub fn b() = 2");
    let Item::Fn(f) = &m.items[0] else {
        panic!("expected a fn")
    };
    assert_eq!(f.vis, Visibility::Public);
    assert_eq!(m.name, ModuleName::anonymous());
    assert!(m.imports.is_empty());
}

#[test]
fn a_test_cannot_be_pub() {
    let diags = errs("pub test \"t\" { 1 }");
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("cannot be `pub`"));
}

#[test]
fn pub_does_not_stop_error_recovery_from_finding_the_next_item() {
    let (module, diags) =
        parse_recovering(SRC, ModuleName::anonymous(), "fn a() = ;\npub fn b() = 1");
    assert_eq!(diags.len(), 1);
    assert_eq!(module.items.len(), 1);
}

#[test]
fn a_double_colon_lexes_as_one_token_rather_than_two_colons() {
    use crate::lexer::{TokenKind, lex};
    let (tokens, diags) = lex(SRC, "a::b : c");
    assert!(diags.is_empty());
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert_eq!(kinds[1], TokenKind::ColonColon);
    assert_eq!(kinds[3], TokenKind::Colon);
}

#[test]
fn a_qualified_name_prints_the_way_it_is_written() {
    let sp = Span::new(SRC, 0, 1);
    let bare = QName::bare(Ident::new("place", sp));
    assert_eq!(bare.to_string(), "place");
    assert!(bare.is_bare());

    let qualified = QName::qualified(Ident::new("orders", sp), Ident::new("place", sp));
    assert_eq!(qualified.to_string(), "orders::place");
    assert!(!qualified.is_bare());
    assert_eq!(qualified.symbol().as_str(), "place");
}

#[test]
fn imports_parse_in_all_three_forms() {
    assert_eq!(dump("import store.orders"), "(import store.orders)");
    assert_eq!(
        dump("import store.orders as ord"),
        "(import store.orders as ord)"
    );
    assert_eq!(
        dump("import store.orders (place, cancel)"),
        "(import store.orders (place, cancel))"
    );
    assert_eq!(dump("import ledger"), "(import ledger)");
    assert_eq!(dump("import a.b.c.d"), "(import a.b.c.d)");
    assert_eq!(dump("import a (one,)"), "(import a (one))");
}

#[test]
fn an_import_binds_its_last_segment_unless_aliased_or_selective() {
    let m = ok("import store.orders\nimport store.orders as ord\nimport store.orders (place)");
    assert_eq!(m.imports.len(), 3);
    for i in &m.imports {
        assert_eq!(i.module_name().as_str(), "store.orders");
    }
    assert_eq!(m.imports[0].binder().unwrap().as_str(), "orders");
    assert_eq!(m.imports[1].binder().unwrap().as_str(), "ord");
    assert!(
        m.imports[2].binder().is_none(),
        "a selective import binds no module binder"
    );
}

#[test]
fn every_part_of_an_import_carries_a_real_span() {
    let src = "import store.orders as ord";
    let m = ok(src);
    let i = &m.imports[0];
    assert_eq!(snippet(src, i.span), src);
    assert_eq!(snippet(src, i.path_span()), "store.orders");
    assert_eq!(snippet(src, i.binder_span()), "ord");
    assert_eq!(snippet(src, i.path[0].span), "store");
    assert_eq!(snippet(src, i.path[1].span), "orders");

    let src = "import store.orders (place, cancel)";
    let m = ok(src);
    let i = &m.imports[0];
    assert_eq!(snippet(src, i.span), src);
    assert_eq!(
        snippet(src, i.binder_span()),
        src,
        "a selective import points at the whole decl"
    );
    let ImportKind::Names(names) = &i.kind else {
        panic!("expected a selective import")
    };
    assert_eq!(snippet(src, names[0].span), "place");
    assert_eq!(snippet(src, names[1].span), "cancel");

    let src = "import store.orders";
    let m = ok(src);
    assert_eq!(snippet(src, m.imports[0].binder_span()), "orders");
}

#[test]
fn imports_come_before_items_in_the_tree_they_produce() {
    let m = ok("import a\nimport b as c\npub fn f() = 1");
    assert_eq!(m.imports.len(), 2);
    assert_eq!(m.items.len(), 1);
    assert_eq!(
        dump_module(&m),
        "(import a)\n(import b as c)\n(pub fn f () 1)"
    );
}

#[test]
fn as_is_contextual_and_stays_usable_as_an_identifier() {
    assert_eq!(dump("fn f(as: Int) = as"), "(fn f ((as Int)) as)");
    assert_eq!(dump("import a as as"), "(import a as as)");
}

#[test]
fn pub_applies_to_every_item_kind_that_can_carry_it() {
    assert_eq!(dump("pub fn f() = 1"), "(pub fn f () 1)");
    assert_eq!(dump("pub type T = Int"), "(pub type T = Int)");
    assert_eq!(
        dump("pub effect db { read get() -> Int }"),
        "(pub effect db (op read get () -> Int))"
    );
    assert_eq!(
        dump("pub nondet effect clock { read now() -> Int }"),
        "(pub nondet effect clock (op read now () -> Int))"
    );
    assert_eq!(dump("type T = Int"), "(type T = Int)");
}

#[test]
fn malformed_imports_are_diagnostics_with_a_code_and_a_real_span() {
    for src in [
        "import",
        "import .",
        "import 1",
        "import a.",
        "import a.1",
        "import a as",
        "import a as 1",
        "import a as b (c)",
        "import a (b) as c",
        "import a (",
        "import a ()",
        "import a (b",
        "import a (1)",
        "import a (b c)",
    ] {
        let ds = errs(src);
        assert!(!ds.is_empty(), "expected a diagnostic for {src:?}");
        for d in &ds {
            assert!(!d.code.is_empty(), "empty code for {src:?}");
            let span = d
                .primary_span()
                .unwrap_or_else(|| panic!("no span for {src:?}"));
            assert!(!span.is_dummy(), "dummy span for {src:?}");
            assert!(span.end as usize <= src.len(), "span past end for {src:?}");
            assert!(span.start <= span.end, "inverted span for {src:?}");
        }
    }
}

#[test]
fn an_import_may_rename_or_select_but_not_both() {
    for src in ["import a as b (c)", "import a (c) as b"] {
        let ds = errs(src);
        assert!(
            ds.iter().any(|d| d.message.contains("not both")),
            "expected the `as`-plus-list diagnostic for {src:?}: {ds:#?}"
        );
    }
}

#[test]
fn an_import_that_selects_nothing_says_what_to_write_instead() {
    let ds = errs("import a ()");
    assert!(
        ds[0].message.contains("selects no names"),
        "{}",
        ds[0].message
    );
    assert!(
        ds[0].notes.iter().any(|n| n.contains("bind the module")),
        "{:#?}",
        ds[0]
    );
}

#[test]
fn a_malformed_import_does_not_stop_the_rest_of_the_file_from_parsing() {
    let (m, ds) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "import a as\nimport b\nfn f() = 1\nfn g() = 2",
    );
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert_eq!(dump_module(&m), "(import b)\n(fn f () 1)\n(fn g () 2)");
}

#[test]
fn an_import_after_a_definition_is_reported_and_still_recorded() {
    let src = "import a\nfn f() = 1\nimport b\nfn g() = 2";
    let (m, ds) = parse_recovering(SRC, ModuleName::anonymous(), src);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(
        ds[0].message.contains("before every definition"),
        "{}",
        ds[0].message
    );
    assert_eq!(snippet(src, ds[0].primary_span().unwrap()), "import");

    let first = ds[0]
        .labels
        .iter()
        .find(|l| !l.primary)
        .expect("a secondary label");
    assert_eq!(snippet(src, first.span), "fn f() = 1");
    assert!(
        !ds[0].notes.is_empty(),
        "it should say where to move the import"
    );

    assert_eq!(m.imports.len(), 2, "the misplaced import is still recorded");
    assert_eq!(m.items.len(), 2);
}

#[test]
fn each_out_of_order_import_is_reported_separately() {
    let (_, ds) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "fn f() = 1\nimport a\nimport b\nfn g() = 2",
    );
    assert_eq!(ds.len(), 2, "{ds:#?}");
}

#[test]
fn a_broken_item_before_an_import_does_not_swallow_it() {
    let (m, ds) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "fn a() = ;\nimport b\nfn c() = 1",
    );
    assert!(
        ds.len() >= 2,
        "expected both the bad body and the misplaced import: {ds:#?}"
    );
    assert_eq!(m.imports.len(), 1);
    assert_eq!(m.items.len(), 1);
}

#[test]
fn import_errors_and_item_errors_are_reported_in_one_run() {
    let (m, ds) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "import a as\nimport b ()\nimport c\nfn f() = ;\nimport d\npub fn g() = 1",
    );
    assert!(
        ds.len() >= 4,
        "expected four independent errors, got {ds:#?}"
    );
    assert_eq!(dump_module(&m), "(import c)\n(import d)\n(pub fn g () 1)");
}

#[test]
fn a_duplicate_import_binding_parses_so_resolution_can_reject_it() {
    let m = ok("import a.orders\nimport b.orders");
    assert_eq!(m.imports.len(), 2);
    assert_eq!(m.imports[0].binder(), m.imports[1].binder());
    assert_ne!(m.imports[0].binder_span(), m.imports[1].binder_span());

    let m = ok("import orders\nimport store.placement as orders");
    assert_eq!(m.imports[0].binder().unwrap().as_str(), "orders");
    assert_eq!(m.imports[1].binder().unwrap().as_str(), "orders");

    let m = ok("import a (place)\nimport b (place)");
    let (ImportKind::Names(x), ImportKind::Names(y)) = (&m.imports[0].kind, &m.imports[1].kind)
    else {
        panic!("expected two selective imports")
    };
    assert_eq!(x[0].name, y[0].name);
    assert_ne!(x[0].span, y[0].span);
}

#[test]
fn qualified_references_parse_in_every_position() {
    assert_eq!(expr("orders::place(x)"), "(call orders::place x)");
    assert_eq!(
        expr("store::db.get[users](k)"),
        "(perform store::db.get[users] k)"
    );
    assert_eq!(expr("store::clock.now()"), "(perform store::clock.now)");
    assert_eq!(
        expr("match v { orders::Placed(x) -> x, orders::Cancelled -> 0 }"),
        "(match v (arm (ctor orders::Placed x) x) (arm (ctor orders::Cancelled) 0))"
    );
    assert_eq!(
        dump("fn f(x: orders::Order) = x"),
        "(fn f ((x orders::Order)) x)"
    );
    assert_eq!(
        dump("fn f(x: orders::Slot<Int>) -> orders::Order = x"),
        "(fn f ((x orders::Slot<Int>)) -> orders::Order x)"
    );
    assert_eq!(
        dump("fn f() / {store::db.read[users], clock.write} = 1"),
        "(fn f () / {store::db.read[users], clock.write} 1)"
    );
    assert_eq!(
        expr("handle f() with { store::db.get[users](k) -> k, return x -> x }"),
        "(handle (call f) (clause store::db.get[users] (k) k) (ret x x))"
    );
}

#[test]
fn a_qualified_name_is_neither_a_field_access_nor_a_perform() {
    assert_eq!(expr("orders::place"), "orders::place");
    assert_eq!(expr("orders.place"), "(field orders place)");
    assert_eq!(expr("orders::rec.field"), "(field orders::rec field)");
    assert_eq!(expr("orders::f(x).g"), "(field (call orders::f x) g)");
}

#[test]
fn a_qualified_reference_spans_the_binder_through_the_name() {
    let src = "fn f() = orders::place(x)";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let ExprKind::App { func, .. } = &f.body.kind else {
        panic!("expected a call")
    };
    let ExprKind::Var(q) = &func.kind else {
        panic!("expected a qualified name")
    };
    assert_eq!(snippet(src, q.span), "orders::place");
    assert_eq!(snippet(src, q.module.as_ref().unwrap().span), "orders");
    assert_eq!(snippet(src, q.name.span), "place");

    let src = "fn f() = store::db.get[users](k)";
    let m = ok(src);
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let ExprKind::Perform { effect, .. } = &f.body.kind else {
        panic!("expected a perform")
    };
    assert_eq!(snippet(src, effect.span), "store::db");
}

#[test]
fn a_module_path_in_a_reference_is_a_single_binder() {
    let ds = errs("fn f() = a::b::c");
    assert!(
        ds.iter().any(|d| d.message.contains("at most one `::`")),
        "{ds:#?}"
    );
    assert!(
        errs("fn f(x: a::b::C) = x")
            .iter()
            .any(|d| d.message.contains("at most one `::`"))
    );
    assert!(
        errs("fn f() = match v { a::b::C -> 1 }")
            .iter()
            .any(|d| d.message.contains("at most one `::`"))
    );
}

#[test]
fn a_dangling_double_colon_is_a_diagnostic_with_a_real_span() {
    for src in [
        "fn f() = a::",
        "fn f() = ::a",
        "fn f(x: a::) = x",
        "fn f() / {a::.read} = 1",
    ] {
        let ds = errs(src);
        assert!(!ds.is_empty(), "expected a diagnostic for {src:?}");
        for d in &ds {
            let span = d
                .primary_span()
                .unwrap_or_else(|| panic!("no span for {src:?}"));
            assert!(!span.is_dummy(), "dummy span for {src:?}");
            assert!(span.end as usize <= src.len(), "span past end for {src:?}");
        }
    }
}

#[test]
fn spec_clauses_parse_in_any_order_and_any_number() {
    assert_eq!(
        dump(
            "fn withdraw(a: Account, n: Int) -> Account\n\
               requires n > 0\n\
               ensures result.balance == a.balance - n\n\
               requires n <= a.balance\n\
               ensures result.id == a.id\n\
             = a"
        ),
        "(fn withdraw ((a Account) (n Int)) -> Account \
         (requires (> n 0)) \
         (ensures (== (field result balance) (- (field a balance) n))) \
         (requires (<= n (field a balance))) \
         (ensures (== (field result id) (field a id))) a)"
    );
}

#[test]
fn spec_clauses_precede_a_block_body_too() {
    assert_eq!(
        dump("fn f(n: Int) -> Int requires n > 0 { n }"),
        "(fn f ((n Int)) -> Int (requires (> n 0)) (block n))"
    );
}

/// The `no_brace` flag: the `{` closes the clause and opens the body.
#[test]
fn a_clause_is_never_followed_by_a_record_literal() {
    assert_eq!(
        dump("fn f(x: Int) -> Int ensures p(x) { x }"),
        "(fn f ((x Int)) -> Int (ensures (call p x)) (block x))"
    );
    assert_eq!(
        dump("fn f(x: Int) -> Int ensures p({a: x}) = x"),
        "(fn f ((x Int)) -> Int (ensures (call p (rec (a x)))) x)"
    );
}

#[test]
fn a_row_annotation_still_precedes_the_clauses() {
    assert_eq!(
        dump("fn f() -> Bool / {db.read[users]} ensures result = true"),
        "(fn f () -> Bool / {db.read[users]} (ensures result) true)"
    );
}

#[test]
fn every_spec_word_is_contextual() {
    for src in [
        "fn requires(x: Int) = x",
        "fn ensures(x: Int) = x",
        "fn law(x: Int) = x",
        "fn forall(x: Int) = x",
        "fn result(x: Int) = x",
        "fn f() = { let requires = 1; let ensures = 2; let law = 3; requires + ensures + law }",
        "fn f() = { let forall = 1; let where = 2; forall + where }",
        "fn f(result: Int) -> Int = result",
        "fn f(law: Int, forall: Int) -> Int = law + forall",
    ] {
        ok(src);
    }
}

#[test]
fn a_law_carries_binders_a_guard_and_a_body() {
    assert_eq!(
        dump(
            "law \"credit and debit cancel\"\n\
               forall (a: Account, n: Int) where n > 0 && n <= a.balance {\n\
                 credited(debited(a, n), n) == a\n\
               }"
        ),
        "(law \"credit and debit cancel\" (forall (a Account) (n Int)) \
         (where (&& (> n 0) (<= n (field a balance)))) \
         (block (== (call credited (call debited a n) n) a)))"
    );
}

#[test]
fn a_law_may_have_no_guard_and_no_binders() {
    assert_eq!(
        dump(
            "law \"reverse is an involution\" forall (xs: List<Int>) { reverse(reverse(xs)) == xs }"
        ),
        "(law \"reverse is an involution\" (forall (xs List<Int>)) \
         (block (== (call reverse (call reverse xs)) xs)))"
    );
    assert_eq!(
        dump("law \"empty reverses\" { reverse(nil()) == nil() }"),
        "(law \"empty reverses\" (block (== (call reverse (call nil)) (call nil))))"
    );
}

#[test]
fn a_law_binder_may_be_function_typed() {
    assert_eq!(
        dump(
            "law \"map fuses\" forall (xs: List<a>, f: (a) -> b, g: (b) -> c) { map(map(xs, f), g) == map(xs, |x| g(f(x))) }"
        ),
        "(law \"map fuses\" (forall (xs List<a>) (f (fn (a) -> b)) (g (fn (b) -> c))) \
         (block (== (call map (call map xs f) g) (call map xs (lam ((x _)) (call g (call f x)))))))"
    );
}

#[test]
fn a_law_body_may_be_a_simulate_region() {
    assert_eq!(
        dump("law \"conserves\" forall (n: Int) { simulate { total() == n } }"),
        "(law \"conserves\" (forall (n Int)) (block (simulate (block (== (call total) n)))))"
    );
}

#[test]
fn a_law_is_only_an_item_when_a_label_follows() {
    let m = ok("fn f() = { let law = 1; law }\nlaw \"one\" { f() == 1 }");
    assert_eq!(m.items.len(), 2);
    assert!(matches!(m.items[1], Item::Law(_)));
    assert!(m.items[1].name().is_none());
    assert!(!m.items[1].visibility().is_public());
}

#[test]
fn a_law_cannot_be_pub() {
    let ds = errs("pub law \"one\" { f() == 1 }");
    assert!(
        ds.iter().any(|d| d.message.contains("cannot be `pub`")),
        "{ds:#?}"
    );
}

#[test]
fn a_forall_binder_without_a_type_is_a_diagnostic() {
    let ds = errs("law \"one\" forall (x) { x == x }");
    assert!(
        ds.iter().any(|d| d.message.contains("must be annotated")),
        "{ds:#?}"
    );
}

#[test]
fn an_empty_forall_is_a_diagnostic() {
    let ds = errs("law \"one\" forall () { true }");
    assert!(
        ds.iter().any(|d| d.message.contains("binds nothing")),
        "{ds:#?}"
    );
}

#[test]
fn a_law_after_a_broken_definition_still_parses() {
    let (m, ds) = parse_recovering(
        SRC,
        ModuleName::anonymous(),
        "fn f( = 1\nlaw \"one\" { true }",
    );
    assert!(!ds.is_empty());
    assert!(
        m.items.iter().any(|i| matches!(i, Item::Law(_))),
        "{:#?}",
        m.items.len()
    );
}

#[test]
fn a_local_binder_may_share_a_module_binders_name() {
    assert_eq!(
        expr("{ let orders = 1; orders + orders::count() }"),
        "(block (let orders 1) (+ orders (call orders::count)))"
    );
}

#[test]
fn a_module_with_imports_and_pub_items_parses_whole() {
    let src = r#"
import ledger
import store.orders as ord
import store.orders (place, cancel)

pub type Order = Placed(Int) | Cancelled

pub effect db {
  read  get[r](key: Int) -> Order
  write put[r](key: Int, value: Order) -> Unit
}

pub fn total(xs: List<ord::Order>) -> Int / {store::db.read[users]} =
  fold(xs, 0, |acc, x| acc + ledger::amount(x))

fn internal() = place(1) + cancel(2)

test "cross-module" {
  handle { assert_eq(total([]), 0) } with { store::db.get[users](k) -> Cancelled }
}
"#;
    let m = ok(src);
    assert_eq!(m.imports.len(), 3);
    assert_eq!(m.items.len(), 5);
    let vis: Vec<bool> = m.items.iter().map(|i| i.visibility().is_public()).collect();
    assert_eq!(vis, [true, true, true, false, false]);
}

#[test]
fn each_input_to_parse_program_becomes_its_own_module() {
    let program = parse_program([
        (SourceId(0), ModuleName::from_dotted("a"), "fn one() = 1"),
        (SourceId(1), ModuleName::from_dotted("b"), "fn two() = 2"),
    ])
    .unwrap();
    assert_eq!(program.modules.len(), 2);
    assert_eq!(program.modules[0].items.len(), 1);
    assert_eq!(program.modules[1].items.len(), 1);
    assert_eq!(program.index_of(&ModuleName::from_dotted("b")), Some(1));
    assert!(program.find(&ModuleName::from_dotted("c")).is_none());
}

#[test]
fn a_row_written_with_a_set_carries_the_sets_atoms() {
    assert_eq!(
        dump(
            "effect set Web = {db.read[users], log.write}\n\
             fn f() -> Int / {Web, clock.read} = 1"
        ),
        "(effect-set Web {db.read[users], log.write})\n\
         (fn f () -> Int / {clock.read, db.read[users], log.write} 1)"
    );
}

/// The alias survives beside the atoms it stood for: erased by normalization, so it moves no hash,
/// and kept so `--explain` can say how the row was written.
#[test]
fn a_row_keeps_the_set_names_it_was_written_with() {
    let m = ok("effect set Web = {log.write}\nfn f() -> Int / {Web, clock.read} = 1");
    let Item::Fn(f) = &m.items[1] else {
        panic!("expected a fn")
    };
    let row = f.effects.as_ref().expect("an annotated row");
    let aliases: Vec<String> = row.aliases.iter().map(|q| q.to_string()).collect();
    assert_eq!(aliases, ["Web"]);
    assert_eq!(row.atoms.len(), 2);
}

#[test]
fn a_set_may_name_another_set() {
    assert_eq!(
        dump(
            "effect set Inner = {db.read[users]}\n\
             effect set Web = {Inner, log.write}\n\
             fn f() -> Int / {Web} = 1"
        ),
        "(effect-set Inner {db.read[users]})\n\
         (effect-set Web {db.read[users], log.write})\n\
         (fn f () -> Int / {db.read[users], log.write} 1)"
    );
}

/// Declaration order is not dependency order, and expansion is a fixed point rather than a fold
/// over the file.
#[test]
fn a_set_may_name_one_declared_after_it() {
    assert_eq!(
        dump(
            "effect set Web = {Inner, log.write}\n\
             effect set Inner = {db.read[users]}\n\
             fn f() -> Int / {Web} = 1"
        ),
        "(effect-set Web {db.read[users], log.write})\n\
         (effect-set Inner {db.read[users]})\n\
         (fn f () -> Int / {db.read[users], log.write} 1)"
    );
}

#[test]
fn an_atom_reached_twice_appears_once_in_the_expansion() {
    assert_eq!(
        dump(
            "effect set A = {db.read[users]}\n\
             effect set B = {db.read[users], A, log.write}\n\
             fn f() -> Int / {B} = 1"
        ),
        "(effect-set A {db.read[users]})\n\
         (effect-set B {db.read[users], log.write})\n\
         (fn f () -> Int / {db.read[users], log.write} 1)"
    );
}

#[test]
fn a_set_member_may_be_qualified_by_the_module_its_effect_came_from() {
    assert_eq!(
        dump("effect set Web = {store::db.read[users]}\nfn f() -> Int / {Web} = 1"),
        "(effect-set Web {store::db.read[users]})\n\
         (fn f () -> Int / {store::db.read[users]} 1)"
    );
}

#[test]
fn a_set_expands_inside_a_function_typed_parameter() {
    assert_eq!(
        dump(
            "effect set Web = {log.write}\n\
             fn run(f: () -> Int / {Web}) -> Int = f()"
        ),
        "(effect-set Web {log.write})\n\
         (fn run ((f (fn () -> Int / {log.write}))) -> Int (call f))"
    );
}

#[test]
fn a_set_expands_inside_a_let_annotation() {
    let m = ok("effect set Web = {log.write}\n\
         fn f() -> Int { let g: () -> Int / {Web} = || 1; g() }");
    let dumped = dump_module(&m);
    assert!(dumped.contains("(fn () -> Int / {log.write})"), "{dumped}");
}

/// `effect set` is only a set when a name follows `set`; `effect set { .. }` is still an ordinary
/// effect that happens to be called `set`.
#[test]
fn an_effect_may_still_be_named_set() {
    assert_eq!(
        dump("effect set {\n  read now() -> Int\n}"),
        "(effect set (op read now () -> Int))"
    );
}

/// A whole row that is a bare name is a row variable, as it always was.
#[test]
fn a_bare_row_is_still_a_row_variable() {
    assert_eq!(
        dump("effect set Web = {log.write}\nfn f<a | e>(x: a) -> a / e = x"),
        "(effect-set Web {log.write})\n(fn f <a | e> ((x a)) -> a / {| e} x)"
    );
}

#[test]
fn a_row_naming_an_undeclared_set_is_refused() {
    let ds = errs("fn f() -> Int / {Web} = 1");
    assert_eq!(ds[0].code, codes::UNKNOWN_EFFECT_SET);
    assert!(ds[0].message.contains("`Web`"), "{ds:#?}");
}

#[test]
fn a_qualified_set_reference_is_refused_and_says_why() {
    let ds = errs("fn f() -> Int / {shared::Web} = 1");
    assert_eq!(ds[0].code, codes::UNKNOWN_EFFECT_SET);
    assert!(
        ds[0]
            .notes
            .iter()
            .any(|n| n.contains("module-local") || n.contains("declaring module")),
        "{ds:#?}"
    );
}

#[test]
fn a_pub_effect_set_is_refused() {
    let ds = errs("pub effect set Web = {log.write}\nfn f() -> Int / {Web} = 1");
    assert_eq!(ds[0].code, codes::UNKNOWN_EFFECT_SET);
    assert!(ds[0].message.contains("cannot be `pub`"), "{ds:#?}");
}

/// A member is an atom or another set, never a whole effect: "every atom of `db`" is every resource
/// label anywhere in the program.
#[test]
fn a_set_naming_a_whole_effect_is_refused_with_the_reason() {
    let ds = errs(
        "effect db {\n  read all[t]() -> Int\n}\n\
         effect set Web = {db}\n\
         fn f() -> Int / {Web} = 1",
    );
    assert_eq!(ds[0].code, codes::UNKNOWN_EFFECT_SET);
    assert!(
        ds[0]
            .notes
            .iter()
            .any(|n| n.contains("member of a set is an atom")),
        "{ds:#?}"
    );
    assert!(
        ds[0]
            .notes
            .iter()
            .any(|n| n.contains("every resource label anywhere in the program")),
        "{ds:#?}"
    );
}

#[test]
fn a_set_that_names_itself_is_refused() {
    let ds = errs("effect set Web = {Web, log.write}\nfn f() -> Int / {Web} = 1");
    assert_eq!(ds[0].code, codes::EFFECT_SET_CYCLE);
    assert!(
        ds[0].notes.iter().any(|n| n.contains("`Web` -> `Web`")),
        "{ds:#?}"
    );
}

#[test]
fn a_cycle_through_another_set_is_refused_and_named_in_order() {
    let ds = errs(
        "effect set A = {B}\n\
         effect set B = {C}\n\
         effect set C = {A}\n\
         fn f() -> Int / {A} = 1",
    );
    let cycle = ds
        .iter()
        .find(|d| d.code == codes::EFFECT_SET_CYCLE)
        .unwrap_or_else(|| panic!("{ds:#?}"));
    assert!(
        cycle
            .notes
            .iter()
            .any(|n| n.contains("`A` -> `B` -> `C` -> `A`")),
        "{cycle:#?}"
    );
}

#[test]
fn two_sets_with_one_name_are_a_duplicate_definition() {
    let ds = errs(
        "effect set Web = {log.write}\n\
         effect set Web = {db.read[users]}\n\
         fn f() -> Int / {Web} = 1",
    );
    assert_eq!(ds[0].code, codes::DUPLICATE_DEFINITION);
    assert!(ds[0].message.contains("effect set `Web`"), "{ds:#?}");
}

/// A set name lives in no namespace `resolve` knows about — expansion has erased it before
/// `resolve` runs — so it collides with nothing.
#[test]
fn a_set_name_may_be_reused_by_a_type() {
    let m = ok("type Web = Int\neffect set Web = {log.write}\nfn f() -> Int / {Web} = 1");
    assert_eq!(m.items.len(), 3);
}

#[test]
fn an_effect_set_cannot_carry_a_row_variable() {
    let ds = errs("effect set Web = {log.write | e}\nfn f() -> Int / {Web} = 1");
    assert!(
        ds.iter().any(|d| d.message.contains("row variable")),
        "{ds:#?}"
    );
}

#[test]
fn an_empty_effect_set_expands_to_nothing() {
    assert_eq!(
        dump("effect set None = {}\nfn f() -> Int / {None} = 1"),
        "(effect-set None {})\n(fn f () -> Int / {} 1)"
    );
}

#[test]
fn a_trailing_comma_in_a_set_is_accepted() {
    assert_eq!(
        dump("effect set Web = {\n  db.read[users],\n  log.write,\n}\nfn f() -> Int / {Web} = 1"),
        "(effect-set Web {db.read[users], log.write})\n\
         (fn f () -> Int / {db.read[users], log.write} 1)"
    );
}

/// Every `.ply` file under `dir`, skipping dotted directories and `target`.
fn collect_ply(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        match path.is_dir() {
            true => collect_ply(&path, out),
            false if path.extension().is_some_and(|x| x == "ply") => out.push(path),
            false => {}
        }
    }
}

// --- The `?` operator --------------------------------------------------------

/// Whether an unexpanded [`ExprKind::Try`] is anywhere in the tree.
fn has_try(m: &Module) -> bool {
    fn e(x: &Expr) -> bool {
        match &x.kind {
            ExprKind::Try { .. } => true,
            ExprKind::Lit(_) | ExprKind::Var(_) => false,
            ExprKind::Binary { lhs, rhs, .. } => e(lhs) || e(rhs),
            ExprKind::Unary { operand, .. } => e(operand),
            ExprKind::Lambda { body, .. } => e(body),
            ExprKind::App { func, args, .. } => e(func) || args.iter().any(e),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => e(cond) || e(then_branch) || e(else_branch),
            ExprKind::Match { scrutinee, arms } => {
                e(scrutinee)
                    || arms
                        .iter()
                        .any(|a| a.guard.as_ref().is_some_and(e) || e(&a.body))
            }
            ExprKind::Block { stmts, tail } => {
                stmts.iter().any(|s| match s {
                    Stmt::Let { value, .. } => e(value),
                    Stmt::Expr(x) => e(x),
                }) || tail.as_deref().is_some_and(e)
            }
            ExprKind::Record { fields } => fields.iter().any(|(_, v)| e(v)),
            ExprKind::RecordUpdate { base, fields } => e(base) || fields.iter().any(|(_, v)| e(v)),
            ExprKind::Field { base, .. } => e(base),
            ExprKind::List { items } => items.iter().any(e),
            ExprKind::Perform { args, .. } => args.iter().any(e),
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                e(body)
                    || clauses.iter().any(|c| e(&c.body))
                    || return_clause.as_ref().is_some_and(|r| e(&r.body))
            }
            ExprKind::WithCell { init, body, .. } => e(init) || e(body),
            ExprKind::WithRegion { body, .. } => e(body),
            ExprKind::Simulate { body } => e(body),
        }
    }
    m.items.iter().any(|item| match item {
        Item::Fn(d) => d.spec.iter().any(|s| e(&s.expr)) || e(&d.body),
        Item::Test(d) => e(&d.body),
        Item::Law(d) => d.guard.as_ref().is_some_and(e) || e(&d.body),
        Item::Type(_) | Item::Effect(_) | Item::Derive(_) | Item::EffectSet(_) => false,
    })
}

/// The body of the only `fn` in a module that parsed clean, dumped.
fn body_of(src: &str) -> String {
    let m = ok(src);
    let Some(Item::Fn(f)) = m.items.iter().rev().find(|i| matches!(i, Item::Fn(_))) else {
        panic!("expected a fn")
    };
    dump_expr(&f.body)
}

fn codes_of(src: &str) -> Vec<&'static str> {
    errs(src).iter().map(|d| d.code).collect()
}

/// A `fn` returning `Result` whose body is `f(x)?`, and a mode-establishing preamble every fixture
/// below shares.
const PRE: &str = "type E = {msg: String}\nfn g(n: Int) -> Result<Int, E> = Ok(n)\n";

#[test]
fn try_expands_to_a_match_with_the_failure_arm_first() {
    assert_eq!(
        body_of(&format!("{PRE}fn f(n: Int) -> Result<Int, E> = Ok(g(n)?)")),
        "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) (arm (ctor Ok ?0) (call Ok ?0)))"
    );
}

/// `None` carries no payload, so its arm is a bare constructor on both sides.
#[test]
fn an_option_returning_function_gets_the_option_constructors() {
    assert_eq!(
        body_of("fn h(n: Int) -> Option<Int> = Some(n)\nfn f(n: Int) -> Option<Int> = Some(h(n)?)"),
        "(match (call h n) (arm (ctor None) None) (arm (ctor Some ?0) (call Some ?0)))"
    );
}

/// The shape every conversion in the corpus takes: the `let`'s own pattern becomes the success
/// arm's binder and the `let` itself is gone, so the sugar and the hand-written `match` are one
/// definition.
#[test]
fn a_let_bound_try_puts_its_own_pattern_on_the_success_arm() {
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = {{ let a = g(n)?; Ok(a) }}"
        )),
        "(block (match (call g n) (arm (ctor Err ?0) (call Err ?0)) \
         (arm (ctor Ok a) (block (call Ok a)))))"
    );
}

/// The parser spike's §3 "pair" shape, spelled without a tuple.
#[test]
fn a_try_composes_with_record_destructuring() {
    assert_eq!(
        body_of(
            "type P = {p: Int, node: Int}\n\
             fn q(n: Int) -> Result<P, Int> = Ok({p: n, node: n})\n\
             fn f(n: Int) -> Result<Int, Int> = { let {p, node} = q(n)?; Ok(p + node) }"
        ),
        "(block (match (call q n) (arm (ctor Err ?0) (call Err ?0)) \
         (arm (ctor Ok (prec (p p) (node node))) (block (call Ok (+ p node))))))"
    );
}

/// The block splits at the statement carrying the `?`, and everything after it — statements *and*
/// tail — becomes the success arm's body.
#[test]
fn a_block_splits_at_the_statement_carrying_the_try() {
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = {{ let z = n + 1; let a = g(z)?; let w = a + 1; Ok(w) }}"
        )),
        "(block (let z (+ n 1)) (match (call g z) (arm (ctor Err ?0) (call Err ?0)) \
         (arm (ctor Ok a) (block (let w (+ a 1)) (call Ok w)))))"
    );
}

/// Two `?`s in one run nest, outer first, and each success arm's body is itself a return position —
/// which is what keeps the continuation in tail position.
#[test]
fn two_tries_in_a_row_nest_in_the_order_written() {
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = {{ let a = g(n)?; let b = g(a)?; Ok(a + b) }}"
        )),
        "(block (match (call g n) (arm (ctor Err ?1) (call Err ?1)) (arm (ctor Ok a) \
         (block (match (call g a) (arm (ctor Err ?0) (call Err ?0)) \
         (arm (ctor Ok b) (block (call Ok (+ a b)))))))))"
    );
}

/// `parse_or(ts)?` in the argument of a call whose function is a bare name: the prefix is one
/// `Var`, which is pure, so the lift is admitted and the continuation stays a tail call.
#[test]
fn a_try_in_a_call_argument_with_a_pure_prefix_is_lifted() {
    assert_eq!(
        body_of(&format!("{PRE}fn f(n: Int) -> Result<Int, E> = g(g(n)?)")),
        "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) (arm (ctor Ok ?0) (call g ?0)))"
    );
}

/// A branch of an `if` in return position is a return position of its own, so a `?` there stays
/// inside the branch and is never lifted across the condition.
#[test]
fn a_try_in_a_return_position_branch_stays_in_the_branch() {
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int, c: Bool) -> Result<Int, E> = if c {{ Ok(g(n)?) }} else {{ Ok(0) }}"
        )),
        "(if c (block (match (call g n) (arm (ctor Err ?1) (call Err ?1)) \
         (arm (ctor Ok ?0) (call Ok ?0)))) (block (call Ok 0)))"
    );
}

/// The condition, by contrast, runs unconditionally, so the `if` node itself is the region root and
/// the whole `if` moves into the success arm.
#[test]
fn a_try_in_a_condition_wraps_the_whole_if() {
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = if g(n)? > 0 {{ Ok(1) }} else {{ Ok(2) }}"
        )),
        "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) \
         (arm (ctor Ok ?0) (if (> ?0 0) (block (call Ok 1)) (block (call Ok 2)))))"
    );
}

/// Postfix, in the tightest tier alongside `f(x)` and `r.field`: `g(n)?.msg` is `(g(n)?).msg`,
/// `-x?` is `-(x?)` and `a == b?` is `a == (b?)`
#[test]
fn question_binds_tighter_than_field_access_and_unary_minus() {
    let pre = "type E = {msg: Int}\n\
               fn r(n: Int) -> Result<E, E> = Ok({msg: n})\n\
               fn g(n: Int) -> Result<Int, E> = Ok(n)\n\
               fn gg(n: Int) -> Result<Result<Int, E>, E> = Ok(Ok(n))\n";
    for (src, want) in [
        (
            "Ok(r(n)?.msg)",
            "(match (call r n) (arm (ctor Err ?1) (call Err ?1)) \
             (arm (ctor Ok ?0) (call Ok (field ?0 msg))))",
        ),
        (
            "Ok(-g(n)?)",
            "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) \
             (arm (ctor Ok ?0) (call Ok (neg ?0))))",
        ),
        (
            "Ok(n == g(n)?)",
            "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) \
             (arm (ctor Ok ?0) (call Ok (== n ?0))))",
        ),
        (
            "Ok(gg(n)??)",
            "(match (call gg n) (arm (ctor Err ?1) (call Err ?1)) \
             (arm (ctor Ok ?0) (match ?0 (arm (ctor Err ?3) (call Err ?3)) \
             (arm (ctor Ok ?2) (call Ok ?2)))))",
        ),
    ] {
        assert_eq!(
            body_of(&format!("{pre}fn f(n: Int) -> Result<Int, E> = {src}")),
            want,
            "for {src}"
        );
    }
}

// --- What `?` refuses, and with which code ----------------------------------

/// `?` reads the mode off the enclosing function's **written** return type, because the parser has
/// no types.
#[test]
fn a_try_with_no_readable_return_type_is_e0118() {
    for (what, src) in [
        ("no `->` at all", format!("{PRE}fn f(n: Int) = g(n)?")),
        (
            "a head that is neither",
            format!("{PRE}fn f(n: Int) -> Int = g(n)?"),
        ),
        (
            "a type parameter",
            format!("{PRE}fn f<a>(n: Int, d: a) -> a = g(n)?"),
        ),
        (
            "a generic alias declared here",
            format!("{PRE}type Box<a> = {{v: a}}\nfn f(n: Int) -> Box<Int> = g(n)?"),
        ),
        (
            "a record type",
            format!("{PRE}fn f(n: Int) -> {{v: Int}} = g(n)?"),
        ),
        (
            "a lambda body",
            format!("{PRE}fn f(n: Int) -> Result<Int, E> = Ok((|x: Int| g(x)?)(n))"),
        ),
        (
            "a `with_region` body",
            format!("{PRE}fn f(n: Int) -> Result<Int, E> = with_region[r] {{ Ok(g(n)?) }}"),
        ),
        (
            "a `simulate` body",
            format!("{PRE}fn f(n: Int) -> Result<Int, E> = simulate {{ Ok(g(n)?) }}"),
        ),
        ("a `test`", format!("{PRE}test \"t\" {{ g(1)? }}")),
        (
            "a `law`",
            format!("{PRE}law \"l\" forall (n: Int) {{ g(n)? == 1 }}"),
        ),
        (
            "a `requires`",
            format!("{PRE}fn f(n: Int) -> Result<Int, E> requires g(n)? > 0 = Ok(n)"),
        ),
    ] {
        assert_eq!(
            codes_of(&src),
            vec![codes::TRY_SCOPE],
            "{what} should be `E0118`"
        );
    }
}

/// An alias chain in this file is followed; one that leaves it is not, for `record_update`'s reason
/// — gate 1 skips a file whose bytes are unchanged, so a meaning read across a module boundary
/// could go stale in a file that never moved.
#[test]
fn a_local_alias_to_result_is_followed_and_a_foreign_one_is_not() {
    assert_eq!(
        body_of(&format!(
            "{PRE}type R = Result<Int, E>\ntype R2 = R\nfn f(n: Int) -> R2 = Ok(g(n)?)"
        )),
        "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) (arm (ctor Ok ?0) (call Ok ?0)))"
    );
    assert_eq!(
        codes_of(&format!(
            "import std.json\n{PRE}fn f(n: Int) -> json::Json = g(n)?"
        )),
        vec![codes::TRY_SCOPE]
    );
}

/// GUIDE §5.7: constructor names are not reserved, so a module may declare its own `Ok` — and
/// expanding a `?` in one would build a `match` naming *that* constructor.
#[test]
fn a_module_that_declares_its_own_ok_refuses_every_try() {
    assert_eq!(
        codes_of(&format!(
            "{PRE}type Mine = Ok(Int) | Nope\nfn f(n: Int) -> Result<Int, E> = Ok(g(n)?)"
        )),
        vec![codes::TRY_SCOPE]
    );
}

/// The four names the expansion emits are not reserved, and a `type` is not the only way to bind
/// one: `import m (Err)` binds `Err` **unqualified**, in the same `Namespace::Value` a declared
/// constructor lives in.
#[test]
fn a_module_that_imports_ok_or_err_unqualified_refuses_every_try() {
    let lib = "pub type Weird<a, e> = Err(e) | Fine(a)\n";
    for name in ["Ok", "Err", "Some", "None"] {
        let app = format!("import lib ({name})\n{PRE}fn f(n: Int) -> Result<Int, E> = Ok(g(n)?)");
        let diags = crate::parse_program(vec![
            (SRC, ModuleName::from_dotted("lib"), lib),
            (SourceId(1), ModuleName::from_dotted("app"), app.as_str()),
        ])
        .expect_err("the expansion would name the imported constructor");
        assert!(
            diags.iter().any(|d| d.code == codes::TRY_SCOPE),
            "importing `{name}` unqualified should refuse `?`; got {:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
    // The module binder alone captures nothing: `lib::Err` is qualified.
    let app = format!("import lib\n{PRE}fn f(n: Int) -> Result<Int, E> = Ok(g(n)?)");
    crate::parse_program(vec![
        (SRC, ModuleName::from_dotted("lib"), lib),
        (SourceId(1), ModuleName::from_dotted("app"), app.as_str()),
    ])
    .expect("an unqualified import binds nothing the expansion emits");
}

/// A statement whose whole value is a `?` binds nothing, so the success arm takes a **wildcard**
/// and the statement itself is gone.
#[test]
fn a_bare_try_statement_binds_nothing_and_keeps_the_wildcard() {
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = {{ g(n)?; Ok(1) }}"
        )),
        "(block (match (call g n) (arm (ctor Err ?0) (call Err ?0)) \
         (arm (ctor Ok _) (block (call Ok 1)))))"
    );
}

/// `sequence` walks a call's parts **left to right**, and that direction is the whole of the
/// impure-prefix rule for the one shape GUIDE §6.10 spells out: "`g(h(x), k(x)?)` is `E0119` —
/// `h(x)` is written before the `?` and the expansion would evaluate it after."
#[test]
fn a_call_argument_scan_stops_at_an_impure_argument_and_not_before_one() {
    let pre =
        format!("{PRE}fn side(n: Int) -> Int = n + 1\nfn two(a: Int, b: Int) -> Int = a + b\n");
    // Impure to the *left* of the `?`: refused, because the lift would run `g(n)` before `side(n)`.
    assert_eq!(
        codes_of(&format!(
            "{pre}fn f(n: Int) -> Result<Int, E> = Ok(two(side(n), g(n)?))"
        )),
        vec![codes::TRY_POSITION]
    );
    // Impure to the *right* of it: lifted, because nothing moves across anything — `g(n)` already
    // runs first.
    assert_eq!(
        body_of(&format!(
            "{pre}fn f(n: Int) -> Result<Int, E> = Ok(two(g(n)?, side(n)))"
        )),
        "(match (call g n) (arm (ctor Err ?1) (call Err ?1)) \
         (arm (ctor Ok ?0) (call Ok (call two ?0 (call side n)))))"
    );
}

/// GUIDE §6.10 names six barriers a `?` may not cross and says every one is `E0118`.
#[test]
fn a_try_inside_a_handler_or_a_cell_is_e0118() {
    let eff = "effect ctr { write bump() -> Int }\n";
    for (what, src) in [
        (
            "a `handle` body",
            format!(
                "{eff}{PRE}fn f(n: Int) -> Result<Int, E> = handle Ok(g(n)?) with {{ ctr.bump() -> 0 }}"
            ),
        ),
        (
            "a `handle` clause",
            format!(
                "{eff}{PRE}fn f(n: Int) -> Result<Int, E> = \
                 handle Ok(ctr.bump()) with {{ ctr.bump() -> {{ let q = g(n)?; 0 }} }}"
            ),
        ),
        (
            "a `handle` return clause",
            format!(
                "{eff}{PRE}fn f(n: Int) -> Result<Int, E> = \
                 handle Ok(1) with {{ ctr.bump() -> 0, return x -> {{ let q = g(n)?; x }} }}"
            ),
        ),
        (
            "a `with_cell` body",
            format!(
                "{PRE}fn f(n: Int) -> Result<Int, E> = \
                 with_cell[k](0) {{ c -> Ok(g(n)?) }}"
            ),
        ),
    ] {
        assert_eq!(
            codes_of(&src),
            vec![codes::TRY_SCOPE],
            "{what} should be `E0118`"
        );
    }
}

/// The float is the whole risk, and this is the rule that closes it: expansion lifts the operand to
/// the head of its region, so anything evaluated before it must be reorderable — which is
/// `is_pure`, the predicate normalization already uses to license reordering a run of `let`s.
#[test]
fn a_try_with_an_impure_prefix_is_e0119() {
    let eff = "effect ctr { read now() -> Int }\n";
    let side = "fn side(n: Int) -> Int = n + 1\n";
    for (what, src) in [
        (
            "a call to its left",
            format!("{PRE}{side}fn f(n: Int) -> Result<Int, E> = Ok(side(n) + g(n)?)"),
        ),
        (
            "a `perform` to its left",
            format!(
                "{eff}{PRE}fn f(n: Int) -> Result<Int, E> / {{ctr.read}} = Ok(ctr.now() + g(n)?)"
            ),
        ),
        (
            "an `if` whose branch calls",
            format!(
                "{PRE}{side}fn f(n: Int) -> Result<Int, E> = \
                 Ok(if n > 0 {{ side(n) }} else {{ 0 }} + g(n)?)"
            ),
        ),
        (
            "a `match` whose arm calls",
            format!(
                "{PRE}{side}fn f(n: Int) -> Result<Int, E> = \
                 Ok(match n {{ 0 -> side(n), _ -> 0 }} + g(n)?)"
            ),
        ),
        (
            "a call behind a `&&`, which the scan may not enter",
            format!(
                "{PRE}{side}fn f(n: Int, c: Bool) -> Result<Int, E> = \
                 Ok(if c && side(n) > 0 {{ 1 }} else {{ 0 }} + g(n)?)"
            ),
        ),
        (
            "a nested block whose statement calls",
            format!(
                "{PRE}{side}fn f(n: Int) -> Result<Int, E> = \
                 Ok({{ let z = side(n); z }} + g(n)?)"
            ),
        ),
    ] {
        assert_eq!(
            codes_of(&src),
            vec![codes::TRY_POSITION],
            "{what} should be `E0119`"
        );
    }
}

/// Nothing conditional may sit between the region root and the `?`
#[test]
fn a_try_behind_a_conditional_is_e0119() {
    for src in [
        format!(
            "{PRE}fn f(n: Int, c: Bool) -> Result<Int, E> = {{ let y = if c {{ g(n)? }} else {{ 0 }}; Ok(y) }}"
        ),
        format!("{PRE}fn f(n: Int, c: Bool) -> Result<Int, E> = Ok(if c {{ g(n)? }} else {{ 0 }})"),
        format!(
            "{PRE}fn f(n: Int, c: Bool) -> Result<Int, E> = Ok(match c {{ true -> g(n)?, false -> 0 }})"
        ),
        format!(
            "{PRE}fn f(n: Int, c: Bool) -> Result<Int, E> = Ok(if c && g(n)? > 0 {{ 1 }} else {{ 0 }})"
        ),
        format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = match n {{ m if g(m)? > 0 -> Ok(1), _ -> Ok(2) }}"
        ),
    ] {
        assert_eq!(codes_of(&src), vec![codes::TRY_POSITION], "for {src}");
    }
}

/// The expansion has no `let` left to carry the annotation on, and a written annotation must not
/// evaporate.
#[test]
fn a_try_that_is_the_whole_value_of_an_annotated_let_is_e0119() {
    assert_eq!(
        codes_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = {{ let a: Int = g(n)?; Ok(a) }}"
        )),
        vec![codes::TRY_POSITION]
    );
    assert_eq!(
        body_of(&format!(
            "{PRE}fn f(n: Int) -> Result<Int, E> = {{ let a: Int = g(n)? + 1; Ok(a) }}"
        )),
        "(block (match (call g n) (arm (ctor Err ?1) (call Err ?1)) \
         (arm (ctor Ok ?0) (block (let a Int (+ ?0 1)) (call Ok a)))))"
    );
}

/// **The guard behind every `unreachable!` arm downstream.**
#[test]
fn no_try_survives_parse_module_anywhere_in_the_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root");
    let mut files = Vec::new();
    collect_ply(root, &mut files);
    assert!(
        files.len() > 100,
        "only {} `.ply` files found under {}; the walk is not reaching the corpus",
        files.len(),
        root.display()
    );

    const USES_IT: &str = "\
type E = {msg: String}
type A = Result<Int, E>
fn g(n: Int) -> Result<Int, E> = Ok(n)
fn h(n: Int) -> Option<Int> = Some(n)
fn a(n: Int) -> Result<Int, E> = Ok(g(n)?)
fn b(n: Int) -> A = { let x = g(n)?; let y = g(x)?; Ok(x + y) }
fn c(n: Int) -> Option<Int> = { let x = h(n)?; Some(x) }
fn d(n: Int) -> Result<Int, E> = g(g(n)?)
fn e(n: Int, c: Bool) -> Result<Int, E> = if c { Ok(g(n)?) } else { Ok(0) }
fn refused_scope(n: Int) -> Int = g(n)?
fn refused_position(n: Int, c: Bool) -> Result<Int, E> = Ok(if c { g(n)? } else { 0 })
fn refused_lambda(n: Int) -> Result<Int, E> = Ok((|x: Int| g(x)?)(n))
test \"t\" { assert_eq(g(1)?, 1) }
law \"l\" forall (n: Int) { g(n)? == n }
";
    let mut sources: Vec<(String, String)> = files
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|s| (p.display().to_string(), s))
        })
        .collect();
    sources.push(("<uses `?`>".to_string(), USES_IT.to_string()));

    let mut saw_the_syntax = false;
    for (name, source) in &sources {
        if name.starts_with("<uses") {
            saw_the_syntax = true;
        }
        let (m, _) = parse_recovering(SRC, ModuleName::from_dotted("m"), source);
        assert!(
            !has_try(&m),
            "an unexpanded `?` escaped `parse_recovering` from {name}"
        );
        if let Ok(m) = parse(SRC, source) {
            assert!(
                !has_try(&m),
                "an unexpanded `?` escaped `parse` from {name}"
            );
        }
    }
    assert!(
        saw_the_syntax,
        "no source in this run wrote a `?`, so the guard proved nothing"
    );
}

/// `parse_expr` has no `fn` around it and so no written return type.
#[test]
fn parse_expr_refuses_a_try_rather_than_leaking_one() {
    let d = crate::parser::parse_expr(SRC, "g(1)?")
        .expect_err("a bare expression has no return type to read a mode off");
    assert!(d.iter().any(|d| d.code == codes::TRY_SCOPE), "{d:#?}");
}

// --- Record update -----------------------------------------------------------

/// Whether an unexpanded [`ExprKind::RecordUpdate`] is anywhere in the tree.
fn has_record_update(m: &Module) -> bool {
    fn e(x: &Expr) -> bool {
        match &x.kind {
            ExprKind::RecordUpdate { .. } => true,
            ExprKind::Lit(_) | ExprKind::Var(_) => false,
            ExprKind::Binary { lhs, rhs, .. } => e(lhs) || e(rhs),
            ExprKind::Unary { operand, .. } => e(operand),
            ExprKind::Lambda { body, .. } => e(body),
            ExprKind::App { func, args, .. } => e(func) || args.iter().any(e),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => e(cond) || e(then_branch) || e(else_branch),
            ExprKind::Match { scrutinee, arms } => {
                e(scrutinee)
                    || arms
                        .iter()
                        .any(|a| a.guard.as_ref().is_some_and(e) || e(&a.body))
            }
            ExprKind::Block { stmts, tail } => {
                stmts.iter().any(|s| match s {
                    Stmt::Let { value, .. } => e(value),
                    Stmt::Expr(x) => e(x),
                }) || tail.as_deref().is_some_and(e)
            }
            ExprKind::Record { fields } => fields.iter().any(|(_, v)| e(v)),
            ExprKind::Field { base, .. } => e(base),
            ExprKind::Try { operand } => e(operand),
            ExprKind::List { items } => items.iter().any(e),
            ExprKind::Perform { args, .. } => args.iter().any(e),
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                e(body)
                    || clauses.iter().any(|c| e(&c.body))
                    || return_clause.as_ref().is_some_and(|r| e(&r.body))
            }
            ExprKind::WithCell { init, body, .. } => e(init) || e(body),
            ExprKind::WithRegion { body, .. } => e(body),
            ExprKind::Simulate { body } => e(body),
        }
    }
    m.items.iter().any(|item| match item {
        Item::Fn(d) => d.spec.iter().any(|s| e(&s.expr)) || e(&d.body),
        Item::Test(d) => e(&d.body),
        Item::Law(d) => d.guard.as_ref().is_some_and(e) || e(&d.body),
        Item::Type(_) | Item::Effect(_) | Item::Derive(_) | Item::EffectSet(_) => false,
    })
}

/// **The guard behind every `unreachable!` arm downstream.**
#[test]
fn no_record_update_survives_parse_module_anywhere_in_the_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root");
    let mut files = Vec::new();
    collect_ply(root, &mut files);
    assert!(
        files.len() > 100,
        "only {} `.ply` files found under {}; the walk is not reaching the corpus",
        files.len(),
        root.display()
    );

    // The corpus does not yet use the syntax everywhere, so a file that does is appended: a guard
    // that only ever saw programs without record updates would pass whether or not expansion ran at
    // all.
    const USES_IT: &str = "\
type L = {a: Int, b: Int}
type W = {lim: L, n: Int}
fn f(w: W) -> L = {..w.lim, a: 1}
fn g(l: L) -> L = {..l}
fn h(l: L) -> List<L> = [{..l, b: 2}, {..l, a: 3}]
fn k(l: L) -> L
  requires g({..l, a: 1}).a == 1
  ensures g({..l, b: 2}).b == 2
= {..l, a: 1}
test \"t\" { assert_eq(f({lim: {a: 0, b: 0}, n: 1}).a, 1) }
test \"u\" { let l: L = {a: 0, b: 0}; assert_eq({..l, a: 7}.a, 7) }
law \"c\" forall (l: L) where g({..l, a: 1}).a == 1 { g({..l, b: 2}).b == 2 }
";
    let mut sources: Vec<(String, String)> = files
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|s| (p.display().to_string(), s))
        })
        .collect();
    sources.push(("<uses record update>".to_string(), USES_IT.to_string()));

    let mut saw_the_syntax = false;
    for (name, source) in &sources {
        if source.contains("{..") {
            saw_the_syntax = true;
        }
        let (m, _) = parse_recovering(SRC, ModuleName::from_dotted("m"), source);
        assert!(
            !has_record_update(&m),
            "an unexpanded record update escaped `parse_recovering` from {name}"
        );
        if let Ok(m) = parse(SRC, source) {
            assert!(
                !has_record_update(&m),
                "an unexpanded record update escaped `parse` from {name}"
            );
        }
    }
    assert!(
        saw_the_syntax,
        "no source in this run contained `{{..`, so the guard proved nothing"
    );
}

/// `parse_expr` has no module around it and so no shape to resolve.
#[test]
fn parse_expr_refuses_a_record_update_rather_than_leaking_one() {
    let d = crate::parser::parse_expr(SRC, "{..s, a: 1}")
        .expect_err("a bare expression has no shape for the base");
    assert!(
        d.iter().any(|d| d.code == codes::RECORD_UPDATE_SHAPE),
        "{d:#?}"
    );
}

#[test]
fn a_record_update_parses_as_copies_then_writes() {
    let m = ok("type R = {a: Int, b: Int, c: Int}\nfn f(s: R) -> R = {..s, c: 1}");
    let Item::Fn(f) = &m.items[1] else {
        panic!("expected a fn")
    };
    assert_eq!(
        dump_expr(&f.body),
        "(rec (a (field s a)) (b (field s b)) (c 1))"
    );
}

/// The copies are sorted **by name**, and `a`/`b`/`c` cannot say so: every one-character field set
/// orders identically under any comparator that compares length first and name second, so a suite
/// written only in single letters passes whichever of the two ran.
#[test]
fn copies_are_sorted_by_name_and_not_by_length() {
    for (src, want, wrong) in [
        (
            "type R = {ab: Int, b: Int, c: Int}\nfn f(s: R) -> R = {..s, c: 1}",
            "(rec (ab (field s ab)) (b (field s b)) (c 1))",
            "shortest-first",
        ),
        (
            "type R = {a: Int, bb: Int, c: Int}\nfn f(s: R) -> R = {..s, c: 1}",
            "(rec (a (field s a)) (bb (field s bb)) (c 1))",
            "longest-first",
        ),
    ] {
        let m = ok(src);
        let Item::Fn(f) = &m.items[1] else {
            panic!("expected a fn")
        };
        assert_eq!(
            dump_expr(&f.body),
            want,
            "a {wrong} comparator reverses this pair and passes every \
             single-letter case in this file"
        );
    }
}

#[test]
fn a_record_update_with_no_written_fields_copies_every_field() {
    let m = ok("type R = {b: Int, a: Int}\nfn f(s: R) -> R = {..s}");
    let Item::Fn(f) = &m.items[1] else {
        panic!("expected a fn")
    };
    assert_eq!(dump_expr(&f.body), "(rec (a (field s a)) (b (field s b)))");
}

/// The sharpest trap in the pass.
#[test]
fn a_shadowing_binder_refuses_rather_than_using_the_outer_type() {
    for src in [
        "type R = {a: Int, b: Int}\nfn f(s: R) -> Int = { let s = 3; {..s, a: 1}; 0 }",
        "type R = {a: Int, b: Int}\nfn f(s: R) -> Int = match 1 { s -> { {..s, a: 1}; 0 } }",
        "type R = {a: Int, b: Int}\nfn f(s: R) -> Int = { let g = |s| {..s, a: 1}; 0 }",
    ] {
        let diags = errs(src);
        assert!(
            diags.iter().any(|d| d.code == codes::RECORD_UPDATE_SHAPE),
            "expected E0116 for {src}, got {diags:#?}"
        );
    }
}

/// The outer binder is still the one in scope while the *value* of a `let` is elaborated, so `let s
/// = {..s, a: 1}` updates the record it shadows.
#[test]
fn a_let_value_sees_the_binder_it_shadows() {
    let m = ok("type R = {a: Int, b: Int}\nfn f(s: R) -> R = { let s: R = {..s, a: 1}; s }");
    let Item::Fn(f) = &m.items[1] else {
        panic!("expected a fn")
    };
    assert_eq!(
        dump_expr(&f.body),
        "(block (let s R (rec (b (field s b)) (a 1))) s)"
    );
}

#[test]
fn a_record_update_may_not_add_a_field() {
    let diags = errs("type R = {a: Int}\nfn f(s: R) -> R = {..s, z: 1}");
    assert!(
        diags.iter().any(|d| d.code == codes::RECORD_UPDATE_FIELD),
        "{diags:#?}"
    );
}

#[test]
fn a_base_whose_type_lives_in_another_module_is_refused() {
    let diags = errs("import other.mod\nfn f(s: mod::Limits) -> Int = { {..s, a: 1}; 0 }");
    assert!(
        diags.iter().any(|d| d.code == codes::RECORD_UPDATE_SHAPE),
        "{diags:#?}"
    );
}

#[test]
fn a_generic_alias_is_refused_rather_than_guessed() {
    let diags = errs("type P<t> = {fst: t, snd: t}\nfn f(p: P<Int>) -> P<Int> = {..p, fst: 1}");
    assert!(
        diags.iter().any(|d| d.code == codes::RECORD_UPDATE_SHAPE),
        "{diags:#?}"
    );
}

#[test]
fn an_alias_cycle_is_refused_rather_than_looping() {
    let diags = errs("type A = B\ntype B = A\nfn f(x: A) -> Int = { {..x, a: 1}; 0 }");
    assert!(
        diags.iter().any(|d| d.code == codes::RECORD_UPDATE_SHAPE),
        "{diags:#?}"
    );
}

#[test]
fn a_second_base_and_a_three_dot_spelling_are_parse_errors() {
    for src in [
        "type R = {a: Int}\nfn f(s: R) -> R = {..s, ..s, a: 1}",
        "type R = {a: Int}\nfn f(s: R) -> R = {...s, a: 1}",
    ] {
        let diags = errs(src);
        assert!(
            diags.iter().any(|d| d.code == codes::UNEXPECTED_TOKEN),
            "expected a parse error for {src}, got {diags:#?}"
        );
    }
}

/// `{x}` is still a block and `{x: e}` is still a record: adding `..` to the lookahead must not
/// have moved either.
#[test]
fn the_brace_disambiguation_is_unchanged() {
    assert_eq!(expr("{x}"), "(block x)");
    assert_eq!(expr("{x: 1}"), "(rec (x 1))");
    assert_eq!(expr("{x, y}"), "(rec (x x) (y y))");
}

/// Whether any call in the module still carries a named argument, or any parameter a default that
/// was never matched against a call.
fn has_named_argument(m: &Module) -> bool {
    fn e(x: &Expr) -> bool {
        crate::effect_set::grow(|| match &x.kind {
            ExprKind::App { func, args, named } => {
                !named.is_empty() || e(func) || args.iter().any(e)
            }
            ExprKind::Lit(_) | ExprKind::Var(_) => false,
            ExprKind::Binary { lhs, rhs, .. } => e(lhs) || e(rhs),
            ExprKind::Unary { operand, .. } => e(operand),
            ExprKind::Lambda { body, .. } => e(body),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => e(cond) || e(then_branch) || e(else_branch),
            ExprKind::Match { scrutinee, arms } => {
                e(scrutinee)
                    || arms
                        .iter()
                        .any(|a| e(&a.body) || a.guard.as_ref().is_some_and(e))
            }
            ExprKind::Block { stmts, tail } => {
                stmts.iter().any(|s| match s {
                    Stmt::Let { value, .. } => e(value),
                    Stmt::Expr(x) => e(x),
                }) || tail.as_deref().is_some_and(e)
            }
            ExprKind::Record { fields } => fields.iter().any(|(_, v)| e(v)),
            ExprKind::RecordUpdate { base, fields } => e(base) || fields.iter().any(|(_, v)| e(v)),
            ExprKind::Field { base, .. } => e(base),
            ExprKind::List { items } => items.iter().any(e),
            ExprKind::Try { operand } => e(operand),
            ExprKind::Perform { args, .. } => args.iter().any(e),
            ExprKind::Handle {
                body,
                clauses,
                return_clause,
            } => {
                e(body)
                    || clauses.iter().any(|c| e(&c.body))
                    || return_clause.as_ref().is_some_and(|r| e(&r.body))
            }
            ExprKind::WithCell { init, body, .. } => e(init) || e(body),
            ExprKind::WithRegion { body, .. } | ExprKind::Simulate { body } => e(body),
        })
    }
    m.items.iter().any(|item| match item {
        Item::Fn(d) => e(&d.body) || d.spec.iter().any(|s| e(&s.expr)),
        Item::Test(d) => e(&d.body),
        Item::Law(d) => e(&d.body) || d.guard.as_ref().is_some_and(e),
        _ => false,
    })
}

/// **The invariant four other crates are built on.**
#[test]
fn no_named_argument_survives_resolve_anywhere_in_the_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root");
    let mut files = Vec::new();
    collect_ply(root, &mut files);
    assert!(
        files.len() > 100,
        "only {} `.ply` files found; the walk is not reaching the corpus",
        files.len()
    );

    const USES_IT: &str = "\
fn greet(name: String, greeting: String = \"hello\", mark: String = \"!\") -> String =
  string_concat(greeting, string_concat(name, mark))
fn a() -> String = greet(\"ada\")
fn b() -> String = greet(\"ada\", \"hi\")
fn c() -> String = greet(\"ada\", greeting: \"hey\")
fn d() -> String = greet(\"ada\", mark: \"?\", greeting: \"yo\")
fn e() -> Unit = assert(a() == b(), Some(\"differ\"))
fn f() -> Unit = assert(a() == b(), message: Some(\"differ\"))
fn g() -> List<String> = [greet(\"x\"), greet(\"y\", mark: \".\")]
test \"t\" { assert_eq(a(), \"helloada!\") }
";

    let mut sources: Vec<(String, String)> = files
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|s| (p.display().to_string(), s))
        })
        .collect();
    sources.push(("<uses named arguments>".to_string(), USES_IT.to_string()));

    let mut saw_the_syntax = false;
    for (name, source) in &sources {
        let Ok(module) = parse(SRC, source) else {
            continue;
        };
        if has_named_argument(&module) {
            saw_the_syntax = true;
        }
        let mut program = Program::single(module);
        let Ok(_) = crate::resolve(&mut program) else {
            continue;
        };
        for module in &program.modules {
            assert!(
                !has_named_argument(module),
                "a named argument survived `resolve` from {name}"
            );
        }
    }
    assert!(
        saw_the_syntax,
        "no source in this run carried a named argument into `resolve`, so the guard \
         proved nothing"
    );
}

/// The other half: a call that omitted an argument really did gain the default, rather than being
/// left short for someone else to trip over.
#[test]
fn an_omitted_argument_is_filled_with_the_default() {
    let module = parse(
        SRC,
        "fn greet(name: String, greeting: String = \"hello\") -> String =\n  \
         string_concat(greeting, name)\n\
         fn a() -> String = greet(\"ada\")\n\
         fn b() -> String = greet(\"ada\", greeting: \"hey\")\n",
    )
    .expect("it parses");
    let mut program = Program::single(module);
    crate::resolve(&mut program).expect("it resolves");
    let items = &program.modules[0].items;
    let Item::Fn(a) = &items[1] else {
        panic!("expected `a`")
    };
    let Item::Fn(b) = &items[2] else {
        panic!("expected `b`")
    };
    assert_eq!(dump_expr(&a.body), "(call greet \"ada\" \"hello\")");
    assert_eq!(dump_expr(&b.body), "(call greet \"ada\" \"hey\")");
}

/// **`parse_unexpanded` is for one spike, and this is what keeps it there.**
#[test]
fn parse_unexpanded_is_reached_by_no_shipping_caller() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root");
    let crates = root.join("crates");

    // Where the name is allowed: the definition, and this test.
    let allowed = [
        crates.join("ply-syntax/src/parser.rs"),
        crates.join("ply-syntax/src/lib.rs"),
        crates.join("ply-syntax/src/tests.rs"),
    ];

    fn collect_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let mut files = Vec::new();
    collect_rs(&crates, &mut files);
    assert!(
        files.len() > 100,
        "only {} `.rs` files found under {}; the walk is not reaching the workspace and \
         this test would pass over anything",
        files.len(),
        crates.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    let mut saw_the_definition = false;
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if !text.contains("parse_unexpanded") {
            continue;
        }
        if allowed.iter().any(|a| a == path) {
            saw_the_definition = true;
            continue;
        }
        offenders.push(path.display().to_string());
    }
    assert!(
        saw_the_definition,
        // ASCII only, and not by preference: `spikes/ply-parser/mine-fixtures.py` mines every
        // string literal in this file into its fixture bundle and asserts each is printable ASCII,
        // so an em dash here stops the corpus generator with a bare `AssertionError`.
        "no file under {} names `parse_unexpanded`, so either it has been renamed or this \
         walk stopped reaching the crate that defines it; either way the check below is \
         vacuous",
        crates.display()
    );
    assert!(
        offenders.is_empty(),
        "`parse_unexpanded` hands out a tree holding `ExprKind::Try` and \
         `ExprKind::RecordUpdate`, which every crate downstream of `ply-syntax` treats as \
         `unreachable!()`. It exists for `spikes/ply-parser` and for nothing else. \
         These files name it: {offenders:?}"
    );
}
