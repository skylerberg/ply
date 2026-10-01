use crate::fixture::Compiled;
use ply_eval::codes;

/// Storing the cell inside itself structurally asks for `T = List<Cell<T>>`.
#[test]
fn a_cell_cannot_be_stored_in_a_list_it_holds() {
    let diags = Compiled::rejected(
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
        "a cell stored inside its own contents would be the leak the reference-counting pass accepts, and the occurs \
         check is what stops it being written: {diags:#?}"
    );
}

#[test]
fn a_declared_field_cannot_hold_a_cell_for_a_cycle_to_run_through() {
    let diags = Compiled::rejected(
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

/// The guard compiled code runs before `cell_set` stores a value.
#[test]
fn the_detector_still_finds_the_shape_it_guards_against() {
    use ply_codegen::heap::{self, Heap};
    use ply_eval::{Span, TaskRegions, Value};

    let mut regions = TaskRegions::new();
    let id = regions.alloc_cell(Value::Unit);
    let mut words = Heap::new();
    let cell = words.bridge(Value::Cell(id));
    let held = words.list_from(&[cell]);
    assert!(
        heap::reaches_cell(held, id),
        "the guard stopped recognizing the one shape it exists for"
    );

    ply_eval::rc::reset();
    let before = ply_eval::rc::stats().cycles;
    ply_eval::rc::note_cell_cycle(id, Span::DUMMY);
    assert_eq!(ply_eval::rc::stats().cycles, before + 1);
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
