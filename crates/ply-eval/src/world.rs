//! Mutable state as a persistent value.
//!
//! A cell is a key, not a pointer. The state it names lives in a [`World`], a
//! persistent map with structural sharing, so forking one is cloning a value
//! and a continuation captured under one world cannot corrupt another.
//!
//! The world is **monotone**: an entry is never removed. That is what makes a
//! `CellId` unable to dangle, and it is what lets a continuation captured
//! inside a `with_cell` region be resumed outside it without the region having
//! to be forbidden. See `docs/adr/0005`.
//!
//! The invariant every operation here exists to protect is that **no two worlds
//! derived from a common ancestor can observe each other's writes**. It holds
//! structurally rather than by convention: the only way to change a world is
//! through `&mut World`, and no operation hands out a `&mut Value` or any other
//! interior-mutable view of an entry, so a `Value` read out of one world can
//! never be a channel back into it. There is deliberately no `get_mut`.

use crate::value::Value;
use rpds::RedBlackTreeMap;
use std::fmt;

/// Dense and allocated per world lineage. Two worlds forked from a common
/// ancestor may hand out the same id for different cells; that is sound only
/// because the machine holds exactly one world at a time and no operation
/// carries a value from one fork into a sibling.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct CellId(pub u32);

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Default)]
pub struct World {
    cells: RedBlackTreeMap<CellId, Value>,
    /// The next id to hand out. Carried across a fork so two forks of one world
    /// never reuse an ancestor's id for something else.
    next: u32,
}

impl World {
    pub fn new() -> World {
        World::default()
    }

    /// O(1) — structural sharing, not a copy. This is the whole of "fork a
    /// fixture per test".
    #[must_use = "a fork is a value; dropping it discards the fork"]
    pub fn fork(&self) -> World {
        self.clone()
    }

    /// Panics only when the id space is exhausted, which needs 2³² live entries
    /// and therefore hundreds of gigabytes, since nothing is ever removed. Use
    /// [`World::try_alloc`] where that has to be a diagnostic instead.
    pub fn alloc(&mut self, initial: Value) -> CellId {
        self.try_alloc(initial)
            .expect("a world lineage has 2^32 cell ids and cannot exhaust them before memory")
    }

    /// `None` once the id space is exhausted. Wrapping the counter would give
    /// two live cells one key, which is the single thing a world may never do,
    /// so exhaustion is refused rather than absorbed.
    pub fn try_alloc(&mut self, initial: Value) -> Option<CellId> {
        let id = CellId(self.next);
        self.next = self.next.checked_add(1)?;
        self.cells.insert_mut(id, initial);
        Some(id)
    }

    pub fn get(&self, id: CellId) -> Option<&Value> {
        self.cells.get(&id)
    }

    /// `false` when the id is not in this world, which the caller must report
    /// rather than ignore.
    pub fn set(&mut self, id: CellId, value: Value) -> bool {
        if !self.cells.contains_key(&id) {
            return false;
        }
        self.cells.insert_mut(id, value);
        true
    }

    /// The persistent form of [`World::set`], for a caller that wants to keep
    /// the prior world as a value. It refuses an id this world does not hold
    /// for the same reason `set` does: inserting one would resurrect a key the
    /// allocator will later hand out again, and the two cells would alias.
    #[must_use = "`with` returns the updated world; the receiver is unchanged"]
    pub fn with(&self, id: CellId, value: Value) -> World {
        if !self.cells.contains_key(&id) {
            return self.clone();
        }
        World {
            cells: self.cells.insert(id, value),
            next: self.next,
        }
    }

    pub fn contains(&self, id: CellId) -> bool {
        self.cells.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.cells.size()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Ascending by id, so two runs over one world iterate identically.
    pub fn cells(&self) -> impl Iterator<Item = (CellId, &Value)> {
        self.cells.iter().map(|(k, v)| (*k, v))
    }

    /// Two worlds forked from one ancestor agree on every id below this mark
    /// and may disagree on every id at or above it.
    pub fn high_water(&self) -> u32 {
        self.next
    }
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.cells()).finish()
    }
}

