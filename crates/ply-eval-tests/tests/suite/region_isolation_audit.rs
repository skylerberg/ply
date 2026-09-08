//! Adversarial audit of the one property the milestone cannot be wrong about: **no two region
//! stacks opened from one fixture observe each other's writes.**

use crate::fixture::Compiled;
use ply_core::Footprint;
use ply_eval::arena::Slot;
use ply_eval::{Fixture, Machine, TaskRegions, Value};
use std::marker::PhantomData;

impl Compiled {
    fn footprint(&self, name: &str) -> &Footprint {
        &self.check.tests[self.index_of(name)].footprint
    }

}

fn int_of(regions: &TaskRegions, slot: Slot) -> i64 {
    match regions.get(slot) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an Int in {slot}, found {other:?}"),
    }
}

fn one_cell() -> Fixture {
    Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(0))))
}

fn cell_of(fixture: &Fixture) -> Slot {
    fixture
        .handle()
        .as_cell(ply_span::Span::DUMMY, "the fixture handle")
        .expect("the handle is a cell")
}

/// The headline property, stated over three stacks rather than two so that a defect that leaks in
/// only one direction cannot hide behind symmetry.
#[test]
fn stacks_opened_from_one_fixture_never_read_each_others_value() {
    let fixture = one_cell();
    let shared = cell_of(&fixture);

    let mut stacks: Vec<TaskRegions> = (0..3).map(|_| fixture.open().0).collect();
    for (i, stack) in stacks.iter_mut().enumerate() {
        assert!(stack.set(shared, Value::Int(i as i64 + 1)));
    }
    // Interleaved a second time: a defect that needs the writes to alternate rather than run in a
    // batch would survive the loop above.
    for i in (0..stacks.len()).rev() {
        assert!(stacks[i].set(shared, Value::Int(i as i64 + 10)));
        for (j, other) in stacks.iter().enumerate() {
            let expected = if j >= i { j as i64 + 10 } else { j as i64 + 1 };
            assert_eq!(
                int_of(other, shared),
                expected,
                "stack {j} after writing {i}"
            );
        }
    }
    assert_eq!(fixture.len(), 1, "the fixture itself is untouched");
    assert_eq!(
        int_of(&fixture.open().0, shared),
        0,
        "and still seeds a zero"
    );
}

/// The direction the forkable world made least obvious was a write to a shared ancestor.
#[test]
fn no_amount_of_writing_to_an_open_stack_moves_what_the_fixture_seeds() {
    const DEPTH: usize = 12;

    let fixture = Fixture::build(|r| {
        Value::list(
            (0..DEPTH)
                .map(|_| Value::Cell(r.alloc_cell(Value::Int(-1))))
                .collect(),
        )
    });
    let cells: Vec<Slot> = match fixture.handle() {
        Value::List(items) => items
            .iter()
            .map(|v| {
                v.as_cell(ply_span::Span::DUMMY, "a handle")
                    .expect("a cell")
            })
            .collect(),
        other => panic!("expected the handle list, found {other:?}"),
    };

    let mut stacks: Vec<TaskRegions> = (0..DEPTH).map(|_| fixture.open().0).collect();
    let mark = |level: usize, i: usize| (level * 100 + i) as i64;
    for level in [0usize, 3, 7] {
        for (i, slot) in cells.iter().enumerate() {
            assert!(stacks[level].set(*slot, Value::Int(mark(level, i))));
        }
        for (other, stack) in stacks.iter().enumerate() {
            for (i, slot) in cells.iter().enumerate() {
                let seen = int_of(stack, *slot);
                let expected = if other == level { mark(level, i) } else { -1 };
                assert_eq!(
                    seen, expected,
                    "stack {other} observed a write made to stack {level}"
                );
            }
        }
        for slot in &cells {
            assert!(stacks[level].set(*slot, Value::Int(-1)));
        }
    }

    for (i, slot) in cells.iter().enumerate() {
        assert_eq!(int_of(&fixture.open().0, *slot), -1, "cell {i}");
    }
}

