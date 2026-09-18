use ply_eval::Value;
use ply_eval::cont::*;
use ply_span::Span;
use ply_ty::BinOp;
use std::rc::Rc;

fn frame(n: i64) -> Frame {
    Frame::BinaryApply {
        op: BinOp::Add,
        lhs: Value::Int(n),
        lhs_span: Span::DUMMY,
        rhs_span: Span::DUMMY,
        span: Span::DUMMY,
    }
}

fn marker_of(f: &Frame) -> i64 {
    match f {
        Frame::BinaryApply {
            lhs: Value::Int(n), ..
        } => *n,
        _ => panic!("expected a marker frame"),
    }
}

fn prompt() -> Rc<Prompt> {
    Rc::new(Prompt { span: Span::DUMMY })
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
    assert_eq!(marker_of(&top), 2);
    let Next::Frame(under, rest) = rest.next() else {
        panic!("expected a frame");
    };
    assert_eq!(marker_of(&under), 1);
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
    let s = Stack::new().push(frame(1)).push_prompt(prompt());
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
        .push_prompt(prompt())
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
    let s = Stack::new().push_prompt(prompt()).push(frame(1));
    let (k, below) = s.capture(1, 0);
    assert!(below.prompt().is_none());

    let resumed = below.resume(&k);
    assert!(resumed.prompt().is_some());
    assert_eq!(resumed.frames(), 1);
}

#[test]
fn a_continuation_may_be_resumed_twice_onto_different_stacks() {
    let s = Stack::new().push_prompt(prompt()).push(frame(9));
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
    assert_eq!(marker_of(&a), 9);
    assert_eq!(marker_of(&b), 9);
}

/// `into_next` moves the frame out when nothing else holds it, so the captured segment must hold it.
#[test]
fn popping_a_captured_frame_leaves_the_continuation_able_to_splice_it_again() {
    let s = Stack::new()
        .push_prompt(prompt())
        .push(frame(1))
        .push(frame(2));
    let (k, below) = s.capture(1, 0);

    let Next::Frame(first, rest) = below.resume(&k).into_next() else {
        panic!("expected a frame");
    };
    assert_eq!(marker_of(&first), 2);
    let Next::Frame(second, _) = rest.into_next() else {
        panic!("expected a frame");
    };
    assert_eq!(marker_of(&second), 1);

    assert_eq!(k.frames(), 2);
    let again = below.resume(&k);
    assert_eq!(again.frames(), 2);
    let Next::Frame(replayed, _) = again.into_next() else {
        panic!("expected a frame");
    };
    assert_eq!(marker_of(&replayed), 2);
}

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
        .push_prompt(prompt())
        .push(frame(1))
        .push_prompt(prompt())
        .push(frame(2))
        .push_prompt(prompt())
        .push(frame(3));

    let (k, below) = s.capture(3, 0);
    assert_eq!(k.segments(), 3);
    assert_eq!(k.frames(), 3);
    assert_eq!(below.segments(), 1);
    assert_eq!(below.frames(), 0);

    assert_eq!(below.resume(&k).segments(), 4);
}
