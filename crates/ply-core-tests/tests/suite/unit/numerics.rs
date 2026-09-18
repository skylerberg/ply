use ply_core::check_program;
use ply_core::print::print_scheme;
use ply_span::{Diagnostic, SourceId, codes};
use ply_syntax::ast::ModuleName;

fn check(source: &str) -> Result<ply_core::CheckOutput, Vec<Diagnostic>> {
    let mut program =
        ply_syntax::parse_program([(SourceId(0), ModuleName::from_dotted("m"), source)])
            .unwrap_or_else(|d| panic!("did not parse: {d:#?}"));
    let resolved =
        ply_syntax::resolve(&mut program).unwrap_or_else(|d| panic!("did not resolve: {d:#?}"));
    check_program(&program, &resolved)
}

#[track_caller]
fn ok(source: &str) -> ply_core::CheckOutput {
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

#[test]
fn the_operand_type_is_learned_from_either_side() {
    ok("pub fn f() -> Float = { let g = |a| a + 1.0; 0.0 }");
    ok("pub fn f() -> Decimal = { let g = |a| 1m + a; 0m }");
    ok("pub fn f() -> Int = { let g = |a| a < 1.5; 0 }");
}

/// Settling one body at a time would decide `Int` before the other body said `Decimal`.
#[test]
fn a_recursive_component_settles_after_every_member() {
    assert_eq!(
        scheme(
            "pub fn total(xs: List<Decimal>, acc: Decimal) -> Decimal =\n\
             \x20 match xs { [] -> acc, [x, ..rest] -> total(rest, acc + x) }",
            "m.total"
        ),
        "(List<Decimal>, Decimal) -> Decimal"
    );
}

#[test]
fn an_unconstrained_operand_is_e0210_rather_than_defaulting_to_int() {
    assert_eq!(
        code("pub fn f() -> Int = { let g = |a, b| a + b; 0 }"),
        codes::NUMERIC_UNDETERMINED
    );
    assert_eq!(
        code("pub fn f() -> Int = { let g = |a| -a; 0 }"),
        codes::NUMERIC_UNDETERMINED
    );
    // Pinned by a literal on the other side, so it never reaches the arm above.
    ok("pub fn f() -> Int = { let g = |a| 0 - a; 0 }");
}

#[test]
fn an_omitted_signature_is_e0126_and_the_message_carries_the_fix() {
    let diags = errors("pub fn shout(m: String) = string_upper(m)");
    assert_eq!(diags[0].code, codes::MISSING_SIGNATURE);
    assert!(
        diags[0]
            .labels
            .iter()
            .any(|l| l.message.contains("-> String")),
        "the diagnostic has to carry the annotation to write: {:?}",
        diags[0].labels
    );
    assert_eq!(
        code("pub fn f(xs: List<Int>, n) -> Int = n"),
        codes::MISSING_SIGNATURE
    );
}

#[test]
fn an_omitted_effect_row_is_still_inferred() {
    assert_eq!(
        scheme("pub fn f(a: Int, b: Int) -> Int = a + b", "m.f"),
        "(Int, Int) -> Int"
    );
}

#[test]
fn a_handler_clause_binder_and_a_lambda_binder_still_infer() {
    ok("effect log { write emit(m: String) -> Unit }\n\
        pub fn shout(m: String) -> Unit = log.emit(m)\n\
        pub fn run() -> Unit = handle shout(\"hi\") with { log.emit(x) -> () }");
    ok("pub fn f(xs: List<Int>) -> List<Int> = map(xs, |n| n + 1)");
}

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
    assert!(diags[0].primary_span().is_some());
}

/// The remainder of a decimal division is a decimal even when the quotient is not.
#[test]
fn decimal_remainder_is_accepted() {
    ok("pub fn f(a: Decimal, b: Decimal) -> Decimal = a % b");
}

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

#[test]
fn the_three_literal_forms_have_three_types() {
    assert_eq!(scheme("pub fn f() -> Int = 1", "m.f"), "() -> Int");
    assert_eq!(scheme("pub fn f() -> Float = 1.0", "m.f"), "() -> Float");
    assert_eq!(scheme("pub fn f() -> Decimal = 1m", "m.f"), "() -> Decimal");
    assert_eq!(code("pub fn f() -> Int = 1.0"), codes::TYPE_MISMATCH);
    assert_eq!(code("pub fn f() -> Float = 1m"), codes::TYPE_MISMATCH);
}

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
        ("bits_of_float", "(Float) -> Int"),
        ("float_of_bits", "(Int) -> Float"),
    ] {
        let source = format!("pub fn f() -> Int = 1\npub fn g() -> {want} = {name}");
        let printed = scheme(&source, "m.g");
        assert_eq!(printed, format!("() -> {want}"), "`{name}`");
    }
}

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

/// The property generator builds its `TypeWorld` from `CheckOutput::ctors`.
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
