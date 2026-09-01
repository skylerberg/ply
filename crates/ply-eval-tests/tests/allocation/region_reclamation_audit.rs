//! When a region's memory goes back — the region-kind rule's other half.

// A `Value`'s payloads are `Arc` and thread-confined by design, which is the crate's own decision
// rather than something to lint here.
#![allow(clippy::arc_with_non_send_sync)]

use crate::counting::charge;
use ply_eval::Value;
use ply_eval::arena::{Arena, Pin, Reclaim, RegionKind, Slot, stale_slot, unique_capture};
use ply_eval::region_kind::infer;
use ply_span::{SourceId, SourceMap, Span, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;

/// Allocations `f` took from the global allocator.
fn counted<R>(f: impl FnOnce() -> R) -> (usize, R) {
    let (out, allocs, _) = charge(f);
    (allocs, out)
}

/// A value whose payload is behind an `Arc`, so `strong_count` says whether the arena freed it or
/// is still holding it for a continuation.
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

#[track_caller]
fn int_at(arena: &Arena, slot: Slot) -> Option<i64> {
    match arena.get(slot) {
        Some(Value::Int(i)) => Some(*i),
        None => None,
        other => panic!("expected an Int in {slot}, found {other:?}"),
    }
}

/// What the machine does at a `perform` whose handler binds a continuation: take the pin, and hold
/// it for exactly as long as the continuation.
fn capture(arena: &mut Arena) -> Pin {
    arena.pin().expect("a capture inside a region pins it")
}

/// The free case, stated as the only thing that could make it not free: the close hands the slots
/// back and the values are dropped there.
#[test]
fn a_unique_regions_close_is_a_truncation() {
    let mut arena = Arena::new();
    let (arc, value) = payload(1);
    let r = arena.open(RegionKind::Unique, Span::DUMMY);
    let slot = arena.alloc(value).expect("inside a region");
    arena.alloc(Value::Int(2));

    let outcome = arena.close(r);

    assert_eq!(outcome, Reclaim::Freed(2));
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.retained_slots(), 0);
    assert!(arena.get(slot).is_none());
    assert_eq!(Arc::strong_count(&arc), 1, "the close dropped the value");
}

/// "Free" as a number rather than an adjective.
#[test]
fn a_unique_close_costs_the_allocator_nothing_even_beside_a_live_continuation() {
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

    // And with a continuation live over a region opened *after* its capture, which is the shape a
    // handler clause allocating its own scratch cell takes.
    let root = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0));
    let pin = capture(&mut arena);
    let (allocations, ()) = counted(|| {
        for _ in 0..100 {
            assert_eq!(warm(&mut arena), Reclaim::Freed(1_000));
        }
    });
    assert_eq!(
        allocations, 0,
        "a region opened after the capture is not covered by it"
    );
    drop(pin);
    arena.close(root);
}

/// The rule, at its sharpest: the region has lexically closed and its slots are still there,
/// because the continuation captured across it can still be resumed and read them.
#[test]
fn a_shared_regions_close_reclaims_nothing_while_a_continuation_lives() {
    let mut arena = Arena::new();
    let (arc, value) = payload(7);
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let cell = arena.alloc(value).expect("inside a region");

    let pin = capture(&mut arena);
    let outcome = arena.close(r);

    assert_eq!(outcome, Reclaim::Retained(1));
    assert_eq!(arena.retained_slots(), 1);
    assert_eq!(arena.retained_regions(), vec![r]);
    assert_eq!(arena.live(), 1, "the bump pointer did not go back");
    assert_eq!(arena.depth(), 0, "but the region is closed to the program");
    assert!(
        arena.get(cell).is_some(),
        "the resumption that reads this is what the region is `shared` for"
    );
    assert_eq!(Arc::strong_count(&arc), 2, "and nothing was dropped");
    drop(pin);
}

