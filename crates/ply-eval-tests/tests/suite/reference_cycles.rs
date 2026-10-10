use ply_eval::codes;

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
