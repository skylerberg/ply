use ply_core::Footprint;
use ply_span::{SourceId, Span, Symbol};
use ply_test::slice::{CausalSlice, Entered, Event, Frame, SliceBuilder};

fn frame(name: &str) -> Frame {
    Frame {
        name: Symbol::new(name),
        hash: None,
        call_site: Span::new(SourceId(0), 0, 1),
    }
}

fn slice() -> CausalSlice {
    CausalSlice {
        traced: true,
        reproduced: true,
        entered: ["outer", "middle", "inner", "returned_already"]
            .iter()
            .map(|n| Entered {
                name: Symbol::new(n),
                hash: None,
                calls: 1,
            })
            .collect(),
        stack: vec![frame("outer"), frame("middle"), frame("inner")],
        observed: Footprint::empty(),
        truncated: false,
    }
}

#[test]
fn depth_counts_up_from_the_failure() {
    let s = slice();
    assert_eq!(s.depth_of(&Symbol::new("inner")), Some(0));
    assert_eq!(s.depth_of(&Symbol::new("outer")), Some(2));
}

/// Everything on the stack ran, but not everything that ran is on the stack.
#[test]
fn a_definition_that_returned_before_the_failure_ran_but_has_no_depth() {
    let s = slice();
    assert!(s.ran(&Symbol::new("returned_already")));
    assert_eq!(s.depth_of(&Symbol::new("returned_already")), None);
}

#[test]
fn an_untraced_slice_claims_nothing_rather_than_claiming_emptiness() {
    let s = CausalSlice::untraced();
    assert!(!s.traced);
    assert!(!s.ran(&Symbol::new("anything")));
    assert!(s.path().is_empty());
}

#[test]
fn recursion_is_visible_as_a_call_count() {
    let mut s = slice();
    s.entered[2].calls = 4096;
    assert_eq!(s.entered[2].calls, 4096);
    assert_eq!(s.depth_of(&Symbol::new("inner")), Some(0));
}

fn enter(name: &str) -> Event {
    Event::Enter {
        name: Symbol::new(name),
        hash: None,
        call_site: Span::new(SourceId(0), 0, 1),
    }
}

fn built(events: &[Event]) -> SliceBuilder {
    let mut b = SliceBuilder::new();
    for e in events {
        b.record(e.clone());
    }
    b
}

/// The two halves of the slice answer different questions, and confusing them ranks a
/// definition that had already returned as if it were where the failure happened.
#[test]
fn the_stack_is_the_path_and_entered_is_everything_that_ran() {
    let mut b = built(&[
        enter("post"),
        enter("format"),
        Event::Return,
        enter("apply_debit"),
    ]);
    b.failed();
    b.record(Event::Return);
    b.record(Event::Return);
    let slice = b.finish(true);

    assert_eq!(
        slice.path(),
        vec![&Symbol::new("post"), &Symbol::new("apply_debit")]
    );
    assert!(slice.ran(&Symbol::new("format")));
    assert_eq!(slice.depth_of(&Symbol::new("format")), None);
    assert_eq!(slice.depth_of(&Symbol::new("apply_debit")), Some(0));
    assert!(!slice.truncated);
}

#[test]
fn a_definition_entered_twice_is_one_row_with_a_count() {
    let mut b = built(&[
        enter("loop_body"),
        Event::Return,
        enter("loop_body"),
        Event::Return,
        enter("loop_body"),
    ]);
    b.failed();
    let slice = b.finish(true);
    assert_eq!(slice.entered.len(), 1);
    assert_eq!(slice.entered[0].calls, 3);
}

/// The cap bounds how many *distinct* definitions are remembered.
#[test]
fn hitting_the_cap_truncates_the_roster_and_not_the_stack() {
    let mut b = SliceBuilder::with_cap(2);
    for name in ["a", "b", "c", "d"] {
        b.record(enter(name));
    }
    b.failed();
    let slice = b.finish(true);

    assert!(slice.truncated);
    assert_eq!(slice.entered.len(), 2);
    assert_eq!(slice.stack.len(), 4);
    assert_eq!(slice.depth_of(&Symbol::new("d")), Some(0));
}

#[test]
fn only_performed_atoms_are_observed() {
    let atom = ply_core::EffectAtom::new(
        "db",
        ply_core::Resource::Named(Symbol::new("users")),
        ply_syntax::ast::Mode::Read,
    );
    let mut b = built(&[enter("f"), Event::Perform(atom.clone())]);
    b.failed();
    let slice = b.finish(true);
    assert_eq!(slice.observed.atoms().collect::<Vec<_>>(), vec![&atom]);
}

/// A failure that is never reported leaves no path, which is different from a path of length
/// zero and has to stay so.
#[test]
fn a_builder_that_was_never_told_of_a_failure_reports_no_stack() {
    let slice = built(&[enter("f")]).finish(true);
    assert!(slice.traced);
    assert!(slice.stack.is_empty());
    assert!(slice.ran(&Symbol::new("f")));
}

#[test]
fn the_first_failure_is_the_one_explained() {
    let mut b = built(&[enter("outer"), enter("inner")]);
    b.failed();
    b.record(Event::Return);
    b.failed();
    assert_eq!(
        b.finish(true).path(),
        vec![&Symbol::new("outer"), &Symbol::new("inner")]
    );
}