/// A seeded world and the value a test reaches it through — what "build the
/// fixture once, fork it per test" produces.
///
/// The pair is the type because the two halves are only meaningful together: a
/// forked world needs a handle from its own lineage, and a handle needs the
/// world its ids were allocated in. [`Fixture::fork`] hands out both at once so
/// they cannot be mismatched.
#[derive(Clone)]
pub struct Fixture {
    world: World,
    handle: Value,
}

impl Fixture {
    /// Runs `seed` once against a fresh world. Whatever it returns is the
    /// handle — a cell, a record of cells, a closure over them.
    pub fn build(seed: impl FnOnce(&mut World) -> Value) -> Fixture {
        let mut world = World::new();
        let handle = seed(&mut world);
        Fixture { world, handle }
    }

    pub fn from_parts(world: World, handle: Value) -> Fixture {
        Fixture { world, handle }
    }

    /// O(1) in the size of the fixture.
    #[must_use = "a fork is a value; dropping it discards the fork"]
    pub fn fork(&self) -> (World, Value) {
        (self.world.fork(), self.handle.clone())
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn handle(&self) -> &Value {
        &self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cont::{Frame, Prompt, Stack};
    use crate::env::Env;
    use ply_span::Span;
    use ply_syntax::ast::BinOp;
    use std::rc::Rc;
    use std::sync::Arc;

    /// A value whose payload is behind an `Arc`, so `Arc::strong_count` reports
    /// whether the world copied it or shared it.
    fn payload(n: i64) -> (Arc<Vec<Value>>, Value) {
        let items = Arc::new(vec![Value::Int(n)]);
        (Arc::clone(&items), Value::List(items))
    }

    fn int_of(world: &World, id: CellId) -> i64 {
        match world.get(id) {
            Some(Value::Int(i)) => *i,
            other => panic!("expected an Int in {id}, found {other:?}"),
        }
    }

    #[test]
    fn a_fork_does_not_see_writes_made_to_its_parent() {
        let mut base = World::new();
        let c = base.alloc(Value::Int(1));

        let mut forked = base.fork();
        forked.set(c, Value::Int(2));

        assert_eq!(int_of(&base, c), 1);
        assert_eq!(int_of(&forked, c), 2);
    }

    #[test]
    fn a_parent_written_after_the_fork_does_not_disturb_the_fork() {
        let mut base = World::new();
        let c = base.alloc(Value::Int(1));
        let forked = base.fork();

        base.set(c, Value::Int(99));

        assert_eq!(int_of(&forked, c), 1);
        assert_eq!(int_of(&base, c), 99);
    }

    #[test]
    fn sibling_forks_never_observe_each_others_writes() {
        let mut base = World::new();
        let shared = base.alloc(Value::Int(0));

        let mut a = base.fork();
        let mut b = base.fork();
        a.set(shared, Value::Int(1));
        b.set(shared, Value::Int(2));
        let a_only = a.alloc(Value::Int(10));
        let b_only = b.alloc(Value::Int(20));

        assert_eq!(int_of(&a, shared), 1);
        assert_eq!(int_of(&b, shared), 2);
        assert_eq!(int_of(&base, shared), 0);

        // The ids collide, which is exactly why nothing may carry a value from
        // one fork into a sibling — each world answers with its own cell.
        assert_eq!(a_only, b_only);
        assert_eq!(int_of(&a, a_only), 10);
        assert_eq!(int_of(&b, b_only), 20);
        assert!(!base.contains(a_only));
    }

    #[test]
    fn a_fork_allocates_without_disturbing_its_parent() {
        let mut base = World::new();
        base.alloc(Value::Int(1));

        let mut forked = base.fork();
        let extra = forked.alloc(Value::Int(7));

        assert_eq!(base.len(), 1);
        assert_eq!(forked.len(), 2);
        assert!(base.get(extra).is_none());
    }

    /// Two forks allocating in parallel *do* collide on the id, which is why
    /// nothing may carry a value from one fork into a sibling.
    #[test]
    fn sibling_forks_reuse_the_same_next_id() {
        let mut base = World::new();
        base.alloc(Value::Int(0));

        let mut a = base.fork();
        let mut b = base.fork();
        assert_eq!(a.alloc(Value::Int(1)), b.alloc(Value::Int(2)));
    }

    #[test]
    fn high_water_marks_the_boundary_between_shared_and_private_ids() {
        let mut base = World::new();
        base.alloc(Value::Unit);
        base.alloc(Value::Unit);
        let mark = base.high_water();

        let mut forked = base.fork();
        let private = forked.alloc(Value::Unit);

        assert_eq!(mark, 2);
        assert!(private.0 >= mark);
        for (id, _) in base.cells() {
            assert!(id.0 < mark);
        }
    }

    #[test]
    fn setting_an_id_this_world_does_not_hold_is_refused() {
        let mut w = World::new();
        assert!(!w.set(CellId(4), Value::Unit));
    }

    #[test]
    fn with_leaves_the_world_it_was_called_on_alone() {
        let mut base = World::new();
        let c = base.alloc(Value::Int(1));

        let next = base.with(c, Value::Int(2));

        assert_eq!(int_of(&base, c), 1);
        assert_eq!(int_of(&next, c), 2);
        assert_eq!(next.high_water(), base.high_water());
    }

    /// Inserting an unheld id would resurrect a key the allocator hands out
    /// later, and the two cells would alias.
    #[test]
    fn with_refuses_an_id_this_world_does_not_hold() {
        let w = World::new();
        let out = w.with(CellId(7), Value::Int(1));
        assert!(!out.contains(CellId(7)));
        assert_eq!(out.len(), 0);
        assert_eq!(out.high_water(), 0);
    }

    #[test]
    fn ids_are_never_reused_within_a_lineage() {
        let mut w = World::new();
        let a = w.alloc(Value::Unit);
        let b = w.alloc(Value::Unit);
        let mut forked = w.fork();
        let c = forked.alloc(Value::Unit);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn an_exhausted_id_space_is_refused_rather_than_wrapped_into_an_alias() {
        let mut w = World {
            cells: RedBlackTreeMap::new(),
            next: u32::MAX,
        };
        assert!(w.try_alloc(Value::Unit).is_none());
        assert_eq!(w.len(), 0);
        assert_eq!(w.high_water(), u32::MAX);
    }

    #[test]
    fn iteration_is_by_ascending_id() {
        let mut w = World::new();
        for i in 0..8 {
            w.alloc(Value::Int(i));
        }
        let ids: Vec<u32> = w.cells().map(|(id, _)| id.0).collect();
        assert_eq!(ids, (0..8).collect::<Vec<_>>());
    }

    /// A fork copies no entry. `Arc::strong_count` is the witness: a deep copy
    /// would clone every payload and every count would rise.
    #[test]
    fn forking_shares_every_value_rather_than_copying_it() {
        let mut base = World::new();
        let mut kept = Vec::new();
        for i in 0..512 {
            let (arc, value) = payload(i);
            kept.push((base.alloc(value), arc));
        }
        for (_, arc) in &kept {
            assert_eq!(Arc::strong_count(arc), 2, "the world and this test hold it");
        }

        let forked = base.fork();

        assert_eq!(forked.len(), base.len());
        for (_, arc) in &kept {
            assert_eq!(
                Arc::strong_count(arc),
                2,
                "a fork that copied a payload would have raised this"
            );
        }
    }

    /// Path copying replaces the entries on one root-to-leaf path and shares
    /// every other, so a write in a fork leaves the other 511 payloads shared.
    #[test]
    fn a_write_in_a_fork_copies_no_value_but_its_own() {
        let mut base = World::new();
        let mut kept = Vec::new();
        for i in 0..512 {
            let (arc, value) = payload(i);
            kept.push((base.alloc(value), arc));
        }

        let mut forked = base.fork();
        let target = kept[256].0;
        assert!(forked.set(target, Value::Int(-1)));

        for (id, arc) in &kept {
            assert_eq!(
                Arc::strong_count(arc),
                2,
                "cell {id} was copied by a write to {target}"
            );
        }
        // The parent still points at the very allocation it did before the
        // write: isolation stated in terms of the payload rather than the map.
        match base.get(target) {
            Some(Value::List(items)) => assert!(Arc::ptr_eq(items, &kept[256].1)),
            other => panic!("the parent's cell was disturbed: {other:?}"),
        }
        assert_eq!(int_of(&forked, target), -1);
    }

    #[test]
    fn a_deep_fork_chain_sees_exactly_its_own_ancestors_writes() {
        const DEPTH: usize = 2_000;

        let mut root = World::new();
        let shared: Vec<CellId> = (0..DEPTH).map(|_| root.alloc(Value::Int(0))).collect();

        let mut chain = vec![root];
        for level in 0..DEPTH {
            let mut next = chain[level].fork();
            assert!(next.set(shared[level], Value::Int(level as i64 + 1)));
            chain.push(next);
        }

        for (level, world) in chain.iter().enumerate() {
            for (i, id) in shared.iter().enumerate() {
                let expected = if i < level { i as i64 + 1 } else { 0 };
                assert_eq!(int_of(world, *id), expected, "level {level}, cell {i}");
            }
        }
    }

    /// A continuation captures control, and control holds `Value`s — which hold
    /// cell *keys*. Which world a resumption reads is therefore decided by the
    /// world it is resumed in, and a world held beside the continuation is a
    /// value that later writes cannot reach.
    #[test]
    fn a_world_held_beside_a_captured_continuation_is_unaffected_by_later_writes() {
        let mut world = World::new();
        let c = world.alloc(Value::Int(1));

        let prompt = Rc::new(Prompt {
            clauses: Rc::new(Vec::new()),
            effects: Rc::new(Vec::new()),
            ret: None,
            env: Env::empty(),
            module: 0,
            span: Span::DUMMY,
        });
        let stack = Stack::new().push_prompt(prompt).push(Frame::BinaryApply {
            op: BinOp::Add,
            lhs: Value::Cell(c),
            lhs_span: Span::DUMMY,
            rhs_span: Span::DUMMY,
            span: Span::DUMMY,
        });
        let (k, _below) = stack.capture(1, 0);
        let at_capture = world.fork();

        world.set(c, Value::Int(2));
        let extra = world.alloc(Value::Int(3));

        assert_eq!(int_of(&at_capture, c), 1);
        assert!(!at_capture.contains(extra));
        assert_eq!(int_of(&world, c), 2);
        assert_eq!(k.frames(), 1);
    }

    #[test]
    fn a_fixture_gives_every_fork_the_seed_and_nobody_elses_writes() {
        let fixture = Fixture::build(|w| {
            let users = w.alloc(Value::list(vec![Value::str("ada"), Value::str("grace")]));
            let counter = w.alloc(Value::Int(0));
            Value::list(vec![Value::Cell(users), Value::Cell(counter)])
        });

        let cells_of = |handle: &Value| match handle {
            Value::List(items) => items
                .iter()
                .map(|v| v.as_cell(Span::DUMMY, "a fixture handle").expect("a cell"))
                .collect::<Vec<_>>(),
            other => panic!("expected the handle list, found {other:?}"),
        };

        let mut worlds = Vec::new();
        for i in 0..64 {
            let (mut world, handle) = fixture.fork();
            let ids = cells_of(&handle);
            assert_eq!(int_of(&world, ids[1]), 0, "every fork starts from the seed");
            assert!(world.set(ids[1], Value::Int(i)));
            worlds.push((world, ids));
        }

        for (i, (world, ids)) in worlds.iter().enumerate() {
            assert_eq!(int_of(world, ids[1]), i as i64);
            assert_eq!(world.len(), fixture.world().len());
        }
        assert_eq!(int_of(fixture.world(), cells_of(fixture.handle())[1]), 0);
    }
}
