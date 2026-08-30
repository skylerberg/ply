//! The list index's type surface, at the source level.
//!
//! Follows `iterate_builtin.rs`, which is where ADR 0022 pinned `iterate`'s.
//! Three things are pinned that nothing else pins.
//!
//! The **signature**, because the `Option` is the whole of ADR 0027's decision:
//! `list_at` is total, so it cannot raise, and that is what puts it in
//! `ply_prove`'s `TOTAL_BUILTINS` where a raising index could not go.
//!
//! The **argument count**, and this file is where a *third* argument is gated.
//! `Builtin::arity()` in `ply-eval` is enforced on every call, but only
//! usefully in one direction: giving `ListAt` an arity of `(2, 3)` leaves
//! `every_builtin_checks_its_argument_count` green, because that test asserts
//! the declared arity is enforced rather than that it is right, and a third
//! argument never gets past the scheme to meet it. (`(1, 1)` is caught, loudly,
//! by every `ply-eval` test that calls the builtin — the hole is arities that
//! are too *wide*, not the table as a whole.) So a third argument to `list_at`
//! is refused here or nowhere.
//!
//! The **purity**, because a peek must publish nothing of its own: the
//! builtin's row is empty and the row a call publishes is entirely its
//! arguments'.
//!
//! # What each test was seen to fail against
//!
//! | corruption in `crates/ply-core/src/infer.rs` | test that went red |
//! | --- | --- |
//! | `list_at`'s return `Type::option(ta)` → `ta` | `list_at_has_the_type_the_contract_states` |
//! | `list_at` given a third parameter `ta` | `a_third_argument_to_list_at_is_refused_by_the_scheme` |
//! | the scheme's row `Row::empty()` → `re.clone()` (+ `e` in `row_vars`) | `the_index_is_pure_and_publishes_only_its_arguments_row` |

use ply_core::{CheckOutput, check_program, print_type};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&program)?;
    check_program(&program, &resolved)
}

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

/// `fn probe_f() = f` returns the builtin itself, so the printed signature of
/// the probe carries the builtin's whole type.
#[test]
fn list_at_has_the_type_the_contract_states() {
    let out = ok("fn probe_at() = list_at\n");
    assert_eq!(sig(&out, "probe_at"), "() -> (List<a>, Int) -> Option<a>");
}

/// The element type is the list's, in both directions.
#[test]
fn the_answer_is_at_the_lists_element_type() {
    let out = ok(r#"
type Row = { id: Int }
fn one(xs: List<Row>) -> Option<Row> = list_at(xs, 0)
fn two(xs: List<String>) -> Option<String> = list_at(xs, 0)
"#);
    assert_eq!(sig(&out, "one"), "(List<{id: Int}>) -> Option<{id: Int}>");
    assert_eq!(sig(&out, "two"), "(List<String>) -> Option<String>");

    let d = errors("fn bad(xs: List<Int>) -> Option<String> = list_at(xs, 0)\n");
    assert!(
        d.iter().any(|d| d.code == codes::TYPE_MISMATCH),
        "an answer at the wrong element type must not check: {d:#?}"
    );
}

/// This is the gate for a *third* argument. `Builtin::arity()` is not: a
/// `(2, 3)` there is enforced consistently and so passes
/// `every_builtin_checks_its_argument_count` while being wrong, and no
/// well-typed call can reach the third slot to notice. An arity that is too
/// narrow is a different matter and `ply-eval`'s tests catch it.
#[test]
fn a_third_argument_to_list_at_is_refused_by_the_scheme() {
    let d = errors("fn bad(xs: List<Int>) -> Option<Int> = list_at(xs, 0, 9)\n");
    assert!(
        !d.is_empty(),
        "`list_at` took a third argument, so its scheme and its arity table disagree"
    );
    let d = errors("fn bad(xs: List<Int>) -> Option<Int> = list_at(xs)\n");
    assert!(
        !d.is_empty(),
        "`list_at` ran without an index, so its scheme and its arity table disagree"
    );
}

/// `list_at` is pure. A call publishes exactly the row of the expressions it
/// was given, and Ply is strict, so an effectful index expression is performed
/// whether or not the index turns out to be in range.
#[test]
fn the_index_is_pure_and_publishes_only_its_arguments_row() {
    let out = ok(r#"
effect tell { write say[out](what: String) -> Unit }

fn quiet(xs: List<Int>) -> Option<Int> = list_at(xs, 0)
fn loud(xs: List<Int>) -> Option<Int> = list_at(xs, { tell.say[out]("x"); 0 })
"#);
    assert_eq!(
        footprint(&out, "quiet"),
        "{}",
        "`list_at` performs nothing of its own"
    );
    assert_eq!(
        footprint(&out, "loud"),
        "{m.tell.write[out]}",
        "an argument's row is the call's"
    );
}

/// A negative index is a type-correct `Int`, so nothing here refuses one. The
/// language's answer to `-1` is `None`, at run time, and that is deliberate:
/// there is no `Nat`, so the alternative to a total index is a raising one.
#[test]
fn a_negative_index_is_a_type_error_nowhere() {
    ok("fn go(xs: List<Int>) -> Option<Int> = list_at(xs, 0 - 1)\n");
    ok("fn go(xs: List<Int>, i: Int) -> Option<Int> = list_at(xs, i)\n");
}

/// The name is not reserved. ADR 0001 as amended by ADR 0012 §A5 reserves
/// exactly three, and `list_at` is not among them — which is what keeps
/// `crates/ply-std/ply/json.ply` and `db.ply`, each of which ships an `nth` of
/// its own, free to keep it, and is why ADR 0027 §6 declines the bare name
/// `nth`.
#[test]
fn the_name_is_not_reserved_so_a_module_may_declare_its_own() {
    let out = ok("fn list_at<a>(xs: List<a>, i: Int) -> Int = 0\n");
    assert_eq!(sig(&out, "list_at"), "(List<a>, Int) -> Int");
}
