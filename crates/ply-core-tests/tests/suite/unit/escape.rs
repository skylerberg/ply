use ply_core::escape::*;
use ply_core::{Span, codes};

/// The boundary a handle leaves by rather than enters by: it names the definition that answered,
/// the way to the handle, and what to answer instead.
#[test]
fn an_entry_points_answer_refuses_a_continuation_by_the_way_to_it() {
    let boundary = Boundary::EntryResult { name: "m.parked" };
    let escapee = Escapee {
        handle: Handle::Continuation,
        route: vec!["item 1".into(), "`m.Just`'s argument 1".into()],
    };
    let d = rejected(&boundary, &escapee, Span::DUMMY);
    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY);
    assert_eq!(
        d.message,
        "`m.parked` answered a continuation, reached through item 1 → `m.Just`'s argument 1"
    );
    assert_eq!(d.labels[0].message, "answered here");
    assert!(
        d.notes.iter().any(|n| n.contains("goes to its caller")),
        "{:#?}",
        d.notes
    );
    assert!(
        (d.notes.iter()).any(|n| n.contains("resume the continuation before the entry answers")),
        "{:#?}",
        d.notes
    );
}

#[test]
fn a_handle_reached_directly_names_no_way() {
    let escapee = Escapee {
        handle: Handle::Task,
        route: Vec::new(),
    };
    assert_eq!(escapee.reached(), "");
    let d = rejected(
        &Boundary::EntryResult { name: "m.spawned" },
        &escapee,
        Span::DUMMY,
    );
    assert_eq!(d.message, "`m.spawned` answered a `Task`");
}
