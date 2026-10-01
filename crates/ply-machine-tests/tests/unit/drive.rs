use ply_eval::{Diagnostic, SourceId, Span, codes};
use ply_machine::drive::place_the_unplaced;

#[test]
fn a_raise_with_no_place_names_the_entry_point_it_came_from() {
    let bare = Diagnostic::error(codes::RUNTIME_ERROR, "`len` expects a List or String");
    let placed = place_the_unplaced(bare, "ply.main");
    assert!(
        placed
            .notes
            .iter()
            .any(|n| n.contains("no place in the source") && n.contains("ply.main")),
        "{:?}",
        placed.notes
    );
}

#[test]
fn a_raise_that_has_a_place_is_left_alone() {
    let with_place = Diagnostic::error(codes::RUNTIME_ERROR, "boom")
        .primary(Span::new(SourceId(0), 1, 2), "here");
    let same = place_the_unplaced(with_place, "ply.main");
    assert!(same.notes.is_empty(), "{:?}", same.notes);
}
