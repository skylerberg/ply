//! Why the leak ADR 0017 §4 accepts is not reachable yet, pinned so that the day
//! it becomes reachable is a failing test rather than a silent leak.
//!
//! §4 accepts that a cycle among escaped values leaks and asks the diagnostics
//! to say so where one is constructible. `ply_eval::rc::cell_cycle` is that
//! diagnostic and it fires on the one shape that would leak — a cell whose
//! contents reach the cell. What this file records is that **no type-correct
//! program writes that shape**, for two independent reasons, neither of which
//! belongs to reference counting:
//!
//! - a `Cell<T>` inside `T` written structurally is the infinite type the occurs
//!   check refuses;
//! - written as a declared variant it is `REGION_ESCAPE` at the declaration,
//!   because a declared field's region would be pinned by whichever cell reached
//!   it first (ADR 0005's demoted region check, still load-bearing here).
//!
//! Take either away and the leak arrives. That is why the argument is a test and
//! not a paragraph.

use ply_core::check_program;
use ply_span::{Diagnostic, SourceId, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn rejected(src: &str) -> Vec<Diagnostic> {
    let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
    let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
    let resolved = resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
    match check_program(&program, &resolved) {
        Ok(_) => Vec::new(),
        Err(diags) => diags,
    }
}

/// Storing the cell inside itself structurally asks for `T = List<Cell<T>>`.
#[test]
fn a_cell_cannot_be_stored_in_a_list_it_holds() {
    let diags = rejected(
        r#"
test "a cell that reaches itself" {
  with_cell[log]([]) { c -> {
    cell_set(c, [c]);
    assert_eq(1, 1)
  } }
}
"#,
    );
    assert!(
        diags.iter().any(|d| d.code == codes::OCCURS_CHECK),
        "a cell stored inside its own contents would be the leak §4 accepts, and the occurs \
         check is what stops it being written: {diags:#?}"
    );
}

/// And declaring a type to hold the cell moves the refusal to the declaration,
/// which is the other half of why the shape has nowhere to be written.
#[test]
fn a_declared_field_cannot_hold_a_cell_for_a_cycle_to_run_through() {
    let diags = rejected(
        r#"
type Loop = Nil | Node(Cell<Loop>)

test "a cell that reaches itself through a variant" {
  with_cell[log](Nil) { c -> {
    cell_set(c, Node(c));
    assert_eq(1, 1)
  } }
}
"#,
    );
    assert!(
        diags.iter().any(|d| d.code == codes::REGION_ESCAPE),
        "a declared `Cell` field is what a cycle would have to run through: {diags:#?}"
    );
}

/// The detector itself still answers, so the guard is not vacuous code that
/// stopped working while nothing could reach it.
///
/// Driven through the evaluator's own value representation rather than through
/// source, which is the only way to build the shape the type system refuses.
#[test]
fn the_detector_still_finds_the_shape_it_guards_against() {
    use ply_eval::TaskRegions;
    use ply_span::Span;

    let mut regions = TaskRegions::new();
    let id = regions.alloc_cell(ply_eval::Value::Unit);
    let held = ply_eval::Value::list(vec![ply_eval::Value::Cell(id)]);

    ply_eval::rc::reset();
    let before = ply_eval::rc::stats().cycles;
    let _ = ply_eval::builtins::call(
        ply_eval::Builtin::CellSet,
        vec![ply_eval::Value::Cell(id), held],
        &mut regions,
        Span::DUMMY,
    );
    assert_eq!(
        ply_eval::rc::stats().cycles,
        before + 1,
        "the guard stopped recognizing the one shape it exists for"
    );
    let reported = ply_eval::rc::take_cycles();
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].code, codes::REFERENCE_CYCLE);
    assert!(
        reported[0]
            .notes
            .iter()
            .any(|n| n.contains("does not collect cycles")),
        "the warning must say why nothing will free it: {:?}",
        reported[0].notes
    );
}
