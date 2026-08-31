//! Inference over the three numeric types, and the prelude's ADTs.
//!
//! Ply has no type-directed dispatch, so `+` cannot be resolved by looking at
//! the operand type at the node — it is usually still a variable there. The
//! operand type is unified across the two sides on sight and *which* numeric
//! type it is settled once the enclosing definition has been inferred, which is
//! what these tests pin down: the settlement has to happen before generalization
//! and after every body in a recursive component.

use crate::check_program;
use crate::print::print_scheme;
use ply_span::{Diagnostic, SourceId, codes};
use ply_syntax::ast::ModuleName;

fn check(source: &str) -> Result<crate::CheckOutput, Vec<Diagnostic>> {
    let mut program =
        ply_syntax::parse_program([(SourceId(0), ModuleName::from_dotted("m"), source)])
            .unwrap_or_else(|d| panic!("did not parse: {d:#?}"));
    let resolved =
        ply_syntax::resolve(&mut program).unwrap_or_else(|d| panic!("did not resolve: {d:#?}"));
    check_program(&program, &resolved)
}

#[track_caller]
fn ok(source: &str) -> crate::CheckOutput {
    check(source).unwrap_or_else(|d| {
        panic!(
            "expected a clean check, got {:#?}",
            d.iter()
                .map(|d| format!("{} {}", d.code, d.message))
                .collect::<Vec<_>>()
        )
    })
}

#[track_caller]
fn errors(source: &str) -> Vec<Diagnostic> {
    check(source).expect_err("expected a diagnostic")
}

#[track_caller]
fn code(source: &str) -> &'static str {
    errors(source)[0].code
}

#[track_caller]
fn scheme(source: &str, name: &str) -> String {
    let out = ok(source);
    let def = out
        .defs
        .get(&ply_span::Symbol::new(name))
        .unwrap_or_else(|| panic!("`{name}` was not checked"));
    print_scheme(&def.scheme)
}

// -- Operator overloading ---------------------------------------------------

#[test]
fn arithmetic_works_at_each_of_the_three_numeric_types() {
    assert_eq!(
        scheme("pub fn f(a: Int, b: Int) -> Int = a + b * 2", "m.f"),
        "(Int, Int) -> Int"
    );
    assert_eq!(
        scheme("pub fn f(a: Float, b: Float) -> Float = a + b * 2.0", "m.f"),
        "(Float, Float) -> Float"
    );
    assert_eq!(
        scheme(
            "pub fn f(a: Decimal, b: Decimal) -> Decimal = a + b * 2m",
            "m.f"
        ),
        "(Decimal, Decimal) -> Decimal"
    );
}

/// The operand type is often unknown at the node and known three tokens later,
/// which is why the decision is deferred rather than taken on sight.
#[test]
fn the_operand_type_is_learned_from_either_side() {
    assert_eq!(
        scheme("pub fn f(a) -> Float = a + 1.0", "m.f"),
        "(Float) -> Float"
    );
    assert_eq!(
        scheme("pub fn f(a) -> Decimal = 1m + a", "m.f"),
        "(Decimal) -> Decimal"
    );
    assert_eq!(
        scheme("pub fn f(a) -> Bool = a < 1.5", "m.f"),
        "(Float) -> Bool"
    );
}

/// A caller inside the same recursive component can be what pins a callee's
/// operand type, so settling one body at a time would decide `Int` before the
/// other body said `Decimal`.
#[test]
fn a_recursive_component_settles_after_every_member() {
    assert_eq!(
        scheme(
            "pub fn total(xs: List<Decimal>, acc) -> Decimal =\n\
             \x20 match xs { [] -> acc, [x, ..rest] -> total(rest, acc + x) }",
            "m.total"
        ),
        "(List<Decimal>, Decimal) -> Decimal"
    );
}

/// An operand type nothing pinned defaults to `Int`, **before** generalization.
/// Leaving it open would publish `add` as `<t>(t, t) -> t`, a signature whose
/// body works at three types and whose declaration claims every one.
#[test]
fn an_unconstrained_operand_defaults_to_int_rather_than_generalizing() {
    assert_eq!(
        scheme("pub fn add(a, b) = a + b", "m.add"),
        "(Int, Int) -> Int"
    );
    assert_eq!(scheme("pub fn neg(a) = 0 - a", "m.neg"), "(Int) -> Int");
}

