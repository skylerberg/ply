//! The list index's type surface, at the source level.

use ply_core::{CheckOutput, check_program, print_type};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&mut program)?;
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

/// `fn probe_at<a>() -> T = list_at` returns the builtin itself, so the printed signature of the
/// probe carries the builtin's whole type.
#[test]
fn list_at_has_the_type_the_contract_states() {
    let want = "(List<a>, Int) -> Option<a>";
    let out = ok(&format!("fn probe_at<a>() -> {want} = list_at\n"));
    assert_eq!(sig(&out, "probe_at"), format!("() -> {want}"));
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

/// This is the gate for a *third* argument.
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

/// `list_at` is pure.
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

/// A negative index is a type-correct `Int`, so nothing here refuses one.
#[test]
fn a_negative_index_is_a_type_error_nowhere() {
    ok("fn go(xs: List<Int>) -> Option<Int> = list_at(xs, 0 - 1)\n");
    ok("fn go(xs: List<Int>, i: Int) -> Option<Int> = list_at(xs, i)\n");
}

/// The name is not reserved.
#[test]
fn the_name_is_not_reserved_so_a_module_may_declare_its_own() {
    let out = ok("fn list_at<a>(xs: List<a>, i: Int) -> Int = 0\n");
    assert_eq!(sig(&out, "list_at"), "(List<a>, Int) -> Int");
}