/// An entry point's reset is the fork's replacement, and it has to mean both halves: the fixture
/// comes back to what it was seeded as, and everything the entry point allocated on top of it is
/// gone.
#[test]
fn a_reset_restores_the_seed_and_discards_what_the_entry_point_allocated() {
    let fixture = one_cell();
    let seeded = cell_of(&fixture);
    let (mut regions, _) = fixture.open();

    assert!(regions.set(seeded, Value::Int(1)));
    let scratch = regions.alloc_cell(Value::Int(2));
    assert_eq!(int_of(&regions, seeded), 1);

    regions.reset();

    assert_eq!(int_of(&regions, seeded), 0);
    assert!(!regions.contains(scratch));
}

/// The hazard that makes every other test here necessary: two stacks opened from one fixture hand
/// out the *same* slot for different cells, and reading a foreign slot succeeds quietly instead of
/// failing.
#[test]
fn a_foreign_slot_is_answered_by_the_reading_stack_and_never_by_its_owner() {
    let fixture = Fixture::empty();
    let (mut a, _) = fixture.open();
    let (mut b, _) = fixture.open();

    let in_a = a.alloc_cell(Value::str("a's secret"));
    let in_b = b.alloc_cell(Value::str("b's secret"));
    assert_eq!(in_a, in_b, "two fresh stacks bump from the same floor");

    assert_eq!(a.get(in_b).map(Value::render).unwrap(), "\"a's secret\"");
    assert_eq!(b.get(in_a).map(Value::render).unwrap(), "\"b's secret\"");

    assert!(a.set(in_b, Value::str("clobbered")));
    assert_eq!(b.get(in_b).map(Value::render).unwrap(), "\"b's secret\"");
}

/// What a slot buys that a `CellId` did not: a slot whose region has been reclaimed reads `None` on
/// every run rather than aliasing whatever was allocated in its place.
#[test]
fn a_slot_from_a_reclaimed_entry_point_reads_nothing_rather_than_its_successor() {
    let (mut regions, _) = Fixture::empty().open();
    let stale = regions.alloc_cell(Value::str("the first run's secret"));

    regions.reset();
    let fresh = regions.alloc_cell(Value::str("the second run's secret"));

    assert_eq!(
        stale.index(),
        fresh.index(),
        "the index was handed out again"
    );
    assert!(
        regions.get(stale).is_none(),
        "and the stale slot reads nothing"
    );
    assert!(
        !regions.set(stale, Value::Int(0)),
        "a write through it is refused"
    );
    assert_eq!(
        regions.get(fresh).map(Value::render).unwrap(),
        "\"the second run's secret\""
    );
}


/// Every carrier the escape brand names is refused, including the closure route it lists first among the
/// ways this could go wrong.
#[test]
fn every_closure_shaped_carrier_out_of_a_region_is_refused() {
    for (carrier, src) in [
        (
            "a closure",
            r#"test "smuggle" {
  let read = with_cell[log](41) { c -> || cell_get(c) };
  assert_eq(read(), 41)
}"#,
        ),
        (
            "a record of closures",
            r#"test "smuggle" {
  let ops = with_cell[log](1) { c -> {get: || cell_get(c), put: |v| cell_set(c, v)} };
  let get = ops.get;
  let put = ops.put;
  put(9);
  assert_eq(get(), 9)
}"#,
        ),
        (
            "a closure that only writes",
            r#"test "smuggle" {
  let bump = with_cell[log](0) { c -> || cell_set(c, cell_get(c) + 1) };
  bump()
}"#,
        ),
    ] {
        let diags = Compiled::rejected(src);
        assert!(
            diags.iter().any(|d| d.message.contains("escapes its")),
            "{carrier} must not carry a cell out of its region: {diags:#?}"
        );
    }
}



/// A `cell` atom reaching a published footprint is what the scheduler colours on, and with every
/// escape route closed a *written row* is the only way one gets there.
#[test]
fn a_declared_cell_atom_is_what_reaches_a_tests_footprint() {
    let compiled = Compiled::new(
        r#"
fn touches(n: Int) -> Int / {cell.read[log]} = n
fn writes(n: Int) -> Int / {cell.read[log], cell.write[log]} = n

test "a read" {
  let seen = with_cell[log](41) { c -> cell_get(c) };
  assert_eq(touches(seen), 41)
}

test "a read and a write" {
  let seen = with_cell[log](1) { c -> { cell_set(c, 9); cell_get(c) } };
  assert_eq(writes(seen), 9)
}
"#,
    );
    let atoms: Vec<String> = compiled
        .footprint("a read")
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(atoms, vec!["cell.read[log]".to_string()]);

    let mixed: Vec<String> = compiled
        .footprint("a read and a write")
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        mixed,
        vec!["cell.read[log]".to_string(), "cell.write[log]".to_string()]
    );

    // And a region discharges its own label: the same atoms performed inside the region never reach
    // the footprint at all.
    let discharged = Compiled::new(
        r#"
test "inside the region" {
  with_cell[log](41) { c -> { cell_set(c, 9); assert_eq(cell_get(c), 9) } }
}
"#,
    );
    assert_eq!(discharged.footprint("inside the region").atoms().count(), 0);
}


