use ply_eval::code::Clause;
use ply_eval::cont::*;
use ply_span::Span;
use ply_span::Symbol;
use ply_syntax::ast::Ident;
use std::rc::Rc;

fn frame(n: i64) -> Frame {
    Frame::FieldAccess {
        field: Ident::new(format!("f{n}"), Span::DUMMY),
        base_span: Span::DUMMY,
    }
}

fn field_of(f: &Frame) -> String {
    match f {
        Frame::FieldAccess { field, .. } => field.name.to_string(),
        _ => panic!("expected a field frame"),
    }
}

fn prompt() -> Rc<Prompt> {
    Rc::new(Prompt {
        clauses: Rc::new(Vec::new()),
        effects: Rc::new(Vec::new()),
        ret: None,
        clause_captures: Vec::new(),
        ret_captures: Rc::from(Vec::new()),
        module: 0,
        span: Span::DUMMY,
    })
}

#[test]
fn a_new_stack_is_done_immediately() {
    assert!(matches!(Stack::new().next(), Next::Done));
    assert!(Stack::new().is_empty());
}

#[test]
fn frames_come_back_innermost_first() {
    let s = Stack::new().push(frame(1)).push(frame(2));
    assert_eq!(s.frames(), 2);
    let Next::Frame(top, rest) = s.next() else {
        panic!("expected a frame");
    };
    assert_eq!(field_of(&top), "f2");
    let Next::Frame(under, rest) = rest.next() else {
        panic!("expected a frame");
    };
    assert_eq!(field_of(&under), "f1");
    assert!(matches!(rest.next(), Next::Done));
}

#[test]
fn popping_a_frame_leaves_the_original_stack_intact() {
    let s = Stack::new().push(frame(1));
    let _ = s.next();
    assert_eq!(s.frames(), 1);
}

#[test]
fn an_exhausted_segment_yields_its_prompt_and_then_the_stack_under_it() {
    let s = Stack::new().push(frame(1)).push_prompt(prompt(), 0);
    let Next::Leave(_, under) = s.next() else {
        panic!("expected to leave the segment");
    };
    assert_eq!(under.segments(), 1);
    assert_eq!(under.frames(), 1);
}

#[test]
fn capture_takes_the_segments_above_and_including_the_handler() {
    let s = Stack::new()
        .push(frame(0))
        .push_prompt(prompt(), 0)
        .push(frame(1))
        .push(frame(2));
    assert_eq!(s.segments(), 2);

    let (k, below) = s.capture(1, 0);
    assert_eq!(k.frames(), 2);
    assert_eq!(k.segments(), 1);
    assert_eq!(below.segments(), 1);
    assert_eq!(below.frames(), 1);
}

#[test]
fn resuming_reinstalls_the_handler_that_delimited_the_capture() {
    let s = Stack::new().push_prompt(prompt(), 0).push(frame(1));
    let (k, below) = s.capture(1, 0);
    assert!(below.prompt().is_none());

    let resumed = below.resume(&k);
    assert!(resumed.prompt().is_some());
    assert_eq!(resumed.frames(), 1);
}

#[test]
fn a_continuation_may_be_resumed_twice_onto_different_stacks() {
    let s = Stack::new().push_prompt(prompt(), 0).push(frame(9));
    let (k, below) = s.capture(1, 0);

    let once = below.resume(&k);
    let twice = below.push(frame(5)).resume(&k);

    assert_eq!(once.frames(), 1);
    assert_eq!(twice.frames(), 2);

    let Next::Frame(a, _) = once.next() else {
        panic!("expected a frame");
    };
    let Next::Frame(b, _) = twice.next() else {
        panic!("expected a frame");
    };
    assert_eq!(field_of(&a), "f9");
    assert_eq!(field_of(&b), "f9");
}

/// `into_next` moves the frame out of its link when nothing else holds it, so a captured
/// segment has to be the thing that stops it.
#[test]
fn popping_a_captured_frame_leaves_the_continuation_able_to_splice_it_again() {
    let s = Stack::new()
        .push_prompt(prompt(), 0)
        .push(frame(1))
        .push(frame(2));
    let (k, below) = s.capture(1, 0);

    let Next::Frame(first, rest) = below.resume(&k).into_next() else {
        panic!("expected a frame");
    };
    assert_eq!(field_of(&first), "f2");
    let Next::Frame(second, _) = rest.into_next() else {
        panic!("expected a frame");
    };
    assert_eq!(field_of(&second), "f1");

    assert_eq!(k.frames(), 2);
    let again = below.resume(&k);
    assert_eq!(again.frames(), 2);
    let Next::Frame(replayed, _) = again.into_next() else {
        panic!("expected a frame");
    };
    assert_eq!(field_of(&replayed), "f2");
}

