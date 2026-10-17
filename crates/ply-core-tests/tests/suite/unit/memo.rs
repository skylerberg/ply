use ply_core::memo::*;
use ply_core::{Closure, ClosureKind, Symbol, Value};
use std::sync::Arc;

fn nested(levels: usize, bottom: Value) -> Value {
    (0..levels).fold(bottom, |inner, _| Value::Ctor {
        name: Symbol::new("Node"),
        args: Arc::new(vec![Value::list(vec![inner])]),
    })
}

/// A parsed program is a constant deeper than any recursion budget; a hold is its entry's, whatever
/// it holds.
#[test]
fn a_constant_is_judged_by_its_leaves_however_deep_they_lie() {
    assert!(world_independent(&nested(2_000, Value::Unit)));
    let hold = Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Hold {
            held: Some(Value::Unit),
            id: 1,
        },
    }));
    assert!(!world_independent(&nested(2_000, hold)));
}

/// A continuation's captures are numbers, so its kind is all that keeps it out of a memo.
#[test]
fn a_continuation_is_never_remembered_though_a_closure_over_its_numbers_is() {
    let closure = |kind: ClosureKind| Value::Closure(Arc::new(Closure { name: None, kind }));
    let captured = || vec![Value::Int(3), Value::Int(1)];

    let native = closure(ClosureKind::Native {
        code: 0,
        arity: 1,
        captured: captured(),
    });
    assert!(world_independent(&nested(3, native)));

    let continuation = closure(ClosureKind::Continuation {
        code: 0,
        arity: 1,
        captured: captured(),
    });
    assert!(!world_independent(&nested(3, continuation)));
}
