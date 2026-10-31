use ply_core::escape::*;
use ply_core::{Span, codes};

#[test]
fn an_entry_point_returning_a_continuation_is_rejected_and_the_message_gives_the_path_to_it() {
    let boundary = Boundary::ReturnedFromEntry { name: "m.parked" };
    let escapee = EscapedHandle {
        handle: Handle::Continuation,
        route: vec!["item 1".into(), "`m.Just`'s argument 1".into()],
    };
    let d = rejected(&boundary, &escapee, Span::DUMMY);
    assert_eq!(d.code, codes::SCHEDULER_ESCAPE_AT_BOUNDARY);
    assert_eq!(
        d.message,
        "`m.parked` returned a continuation, reached through item 1 → `m.Just`'s argument 1"
    );
    assert_eq!(d.labels[0].message, "returned here");
    assert!(
        d.notes.iter().any(|n| n.contains("goes to its caller")),
        "{:#?}",
        d.notes
    );
    assert!(
        (d.notes.iter()).any(|n| n.contains("resume the continuation before the entry returns")),
        "{:#?}",
        d.notes
    );
}

#[test]
fn a_task_handle_returned_directly_from_an_entry_point_is_rejected_with_a_message_that_has_no_field_path()
 {
    let escapee = EscapedHandle {
        handle: Handle::Task,
        route: Vec::new(),
    };
    assert_eq!(escapee.reached(), "");
    let d = rejected(
        &Boundary::ReturnedFromEntry { name: "m.spawned" },
        &escapee,
        Span::DUMMY,
    );
    assert_eq!(d.message, "`m.spawned` returned a `Task`");
}
