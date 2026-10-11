use ply_eval::Value;
use ply_eval::arena::{Arena, Owner, Reclaim, RegionKind, Slot};

/// [`Arena::close`] never reads the region's kind.
#[test]
fn what_a_close_reclaims_is_decided_by_the_extent_and_never_by_the_kind() {
    for kind in [RegionKind::Unique, RegionKind::Shared] {
        let mut arena = Arena::new();
        let region = arena.open(Owner::ENTRY, kind);
        let cell = arena
            .alloc(Owner::ENTRY, Value::Int(1))
            .expect("the region is open");

        assert_eq!(
            arena.close(region),
            Reclaim::Freed(1),
            "{kind}: nothing can reach these slots and they were kept anyway"
        );
        assert_eq!(arena.get(cell), None, "{kind}");
    }
}

/// Only the generation catches a stale read, so between renewals wrapping is the one way a wrong
/// value returns.
#[test]
fn a_positions_generation_only_rises_and_never_hands_back_an_identity() {
    const ROUNDS: u32 = 2_000;
    let mut arena = Arena::new();
    let mut seen: Vec<Slot> = Vec::new();
    for round in 0..ROUNDS {
        let region = arena.open(Owner::ENTRY, RegionKind::Unique);
        let slot = arena
            .alloc(Owner::ENTRY, Value::Int(round as i64))
            .expect("just opened");
        assert_eq!(
            slot.index(),
            0,
            "the bump pointer went back to the same place"
        );
        assert_eq!(
            slot.generation(),
            round,
            "a position's generation is the number of closes it has been through"
        );
        assert!(
            !seen.contains(&slot),
            "position 0 handed out an identity it had used before, which is the only way a stale \
             read becomes a value rather than a diagnostic"
        );
        seen.push(slot);
        arena.close(region);
    }
    for stale in &seen[..seen.len() - 1] {
        assert!(
            arena.get(*stale).is_none(),
            "{stale} resolved after its close"
        );
    }
    assert_eq!(
        arena.stats().closes_freed as u32,
        ROUNDS,
        "every round has to have been a real free, or the counter never moved"
    );
}
