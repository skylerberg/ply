use crate::counting::charge;
use ply_codegen::heap::{self, Heap, Word};
use ply_eval::Value;

/// `live` bridges made and released, their words kept in `words`, whose capacity is `live`.
fn round(h: &mut Heap, words: &mut Vec<Word>, live: usize) {
    for i in 0..live {
        words.push(h.bridge(Value::Float(i as f64)));
    }
    for w in words.drain(..) {
        heap::dec(w);
    }
}

#[test]
fn a_warm_heap_bridges_from_its_free_list_without_touching_the_allocator() {
    const LIVE: usize = 1_000;
    const ROUNDS: usize = 100;
    let mut h = Heap::new();
    heap::enter(&mut h);
    let mut words = Vec::with_capacity(LIVE);
    round(&mut h, &mut words, LIVE);

    let ((), allocations, bytes) = charge(|| {
        for _ in 0..ROUNDS {
            round(&mut h, &mut words, LIVE);
        }
    });
    heap::leave();

    assert_eq!(
        (allocations, bytes),
        (0, 0),
        "{ROUNDS} rounds of {LIVE} bridges on a warm heap reached the allocator"
    );
    assert_eq!(
        h.recycled(),
        ROUNDS * LIVE,
        "a bridge took fresh memory with dead bridges' blocks on hand"
    );
    assert_eq!(h.bridges(), 0, "a released bridge stayed in the table");
    h.end();
}
