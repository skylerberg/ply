use ply_eval::Value;
use ply_eval::window::*;

#[test]
fn a_move_leaves_the_slot_marked_and_a_vacant_slot_stays_vacant() {
    let mut w = Windows::new();
    w.enter(2);
    w.write(0, Value::Int(7));
    match w.take(0) {
        SlotVal::Full(Value::Int(7)) => {}
        other => panic!("expected the value, got {other:?}"),
    }
    assert!(matches!(w.read(0), SlotVal::Moved));
    assert!(matches!(w.take(1), SlotVal::Vacant));
    assert!(matches!(w.read(1), SlotVal::Vacant));
}

/// The overlap rule: the shared portion below the captured prompt's entry is cloned, and the
/// extent above it is moved out — so the activation continuing below still reads its window,
/// and the resumption reads the snapshot.
#[test]
fn a_cut_clones_the_shared_portion_and_moves_the_extent() {
    let mut w = Windows::new();
    w.enter(2);
    w.write(0, Value::Int(1));
    w.write(1, Value::Int(2));
    w.enter(1);
    w.write(2, Value::Int(3));
    let saved = w.cut(0, 2);
    assert_eq!(saved.len(), 3);
    assert_eq!(w.len(), 2, "the shared portion stays on the stack");
    w.restore(&saved);
    assert_eq!(w.len(), 5);
}