/// A cell in a *constructor argument* used to be the one carrier the region check could not see:
/// the variant's field type holds the `Cell`, so the region's result type was `Held` and mentioned
/// no region.
#[test]
fn a_cell_in_a_constructor_argument_is_refused_where_the_field_is_declared() {
    let diags = Compiled::rejected(
        r#"
type Held = Held(Cell<Int>)

test "a constructor carries the cell out of its region" {
  let h = with_cell[log](1) { c -> Held(c) };
  match h { Held(c) -> { cell_set(c, 2); assert_eq(cell_get(c), 2) } }
}
"#,
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::REGION_ESCAPE),
        "a declared `Cell` field is a brand with nowhere to appear: {diags:#?}"
    );
}

/// The boundary of that hole: the region variable in a declared `Cell<T>` field is fixed by the
/// first region that fills it, so a second region using the same type is a mismatch rather than a
/// silent alias between two regions' cells.
#[test]
fn one_variant_cannot_hold_cells_from_two_regions_at_once() {
    let diags = Compiled::rejected(
        r#"
type Held = Held(Cell<Int>)

test "two regions through one variant" {
  let a = with_cell[log](1) { c -> Held(c) };
  let b = with_cell[audit](2) { c -> Held(c) };
  match a { Held(c) -> match b { Held(d) -> assert_eq(cell_get(c) + cell_get(d), 3) } }
}
"#,
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::TYPE_MISMATCH),
        "a second region must not quietly reuse the first one's tag: {diags:#?}"
    );
}

/// A cell in a list element or a record field *is* caught, because both keep the `Cell` type in the
/// region's result type where `mentions_region` finds it.
#[test]
fn a_cell_in_a_list_or_a_record_field_is_refused_by_the_region_check() {
    for (carrier, src) in [
        (
            "list",
            r#"
test "smuggle" {
  let xs = with_cell[log](1) { c -> [c] };
  assert_eq(len(xs), 1)
}
"#,
        ),
        (
            "record",
            r#"
test "smuggle" {
  let r = with_cell[log](1) { c -> {cell: c} };
  assert_eq(cell_get(r.cell), 1)
}
"#,
        ),
    ] {
        let diags = Compiled::rejected(src);
        assert!(
            diags.iter().any(|d| d.message.contains("escapes its")),
            "a cell in a {carrier} must be refused: {diags:#?}"
        );
    }
}








/// A test can only sample the executions somebody thought of.
#[test]
fn a_region_stack_and_the_values_in_it_cannot_cross_a_thread() {
    assert!(
        !is_send!(TaskRegions),
        "a region stack must stay thread-confined"
    );
    assert!(!is_send!(ply_eval::Arena));
    assert!(!is_send!(Value), "Value must stay thread-confined");
    assert!(!is_send!(ply_eval::Continuation));
    assert!(!is_send!(ply_eval::Stack));
    assert!(!is_send!(ply_eval::Windows));
    assert!(!is_send!(ply_eval::Fixture));
    assert!(!is_send!(Machine<'static>));
    // The sanity half: the probe reports `true` for something that is `Send`.
    assert!(is_send!(Slot));
    assert!(is_send!(ply_span::Span));
}

/// Autoref specialization: the inherent method exists only when `T: Send`, and the trait method on
/// `&Probe<T>` needs one more autoref step, so it is chosen exactly when the inherent one does not
/// apply.
struct Probe<T>(PhantomData<T>);

impl<T: Send> Probe<T> {
    fn probe(&self) -> bool {
        true
    }
}

trait NotSend {
    fn probe(&self) -> bool;
}

impl<T> NotSend for &Probe<T> {
    fn probe(&self) -> bool {
        false
    }
}

macro_rules! is_send {
    ($t:ty) => {
        (&Probe::<$t>(PhantomData)).probe()
    };
}
use is_send;