/// Reclamation is the *last* continuation dying, not the first.
#[test]
fn the_slots_go_back_when_the_last_continuation_dies_and_not_before() {
    let mut arena = Arena::new();
    let (arc, value) = payload(7);
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let cell = arena.alloc(value).expect("inside a region");

    let pin = capture(&mut arena);
    // A clause binder and a `Resume` frame holding the same continuation.
    let second = pin.clone();
    assert_eq!(arena.close(r), Reclaim::Retained(1));

    drop(pin);
    arena.collect();
    assert_eq!(arena.retained_slots(), 1, "one holder is still a holder");
    assert_eq!(arena.live_pins(), 1);
    assert!(arena.get(cell).is_some());

    drop(second);
    arena.collect();

    assert_eq!(arena.retained_slots(), 0);
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.live_pins(), 0);
    assert_eq!(Arc::strong_count(&arc), 1, "the late reclamation freed it");
    assert_eq!(arena.stats().slots_reclaimed_late, 1);
    assert_eq!(arena.stats().closes_deferred, 1);
}

/// A continuation that died before the region's lexical close costs the region nothing: the close
/// is the same truncation a `unique` region's is.
#[test]
fn a_shared_region_whose_continuation_already_died_closes_by_truncation() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let cell = arena.alloc(Value::Int(1)).expect("inside a region");

    let pin = capture(&mut arena);
    drop(pin);

    assert_eq!(arena.close(r), Reclaim::Freed(1));
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.retained_slots(), 0);
    assert_eq!(arena.stats().closes_deferred, 0);
    assert!(arena.get(cell).is_none());
}

/// The close of a region a live continuation crosses is deferred, and the next close after the
/// continuation dies is what hands the slots back — a program that keeps opening regions reclaims
/// without anyone asking it to.
#[test]
fn a_later_close_reclaims_what_an_earlier_one_could_not() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0));
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));

    let pin = capture(&mut arena);
    assert_eq!(arena.close(inner), Reclaim::Retained(1));
    drop(pin);

    // The next close reaps the dead continuation before deciding anything, so the held run goes
    // back there and this close accounts only for its own.
    assert_eq!(arena.close(outer), Reclaim::Freed(1));
    assert_eq!(arena.stats().slots_reclaimed_late, 1);
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.retained_slots(), 0);
}

#[test]
fn a_slot_reclaimed_late_reads_nothing_rather_than_the_next_regions_value() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let stale = arena.alloc(Value::Int(1)).expect("inside a region");

    let pin = capture(&mut arena);
    assert_eq!(arena.close(r), Reclaim::Retained(1));
    assert_eq!(int_at(&arena, stale), Some(1));
    drop(pin);
    arena.collect();

    let next = arena.open(RegionKind::Unique, Span::DUMMY);
    let fresh = arena.alloc(Value::Int(2)).expect("inside a region");

    assert_eq!(
        stale.index(),
        fresh.index(),
        "the bump pointer reused the position, which is why the generation matters"
    );
    assert_ne!(stale.generation(), fresh.generation());
    assert_eq!(int_at(&arena, stale), None);
    assert!(!arena.set(stale, Value::Int(99)), "and a write is refused");
    assert_eq!(int_at(&arena, fresh), Some(2), "the live slot is untouched");
    arena.close(next);
}

/// And that `None` is a diagnostic rather than a shrug.
#[test]
fn a_stale_access_is_reported_against_ply_rather_than_the_program() {
    let d = stale_slot(Slot::new(3, 1), Span::DUMMY);
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert!(d.message.contains("reclaimed"), "{}", d.message);
    assert!(!d.notes.is_empty(), "it says how it could have happened");
}

/// Mixed kinds, nested.
#[test]
fn a_unique_region_nested_in_a_held_shared_one_is_still_reclaimed_at_its_close() {
    let mut arena = Arena::new();
    let shared = arena.open(RegionKind::Shared, Span::DUMMY);
    let held = arena.alloc(Value::Int(1)).expect("inside a region");
    let pin = capture(&mut arena);

    let unique = arena.open(RegionKind::Unique, Span::DUMMY);
    let scratch = arena.alloc(Value::Int(2)).expect("inside a region");
    assert_eq!(arena.close(unique), Reclaim::Freed(1));

    assert_eq!(arena.live(), 1);
    assert!(arena.get(scratch).is_none());
    assert_eq!(int_at(&arena, held), Some(1));

    assert_eq!(arena.close(shared), Reclaim::Retained(1));
    drop(pin);
    arena.collect();
    assert_eq!(arena.live(), 0);
}