/// The one place W2 refuses what every other language allows.
#[test]
fn decimal_division_is_e0209_and_names_its_replacement() {
    let diags = errors("pub fn unit(total: Decimal, count: Decimal) -> Decimal = total / count");
    assert_eq!(diags[0].code, codes::DECIMAL_DIVISION);
    assert!(
        diags[0]
            .notes
            .iter()
            .any(|n| n.contains("decimal_div") && n.contains("HalfEven")),
        "the diagnostic has to say what to call instead: {:?}",
        diags[0].notes
    );
    // The span is the operator's expression, not the whole definition.
    assert!(diags[0].primary_span().is_some());
}

/// `%` is allowed where `/` is not: the remainder of a decimal division is a
/// decimal even when the quotient is not.
#[test]
fn decimal_remainder_is_accepted() {
    ok("pub fn f(a: Decimal, b: Decimal) -> Decimal = a % b");
}

/// `/` at the other two numeric types is untouched.
#[test]
fn division_is_refused_only_at_decimal() {
    ok("pub fn f(a: Int, b: Int) -> Int = a / b");
    ok("pub fn f(a: Float, b: Float) -> Float = a / b");
}

#[test]
fn arithmetic_on_a_non_numeric_type_names_the_three_that_work() {
    let diags = errors("pub fn f(a: String, b: String) -> String = a + b");
    assert_eq!(diags[0].code, codes::TYPE_MISMATCH);
    assert!(
        diags[0].message.contains("`+`") && diags[0].message.contains("String"),
        "{}",
        diags[0].message
    );
    assert!(
        diags[0]
            .notes
            .iter()
            .any(|n| n.contains("Int") && n.contains("Float") && n.contains("Decimal")),
        "{:?}",
        diags[0].notes
    );
}

/// Mixing two numeric types is one diagnostic about the operands, not two.
#[test]
fn mixing_two_numeric_types_is_reported_once() {
    let diags = errors("pub fn f(a: Int, b: Float) -> Int = a + b");
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].code, codes::TYPE_MISMATCH);
}

#[test]
fn ordered_comparison_works_at_the_numeric_types_and_nowhere_else() {
    ok("pub fn f(a: Int, b: Int) -> Bool = a < b");
    ok("pub fn f(a: Float, b: Float) -> Bool = a >= b");
    ok("pub fn f(a: Decimal, b: Decimal) -> Bool = a <= b");
    assert_eq!(
        code("pub fn f(a: Bool, b: Bool) -> Bool = a < b"),
        codes::TYPE_MISMATCH
    );
}

#[test]
fn negation_is_defined_at_each_numeric_type() {
    assert_eq!(
        scheme("pub fn f(a: Float) -> Float = -a", "m.f"),
        "(Float) -> Float"
    );
    assert_eq!(
        scheme("pub fn f(a: Decimal) -> Decimal = -a", "m.f"),
        "(Decimal) -> Decimal"
    );
    assert_eq!(
        scheme("pub fn f(a: Int) -> Int = -a", "m.f"),
        "(Int) -> Int"
    );
    assert_eq!(
        code("pub fn f(a: String) -> String = -a"),
        codes::TYPE_MISMATCH
    );
}

/// Three types, so a literal of one is not a literal of another. Without this
/// the deferral would have nothing to settle.
#[test]
fn the_three_literal_forms_have_three_types() {
    assert_eq!(scheme("pub fn f() -> Int = 1", "m.f"), "() -> Int");
    assert_eq!(scheme("pub fn f() -> Float = 1.0", "m.f"), "() -> Float");
    assert_eq!(scheme("pub fn f() -> Decimal = 1m", "m.f"), "() -> Decimal");
    assert_eq!(code("pub fn f() -> Int = 1.0"), codes::TYPE_MISMATCH);
    assert_eq!(code("pub fn f() -> Float = 1m"), codes::TYPE_MISMATCH);
}

// -- The prelude's ADTs -----------------------------------------------------

