//! The bit surface's type surface: `Int` only, and the diagnostics that say so.

use crate::fixture::compile;
use ply_core::{CheckOutput, print_type};
use ply_span::{Diagnostic, Symbol, codes};

fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_type(&out.defs[&Symbol::new(format!("m.{name}"))].scheme.ty)
}

fn footprint(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(format!("m.{name}"))]
        .footprint
        .to_string()
}

/// Every one of the six, at `Int`, with the answer at `Int` too.
#[test]
fn the_bit_operators_are_defined_at_int() {
    let out = ok(r#"
fn and(a: Int, b: Int) -> Int = a & b
fn or(a: Int, b: Int) -> Int = a | b
fn xor(a: Int, b: Int) -> Int = a ^ b
fn shl(a: Int, b: Int) -> Int = a << b
fn shr(a: Int, b: Int) -> Int = a >> b
fn ushr(a: Int, b: Int) -> Int = a >>> b
fn not(a: Int) -> Int = ~a
"#);
    for name in ["and", "or", "xor", "shl", "shr", "ushr"] {
        assert_eq!(sig(&out, name), "(Int, Int) -> Int", "`{name}`");
    }
    assert_eq!(sig(&out, "not"), "(Int) -> Int");
}

/// A signature is written but a *body* is inferred, and the operators are what pins it
#[test]
fn an_operator_pins_an_inferred_binder_to_int() {
    let out = ok("fn mix(xs: List<Int>, seed: Int) -> Int = fold(xs, seed, |acc, x| acc ^ x)\n");
    assert_eq!(sig(&out, "mix"), "(List<Int>, Int) -> Int");
}

/// `&` between two `Bool`s is a refusal that names both, not a short-circuit spelled differently.
#[test]
fn a_bit_operator_over_two_bools_names_both_sides() {
    let d = errors("fn bad(a: Bool, b: Bool) -> Int = a & b\n");
    // The operand diagnostics only: a `&` answers its operands' type, so a `Bool` one also fails
    // against the written `-> Int`, and that third diagnostic is a cascade rather than a side.
    let mismatches: Vec<&Diagnostic> = d
        .iter()
        .filter(|d| d.code == codes::TYPE_MISMATCH && d.message.contains("operand of `&`"))
        .collect();
    assert_eq!(
        mismatches.len(),
        2,
        "each side is judged on its own, so two `Bool`s are two diagnostics: {d:#?}"
    );
    assert!(
        mismatches[0].message.contains("left operand of `&`"),
        "{:#?}",
        mismatches[0]
    );
    assert!(
        mismatches[1].message.contains("right operand of `&`"),
        "{:#?}",
        mismatches[1]
    );
    let spans: Vec<_> = mismatches
        .iter()
        .filter_map(|d| d.labels.first().map(|l| l.span))
        .collect();
    assert_ne!(
        spans[0], spans[1],
        "both diagnostics point at the same operand, so one side is unnamed"
    );
}

/// The note is what turns a mismatch into an answer, and `Bool` is the only wrong type that gets
#[test]
fn the_bool_mismatch_says_which_operators_bool_has() {
    let d = errors("fn bad(a: Bool, b: Bool) -> Int = a & b\n");
    assert!(
        d.iter().any(|d| d
            .notes
            .iter()
            .any(|n| n.contains("`&&`") && n.contains("`||`") && n.contains("`!`"))),
        "a `Bool` operand must be told where its operators are: {d:#?}"
    );

    let d = errors("fn bad(a: Float, b: Float) -> Int = a & b\n");
    assert!(d.iter().any(|d| d.code == codes::TYPE_MISMATCH), "{d:#?}");
    assert!(
        !d.iter().any(|d| d.notes.iter().any(|n| n.contains("`&&`"))),
        "a `Float` has no logical operators to be redirected to: {d:#?}"
    );
}

/// Neither of the two numeric types with no bit pattern is admitted, and the shifts are refused on
/// the count as well as on the value.
#[test]
fn float_and_decimal_have_no_bits() {
    for source in [
        "fn bad(a: Float, b: Float) -> Int = a | b\n",
        "fn bad(a: Decimal, b: Decimal) -> Int = a ^ b\n",
        "fn bad(a: Int, b: Float) -> Int = a << b\n",
        "fn bad(a: Float) -> Int = ~a\n",
        "fn bad(a: String, b: String) -> Int = a & b\n",
    ] {
        let d = errors(source);
        assert!(
            d.iter().any(|d| d.code == codes::TYPE_MISMATCH),
            "{source} checked: {d:#?}"
        );
    }
}

/// A bit operator is not a comparison, which is what the arm it would otherwise have fallen through
/// to would have made it.
#[test]
fn a_bit_operator_answers_an_int() {
    let d = errors("fn bad(a: Int, b: Int) -> Bool = a & b\n");
    assert!(
        d.iter().any(|d| d.code == codes::TYPE_MISMATCH),
        "`&` answered a `Bool`: {d:#?}"
    );
}

/// `~` is the third prefix operator and the only one that is not `Bool` or numeric-polymorphic.
#[test]
fn bitwise_not_is_int_to_int() {
    let out = ok("fn flip(a: Int) -> Int = ~a\n");
    assert_eq!(sig(&out, "flip"), "(Int) -> Int");
    let d = errors("fn bad(a: Bool) -> Bool = ~a\n");
    assert!(d.iter().any(|d| d.code == codes::TYPE_MISMATCH), "{d:#?}");
}

/// Precedence is the parser's, but a wrong table shows up as a type error first.
#[test]
fn the_bit_operators_bind_tighter_than_comparison() {
    ok("fn f(a: Int, b: Int, c: Int) -> Bool = a & b == c\n");
    ok("fn g(a: Int, b: Int, c: Int) -> Bool = a << b < c\n");
}

/// The three that answer where `+`, `-` and `*` raise.
#[test]
fn the_wrapping_builtins_have_the_type_the_adr_states() {
    let out = ok(r#"
fn probe_add() -> (Int, Int) -> Int = wrap_add
fn probe_sub() -> (Int, Int) -> Int = wrap_sub
fn probe_mul() -> (Int, Int) -> Int = wrap_mul
"#);
    for name in ["probe_add", "probe_sub", "probe_mul"] {
        assert_eq!(sig(&out, name), "() -> (Int, Int) -> Int", "`{name}`");
    }
}

/// The gate for a *third* argument, exactly as in `list_builtins.rs`:
#[test]
fn a_third_argument_to_a_wrapping_builtin_is_refused_by_the_scheme() {
    for source in [
        "fn bad() -> Int = wrap_add(1, 2, 3)\n",
        "fn bad() -> Int = wrap_sub(1, 2, 3)\n",
        "fn bad() -> Int = wrap_mul(1, 2, 3)\n",
        "fn bad() -> Int = wrap_add(1)\n",
    ] {
        let d = errors(source);
        assert!(
            !d.is_empty(),
            "{source} checked, so the scheme and `Builtin::arity()` disagree"
        );
    }
}

/// They are `Int`-only and pure.
#[test]
fn the_wrapping_builtins_are_int_only_and_pure() {
    let d = errors("fn bad(a: Float, b: Float) -> Float = wrap_add(a, b)\n");
    assert!(d.iter().any(|d| d.code == codes::TYPE_MISMATCH), "{d:#?}");

    let out = ok(r#"
effect tell { write say[out](what: String) -> Unit }

fn quiet(a: Int, b: Int) -> Int = wrap_mul(a, b)
fn loud(a: Int) -> Int = wrap_mul(a, { tell.say[out]("x"); 2 })
"#);
    assert_eq!(footprint(&out, "quiet"), "{}");
    assert_eq!(footprint(&out, "loud"), "{m.tell.write[out]}");
}

/// A bit operator publishes nothing of its own either, and both operands are evaluated because Ply
/// is strict — `&` is not `&&`.
#[test]
fn a_bit_operator_publishes_only_its_operands_row() {
    let out = ok(r#"
effect tell { write say[out](what: String) -> Unit }

fn quiet(a: Int, b: Int) -> Int = a & b
fn loud(a: Int) -> Int = a & { tell.say[out]("x"); 1 }
"#);
    assert_eq!(footprint(&out, "quiet"), "{}");
    assert_eq!(footprint(&out, "loud"), "{m.tell.write[out]}");
}

/// None of the four names is reserved, which is the the list index decision hazard restated:
#[test]
fn a_module_may_declare_its_own_wrapping_function() {
    let out = ok(
        "fn wrap_add(a: String, b: String) -> String = a ++ b\nfn use() -> String = wrap_add(\"a\", \"b\")\n",
    );
    assert_eq!(sig(&out, "wrap_add"), "(String, String) -> String");
}