/// A region opened *at* a capture's bump pointer, having allocated nothing, is still not covered by
/// it: coverage is "was this region open at the capture", which the marks cannot tell apart and the
/// open ordinals can.
#[test]
fn a_region_opened_after_a_capture_that_allocated_nothing_is_not_covered_by_it() {
    let mut arena = Arena::new();
    let root = arena.open(RegionKind::Shared, Span::DUMMY);
    let pin = capture(&mut arena);
    assert_eq!(pin.extent(), 0, "nothing was live at the capture");

    let after = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(1));

    assert_eq!(
        arena.close(after),
        Reclaim::Freed(1),
        "same mark as the capture, opened after it"
    );
    assert_eq!(
        arena.close(root),
        Reclaim::Freed(0),
        "and the region the capture *is* in holds nothing to hold on to"
    );
    drop(pin);
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.retained_slots(), 0);
}

/// A bump pointer frees a suffix and not a hole.
#[test]
fn a_retained_run_under_a_live_regions_slots_waits_for_that_region() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    let before = arena.alloc(Value::Int(0)).expect("inside a region");

    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let pin = capture(&mut arena);
    assert_eq!(arena.close(inner), Reclaim::Retained(1));

    // The enclosing region carries on allocating over the retained run.
    let after = arena.alloc(Value::Int(2)).expect("inside a region");
    drop(pin);
    arena.collect();

    assert_eq!(
        arena.retained_slots(),
        1,
        "the run is not the top of the arena, so it cannot be truncated away"
    );
    assert_eq!(int_at(&arena, after), Some(2), "and nothing above it moved");
    assert_eq!(int_at(&arena, before), Some(0));

    assert_eq!(arena.close(outer), Reclaim::Freed(3));
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.retained_slots(), 0);
}

/// Runs held inside a region are absorbed into that region's own run when it closes, rather than
/// accumulating one entry per iteration.
#[test]
fn runs_held_inside_a_region_are_absorbed_into_it_when_it_closes() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    let mut branches = Vec::new();
    for i in 0..64 {
        let inner = arena.open(RegionKind::Shared, Span::DUMMY);
        arena.alloc(Value::Int(i));
        branches.push(capture(&mut arena));
        assert_eq!(arena.close(inner), Reclaim::Retained(1));
    }
    assert_eq!(arena.retained_slots(), 64);

    assert_eq!(arena.close(outer), Reclaim::Retained(64));
    let held = arena.retained_regions();
    assert_eq!(held.len(), 65, "the enclosing region and its sixty-four");
    let mut sorted = held.clone();
    sorted.sort_unstable();
    assert_eq!(held, sorted, "reported in a deterministic order");

    branches.clear();
    arena.collect();
    assert_eq!(arena.live(), 0);
    assert_eq!(arena.retained_slots(), 0);
    assert_eq!(arena.stats().slots_reclaimed_late, 64);
}

/// The interaction the region model's Consequences call the hardest to see: `unique` inferred where a
/// capture is reachable frees memory a continuation still holds.
#[test]
fn no_region_a_capture_can_be_taken_inside_is_inferred_unique() {
    const AMB: &str = "effect amb { read flip[coin]() -> Bool }\n";
    let shapes: &[(&str, &str)] = &[
        (
            "a general clause inside the region",
            "fn go() -> Int = with_cell[r](0) { c ->
               handle { if amb.flip[coin]() { 1 } else { 2 } }
               with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x } }",
        ),
        (
            "a handle lexically enclosing the region, which answers across it",
            "fn go() -> Int =
               handle { with_cell[r](0) { c -> if amb.flip[coin]() { 1 } else { 2 } } }
               with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }",
        ),
        (
            "a perform the region does not answer",
            "fn go() -> Bool = with_cell[r](0) { c -> amb.flip[coin]() }",
        ),
        (
            "a capture reachable only through a called definition",
            "fn coin() -> Bool = amb.flip[coin]()
             fn go() -> Bool = with_cell[r](0) { c -> coin() }",
        ),
        (
            "a task, which the scheduler parks and resumes",
            "fn work() -> Unit = ()
             fn go() -> Unit = with_cell[r](0) { c -> { let t = task.spawn(|| work()); task.join(t) } }",
        ),
        (
            "a simulated region, which does the same to every task in it",
            "fn go() -> Int = with_cell[r](0) { c -> simulate { cell_get(c) } }",
        ),
    ];

    for (what, body) in shapes {
        let src = format!("{AMB}{body}\n");
        let (program, resolved) = load(&src);
        let regions = infer(&program, &resolved);
        assert!(!regions.is_empty(), "{what}: this probe opens no region");
        assert_eq!(
            regions.unique(),
            0,
            "{what}: a `unique` region here is memory freed under a live continuation\n{src}"
        );
    }
}