/// A builtin whose type mentions a type the user has to import first would be
/// incoherent, so these four are in scope with no declaration anywhere.
#[test]
fn the_prelude_adts_are_in_scope_without_a_declaration() {
    ok("pub fn f(d: Decimal) -> Option<Int> = int_of_decimal(d, HalfEven)");
    ok("pub fn f() -> Ordering = Less");
    ok("pub fn f(x: Int) -> Result<Int, String> = Ok(x)");
    ok("pub fn f() -> Rounding = Ceiling");
    ok("pub fn f() -> Option<Int> = None");
}

#[test]
fn a_prelude_adt_can_be_matched_and_is_checked_for_exhaustiveness() {
    ok("pub fn f(o: Option<Int>) -> Int = match o { Some(x) -> x, None -> 0 }");
    assert_eq!(
        code("pub fn f(o: Option<Int>) -> Int = match o { Some(x) -> x }"),
        codes::NON_EXHAUSTIVE_MATCH
    );
    assert_eq!(
        code("pub fn f(r: Rounding) -> Int = match r { HalfEven -> 1, Down -> 2 }"),
        codes::NON_EXHAUSTIVE_MATCH
    );
}

/// A language with two `Option`s is worse than one with none.
#[test]
fn redeclaring_a_prelude_adt_is_a_duplicate_definition() {
    assert_eq!(
        code("pub type Option<a> = None | Some(a)"),
        codes::DUPLICATE_DEFINITION
    );
    assert_eq!(
        code("pub type Ordering = Less"),
        codes::DUPLICATE_DEFINITION
    );
}

#[test]
fn the_prelude_adts_carry_their_declared_arity() {
    assert_eq!(
        code("pub fn f() -> Option<Int, Int> = None"),
        codes::ARITY_MISMATCH
    );
    assert_eq!(
        code("pub fn f() -> Result<Int> = Ok(1)"),
        codes::ARITY_MISMATCH
    );
}

// -- The numeric builtins ---------------------------------------------------

#[test]
fn the_decimal_builtins_have_the_signatures_the_contract_names() {
    let out = ok("pub fn f() -> Int = 1");
    let _ = out;
    for (name, want) in [
        (
            "decimal_div",
            "(Decimal, Decimal, Int, Rounding) -> Decimal",
        ),
        ("decimal_round", "(Decimal, Int, Rounding) -> Decimal"),
        ("decimal_of_int", "(Int) -> Decimal"),
        ("int_of_decimal", "(Decimal, Rounding) -> Option<Int>"),
        ("float_of_decimal", "(Decimal) -> Float"),
        ("decimal_of_float", "(Float) -> Option<Decimal>"),
        ("decimal_of_string", "(String) -> Option<Decimal>"),
        ("decimal_to_string", "(Decimal) -> String"),
    ] {
        // Checking through a definition that names it is what proves the builtin
        // is reachable *and* has the type, in one step.
        let source = format!("pub fn f() -> Int = 1\npub fn g() = {name}");
        let printed = scheme(&source, "m.g");
        assert_eq!(printed, format!("() -> {want}"), "`{name}`");
    }
}

/// A scale is an argument rather than a default, and a rounding mode is a value
/// the caller writes down. That is the whole content of refusing `/`.
#[test]
fn decimal_div_will_not_typecheck_without_a_rounding_mode() {
    assert_eq!(
        code("pub fn f(a: Decimal, b: Decimal) -> Decimal = decimal_div(a, b, 2)"),
        codes::ARITY_MISMATCH
    );
    assert_eq!(
        code("pub fn f(a: Decimal, b: Decimal) -> Decimal = decimal_div(a, b, 2, 0)"),
        codes::TYPE_MISMATCH
    );
}

/// The property generator builds its `TypeWorld` from `CheckOutput::ctors`, so
/// a prelude ADT that never reached that map would be `E0418` — a law over an
/// `Option` nobody can check.
#[test]
fn the_prelude_adts_reach_the_check_output() {
    let out = ok("pub fn f() -> Option<Int> = None");
    for ctor in ["None", "Some", "Ok", "Err", "Less", "HalfEven"] {
        assert!(
            out.ctors.contains_key(&ply_span::Symbol::new(ctor)),
            "`{ctor}` is missing from CheckOutput::ctors"
        );
    }
    let some = &out.ctors[&ply_span::Symbol::new("Some")];
    assert_eq!(some.type_name.as_str(), "Option");
    assert_eq!(some.arity, 1);
    assert_eq!(some.scheme.ty_vars.len(), 1);
}
