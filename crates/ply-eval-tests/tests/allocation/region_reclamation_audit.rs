// `Value`'s payloads are `Arc` and thread-confined by design.
#![allow(clippy::arc_with_non_send_sync)]

use crate::counting::charge;
use ply_eval::Value;
use ply_eval::arena::{Arena, Reclaim, RegionKind};
use ply_span::Span;
use std::sync::Arc;

fn counted<R>(f: impl FnOnce() -> R) -> (usize, R) {
    let (out, allocs, _) = charge(f);
    (allocs, out)
}

/// Behind an `Arc`, so `strong_count` shows whether the arena freed it or still holds it.
fn payload(n: i64) -> (Arc<Vec<Value>>, Value) {
    let items = Arc::new(vec![Value::Int(n)]);
    (
        Arc::clone(&items),
        Value::Ctor {
            name: "Box".into(),
            args: items,
        },
    )
}

#[test]
fn a_regions_close_is_a_truncation() {
    for kind in [RegionKind::Unique, RegionKind::Shared] {
        let mut arena = Arena::new();
        let (arc, value) = payload(1);
        let r = arena.open(kind, Span::DUMMY);
        let slot = arena.alloc(value).expect("inside a region");
        arena.alloc(Value::Int(2));

        let outcome = arena.close(r);

        assert_eq!(outcome, Reclaim::Freed(2), "{kind}");
        assert_eq!(arena.live(), 0, "{kind}");
        assert!(arena.get(slot).is_none(), "{kind}");
        assert_eq!(
            Arc::strong_count(&arc),
            1,
            "{kind}: the close dropped the value"
        );
    }
}

#[test]
fn a_close_costs_the_allocator_nothing() {
    let mut arena = Arena::new();
    let warm = |arena: &mut Arena| {
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        for i in 0..1_000 {
            arena.alloc(Value::Int(i));
        }
        arena.close(r)
    };
    for _ in 0..2 {
        warm(&mut arena);
    }

    let (allocations, ()) = counted(|| {
        for _ in 0..100 {
            assert_eq!(warm(&mut arena), Reclaim::Freed(1_000));
        }
    });
    assert_eq!(allocations, 0);

    // The same, under an enclosing region that keeps the chunks from going back.
    let root = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0));
    let (allocations, ()) = counted(|| {
        for _ in 0..100 {
            assert_eq!(warm(&mut arena), Reclaim::Freed(1_000));
        }
    });
    assert_eq!(allocations, 0);
    arena.close(root);
}
