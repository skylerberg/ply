use crate::counting::charge;
use ply_eval::arena::Slot;
use ply_eval::{Fixture, Value};

struct Cost {
    allocs: usize,
    bytes: usize,
}

fn cost_of<T>(f: impl FnOnce() -> T) -> (T, Cost) {
    let (out, allocs, bytes) = charge(f);
    (out, Cost { allocs, bytes })
}

/// Records, so a copy of one is a copy of something rather than of an `i64`.
fn seeded(cells: usize) -> Fixture {
    Fixture::build(|regions| {
        Value::list(
            (0..cells)
                .map(|i| {
                    Value::Cell(regions.alloc_cell(Value::list(vec![
                        Value::Int(i as i64),
                        Value::str(format!("row {i}")),
                    ])))
                })
                .collect(),
        )
    })
}

fn slots_of(fixture: &Fixture) -> Vec<Slot> {
    match fixture.handle() {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::Cell(slot) => *slot,
                other => panic!("expected a cell, found {other:?}"),
            })
            .collect(),
        other => panic!("expected the handle list, found {other:?}"),
    }
}

#[test]
fn opening_a_fixture_costs_the_fixture_and_the_number_is_printed() {
    println!("\n  cells   open allocations   open bytes");
    let mut at = Vec::new();
    for size in [1usize, 1_000, 100_000] {
        let fixture = seeded(size);
        let slots = slots_of(&fixture);
        let ((mut regions, _), cost) = cost_of(|| fixture.open());

        assert_eq!(regions.live(), size);
        assert_eq!(regions.base_len(), size);
        println!("  {size:>5}   {:>16}   {:>10}", cost.allocs, cost.bytes);
        at.push((size, cost.allocs));

        // Nothing a test writes reaches the fixture it opened from.
        assert!(regions.set(slots[size / 2], Value::Int(-1)));
        let (after, _) = fixture.open();
        for (i, (_, value)) in after.slots().enumerate() {
            assert!(
                matches!(value, Value::List(items)
                    if matches!(items.first(), Some(Value::Int(n)) if *n == i as i64)),
                "cell {i} of a {size}-cell fixture reads {value:?} after a test wrote it"
            );
        }
    }

    let (_, one) = at[0];
    let (_, many) = at[2];
    assert!(
        many > one,
        "opening a 100,000-cell fixture allocated {many} against {one} for one \
         cell, which would mean the replay is not happening"
    );
}

#[test]
fn resetting_to_the_fixture_allocates_nothing_however_much_the_run_did() {
    let fixture = seeded(1_000);
    let (mut regions, _) = fixture.open();

    for round in 0..4 {
        for i in 0..50_000 {
            regions.alloc_cell(Value::Int(i));
        }
        let ((), cost) = cost_of(|| regions.reset());
        assert_eq!(
            (cost.allocs, cost.bytes),
            (0, 0),
            "round {round}: resetting after 50,000 cells allocated {} times",
            cost.allocs
        );
        assert_eq!(regions.live(), 1_000, "round {round}");
    }
}

#[test]
fn two_stacks_opened_from_one_fixture_share_no_storage() {
    let fixture = seeded(64);
    let slots = slots_of(&fixture);

    let (mut a, _) = fixture.open();
    let (b, _) = fixture.open();
    assert!(a.set(slots[7], Value::Int(-1)));

    assert!(matches!(a.get(slots[7]), Some(Value::Int(-1))));
    assert!(matches!(b.get(slots[7]), Some(Value::List(_))));
}
