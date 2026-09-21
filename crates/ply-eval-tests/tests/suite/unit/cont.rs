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