/// And when the machine and the inference do disagree anyway, the answer is a report and not a
/// free: the pin is taken across the `unique` region, so its slots survive, and the region is named
/// so the run can say what happened.
#[test]
fn a_capture_across_a_unique_region_is_named_rather_than_freed() {
    let mut arena = Arena::new();
    let unique = arena.open(RegionKind::Unique, Span::DUMMY);
    let cell = arena.alloc(Value::Int(1)).expect("inside a region");

    assert_eq!(arena.unique_open(), Some(unique));
    let pin = capture(&mut arena);

    assert_eq!(arena.close(unique), Reclaim::Retained(1));
    assert_eq!(
        int_at(&arena, cell),
        Some(1),
        "a disagreement about reachability is never resolved by freeing"
    );

    let d = unique_capture(unique, Span::DUMMY, Span::DUMMY);
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert!(d.message.contains("unique"), "{}", d.message);
    drop(pin);
}

#[test]
fn a_capture_outside_every_region_has_nothing_to_pin() {
    let mut arena = Arena::new();
    assert!(arena.pin().is_none());
    assert_eq!(arena.stats().pins_taken, 0);
}

/// Reclamation must not become the retracted snapshot reading by another route.
#[test]
fn holding_a_region_for_a_continuation_does_not_save_or_restore_it() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let trace = arena.alloc(Value::Int(0)).expect("inside a region");

    let pin = capture(&mut arena);
    // Resumption one, then resumption two, on one arena that moves forward.
    for _ in 0..2 {
        let seen = int_at(&arena, trace).expect("the region is held");
        arena.set(trace, Value::Int(seen + 1));
    }

    assert_eq!(
        int_at(&arena, trace),
        Some(2),
        "snapshot-at-capture would answer 1, and it is the reading the region model retracted"
    );
    assert_eq!(arena.stats().snapshots, 0, "nothing on this path snapshots");
    assert_eq!(arena.stats().restores, 0);
    assert_eq!(arena.close(r), Reclaim::Retained(1));
    drop(pin);
}

/// A checkpoint may not undo an allocation a live continuation can reach: that is the same free,
/// taken from the other end.
#[test]
fn a_restore_is_refused_while_a_continuation_is_pinned() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    let before = arena.alloc(Value::Int(1)).expect("inside a region");
    let checkpoint = arena.snapshot(r).expect("a shared region snapshots");

    let reachable = arena.alloc(Value::Int(2)).expect("inside a region");
    let pin = capture(&mut arena);

    assert!(!arena.restore(&checkpoint));
    assert_eq!(int_at(&arena, reachable), Some(2));

    drop(pin);
    assert!(arena.restore(&checkpoint), "and allowed once it has died");
    assert_eq!(int_at(&arena, before), Some(1));
    assert_eq!(int_at(&arena, reachable), None);
    arena.close(r);
}

/// The reference-counting pass accepts that a cycle leaks, and this is what one looks like in the allocator: a
/// continuation parked in a cell of a region it pins.
#[test]
fn a_continuation_parked_in_the_region_that_pins_it_is_the_leak_adr_0017_accepts() {
    let mut arena = Arena::new();
    let (arc, value) = payload(1);
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(value).expect("inside a region");
    // `cell_set(s, Just(k))`: the continuation becomes reachable only from a slot of the region it
    // is holding open, so nothing outside will drop it.
    let parked = capture(&mut arena);

    assert_eq!(arena.close(r), Reclaim::Retained(1));
    arena.collect();

    assert_eq!(
        arena.retained_slots(),
        1,
        "the region holds the continuation that holds the region"
    );
    assert_eq!(arena.stats().slots_reclaimed_late, 0);
    assert_eq!(Arc::strong_count(&arc), 2, "so the value is still held");

    // The arena's own drop is what ends it — a task ending, rather than any region closing.
    drop(arena);
    assert_eq!(Arc::strong_count(&arc), 1);
    drop(parked);
}

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("reclaim.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("reclaim"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}