/// The relative bookkeeping a capture reads off the cut: window-bearing frames say how many
/// slots sit above the captured prompt's push height, and the segment says how many below it
/// belong to the pushing activation.
#[test]
fn a_capture_reads_its_slot_metrics_off_the_frames_it_cuts() {
    let s = Stack::new()
        .push_prompt(prompt(), 3)
        .push(frame(1))
        .push(Frame::Call {
            name: None,
            call_site: Span::DUMMY,
            memo: false,
            callee_window: 4,
            caller_window: 3,
        })
        .push(Frame::Exit {
            callee_window: 2,
            caller_window: 4,
        });
    let (k, _) = s.capture(1, 0);
    assert_eq!(k.cut_deltas(), 6, "4 from the call, 2 from the exit");
    assert_eq!(k.cut_window(), 3, "the prompt was pushed with window 3");
}

/// A stack may hold as many frames as the calls under `DEFAULT_MAX_CALLS` can pend, which no
/// constant caps, so releasing one has to be a loop.
#[test]
fn dropping_a_deep_stack_does_not_recurse_through_the_native_stack() {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let mut s = Stack::new();
            for i in 0..200_000 {
                s = s.pushed(frame(i));
            }
            assert_eq!(s.frames(), 200_000);
            drop(s);
        })
        .expect("failed to spawn")
        .join()
        .expect("dropping the stack overflowed the thread stack");
}

#[test]
fn capture_crosses_every_handler_between_the_perform_and_its_own() {
    let s = Stack::new()
        .push_prompt(prompt(), 0)
        .push(frame(1))
        .push_prompt(prompt(), 0)
        .push(frame(2))
        .push_prompt(prompt(), 0)
        .push(frame(3));

    let (k, below) = s.capture(3, 0);
    assert_eq!(k.segments(), 3);
    assert_eq!(k.frames(), 3);
    assert_eq!(below.segments(), 1);
    assert_eq!(below.frames(), 0);

    assert_eq!(below.resume(&k).segments(), 4);
}

#[test]
fn find_handler_reports_the_innermost_matching_prompt() {
    let effect = Symbol::new("db");
    let op = Symbol::new("get");
    let clause = |resource: Option<&str>| Clause {
        effect: ply_syntax::ast::QName::bare(Ident::new("db", Span::DUMMY)),
        op: op.clone(),
        resource: resource.map(Symbol::new),
        params: Rc::new(Vec::new()),
        resume: None,
        body: ply_eval::code::lower(&crate::unit::build::int(0)).code,
        size: 0,
        captures: ply_eval::code::no_captures(),
        span: Span::DUMMY,
    };
    let with = |c: Clause| {
        Rc::new(Prompt {
            clauses: Rc::new(vec![c]),
            effects: Rc::new(vec![effect.clone()]),
            ret: None,
            clause_captures: vec![Rc::from(Vec::new())],
            ret_captures: Rc::from(Vec::new()),
            module: 0,
            span: Span::DUMMY,
        })
    };

    let s = Stack::new()
        .push_prompt(with(clause(Some("users"))), 0)
        .push_prompt(with(clause(Some("orders"))), 0);

    let users = Symbol::new("users");
    let found = s
        .find_handler(&effect, &op, Some(&users))
        .expect("the outer handler matches");
    assert_eq!(found.segments, 2);
    assert!(matches!(found.target, Target::Ply { clause: 0, .. }));

    let orders = Symbol::new("orders");
    let inner = s
        .find_handler(&effect, &op, Some(&orders))
        .expect("the inner handler matches");
    assert_eq!(inner.segments, 1);
}

#[test]
fn an_unhandled_operation_finds_no_prompt() {
    let s = Stack::new().push_prompt(prompt(), 0);
    assert!(
        s.find_handler(&Symbol::new("db"), &Symbol::new("get"), None)
            .is_none()
    );
}

/// A `simulate` region's delimiter answers the three simulated effects and nothing else, and a
/// `handle` nested inside one still shadows it.
#[test]
fn a_sim_delimiter_answers_the_scheduled_operations_only() {
    let s = Stack::new().push_sim(SimId(0));
    let now = Symbol::new("now");
    assert!(matches!(
        s.find_handler(&Symbol::new("clock"), &now, None)
            .expect("the region handles `clock.now`")
            .target,
        Target::Sim(SimId(0))
    ));
    assert!(
        s.find_handler(&Symbol::new("db"), &Symbol::new("get"), None)
            .is_none(),
        "a region must not claim an effect the language has never heard of"
    );

    let clause = Clause {
        effect: ply_syntax::ast::QName::bare(Ident::new("clock", Span::DUMMY)),
        op: now.clone(),
        resource: None,
        params: Rc::new(Vec::new()),
        resume: None,
        body: ply_eval::code::lower(&crate::unit::build::int(0)).code,
        size: 0,
        captures: ply_eval::code::no_captures(),
        span: Span::DUMMY,
    };
    let inner = s.push_prompt(
        Rc::new(Prompt {
            clauses: Rc::new(vec![clause]),
            effects: Rc::new(vec![Symbol::new("clock")]),
            ret: None,
            clause_captures: vec![Rc::from(Vec::new())],
            ret_captures: Rc::from(Vec::new()),
            module: 0,
            span: Span::DUMMY,
        }),
        0,
    );
    assert!(matches!(
        inner
            .find_handler(&Symbol::new("clock"), &now, None)
            .expect("the nested handler matches")
            .target,
        Target::Ply { .. }
    ));
}
