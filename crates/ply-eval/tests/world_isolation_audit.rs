//! Adversarial audit of the one property the milestone cannot be wrong about:
//! **no two worlds derived from a common ancestor observe each other's writes.**
//!
//! Everything else in Ply degrades gracefully when it is wrong — a slower
//! schedule, a wider suspect set. This one does not: two tests that can see each
//! other's state produce a flake that survives the cache, and the cache makes it
//! stick. So the attacks here are written to *succeed* if they can, and the
//! assertions state what must remain true when they cannot.
//!
//! Three layers, because a defect can hide in any of them:
//!
//! 1. [`World`] on its own — siblings, ancestors mutated mid-chain, the id
//!    collision that makes carrying a value across a fork unsafe.
//! 2. The machine on real source — a cell smuggled out of its region through
//!    every carrier the language has, and a continuation resumed after the
//!    region that made it returned.
//! 3. The types — an argument from the definitions rather than from a run,
//!    because a test can only sample the executions somebody thought of.

use ply_core::{CheckOutput, Footprint, check_program};
use ply_eval::{CellId, Engine, Interp, Machine, Value, World};
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::marker::PhantomData;

// ------------------------------------------------------------------ harness

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    fn rejected(src: &str) -> Vec<Diagnostic> {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        match check_program(&program, &resolved) {
            Ok(_) => Vec::new(),
            Err(diags) => diags,
        }
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn interp(&self) -> Interp<'_> {
        Interp::new(&self.program, &self.resolved, &self.check)
    }

    fn index_of(&self, name: &str) -> usize {
        self.check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"))
    }

    fn footprint(&self, name: &str) -> &Footprint {
        &self.check.tests[self.index_of(name)].footprint
    }

    /// A world-isolation defect that only one engine has is still a defect, so
    /// nothing here is believed until both engines say it.
    fn run_both(&self, name: &str) -> Result<(), Diagnostic> {
        let index = self.index_of(name);
        let machine = self.machine().eval_test(index);
        let treewalk = self.interp().eval_test(index);
        match (&machine, &treewalk) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(m), Err(t)) => {
                assert_eq!(
                    m.message, t.message,
                    "the engines disagree about why {name:?} failed"
                );
                Err(machine.unwrap_err())
            }
            (Ok(()), Err(t)) => panic!("only the tree-walker failed {name:?}: {t:#?}"),
            (Err(m), Ok(())) => panic!("only the machine failed {name:?}: {m:#?}"),
        }
    }
}

fn int_of(world: &World, id: CellId) -> i64 {
    match world.get(id) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an Int in {id}, found {other:?}"),
    }
}

// ------------------------------------------------- 1. the world on its own

/// The headline property, stated over three siblings rather than two so that a
/// defect that leaks in only one direction cannot hide behind symmetry.
#[test]
fn sibling_forks_writing_one_cell_never_read_each_others_value() {
    let mut ancestor = World::new();
    let shared = ancestor.alloc(Value::Int(0));

    let mut forks: Vec<World> = (0..3).map(|_| ancestor.fork()).collect();
    for (i, fork) in forks.iter_mut().enumerate() {
        assert!(fork.set(shared, Value::Int(i as i64 + 1)));
    }
    // Interleaved a second time: a defect that needs the writes to alternate
    // rather than run in a batch would survive the loop above.
    for i in (0..forks.len()).rev() {
        assert!(forks[i].set(shared, Value::Int(i as i64 + 10)));
        for (j, other) in forks.iter().enumerate() {
            let expected = if j >= i { j as i64 + 10 } else { j as i64 + 1 };
            assert_eq!(
                int_of(other, shared),
                expected,
                "fork {j} after writing {i}"
            );
        }
    }
    assert_eq!(int_of(&ancestor, shared), 0, "the ancestor is untouched");
}

/// A write to a *shared ancestor* is the direction the persistent map makes
/// least obvious: the descendants were forked before the write and hold the
/// same tree nodes it path-copies.
#[test]
fn a_write_to_an_ancestor_reaches_no_descendant_forked_before_it() {
    const DEPTH: usize = 12;

    let mut root = World::new();
    let cells: Vec<CellId> = (0..DEPTH).map(|_| root.alloc(Value::Int(-1))).collect();

    let mut chain = vec![root];
    for level in 1..=DEPTH {
        let next = chain[level - 1].fork();
        chain.push(next);
    }

    // Mutate the middle of the chain *after* every descendant exists.
    let mark = |level: usize, i: usize| (level * 100 + i) as i64;
    for level in [0usize, 3, 7] {
        for (i, id) in cells.iter().enumerate() {
            assert!(chain[level].set(*id, Value::Int(mark(level, i))));
        }
        for (other, world) in chain.iter().enumerate() {
            for (i, id) in cells.iter().enumerate() {
                let seen = int_of(world, *id);
                let expected = if other == level { mark(level, i) } else { -1 };
                assert_eq!(
                    seen, expected,
                    "world {other} observed a write made to world {level}"
                );
            }
        }
        // Put it back, so the next round starts from a chain nobody has written.
        for id in &cells {
            assert!(chain[level].set(*id, Value::Int(-1)));
        }
    }
}

/// A fork taken *after* an earlier write inherits it; one taken before does
/// not. The two together are what "copy-on-write" has to mean.
#[test]
fn a_fork_inherits_exactly_the_writes_made_before_it_was_taken() {
    let mut base = World::new();
    let c = base.alloc(Value::Int(0));

    let before = base.fork();
    base.set(c, Value::Int(1));
    let after = base.fork();
    base.set(c, Value::Int(2));

    assert_eq!(int_of(&before, c), 0);
    assert_eq!(int_of(&after, c), 1);
    assert_eq!(int_of(&base, c), 2);
}

/// The hazard that makes every other test here necessary: two siblings hand out
/// the same `CellId` for different cells, and reading a foreign id succeeds
/// quietly instead of failing. Nothing detects a value that crossed a fork —
/// which is why nothing may carry one, and why the machine's isolation has to
/// be structural rather than checked.
#[test]
fn a_foreign_id_is_answered_by_the_reading_world_and_never_by_its_owner() {
    let base = World::new();
    let mut a = base.fork();
    let mut b = base.fork();

    let in_a = a.alloc(Value::str("a's secret"));
    let in_b = b.alloc(Value::str("b's secret"));
    assert_eq!(in_a, in_b, "siblings reuse the ancestor's high-water mark");

    assert_eq!(a.get(in_b).map(Value::render).unwrap(), "\"a's secret\"");
    assert_eq!(b.get(in_a).map(Value::render).unwrap(), "\"b's secret\"");
    assert!(base.get(in_a).is_none(), "the ancestor gained nothing");

    assert!(a.set(in_b, Value::str("clobbered")));
    assert_eq!(b.get(in_b).map(Value::render).unwrap(), "\"b's secret\"");
}

/// `with` is the persistent form, and the reason it refuses an id the world does
/// not hold: inserting one would resurrect a key the allocator hands out later,
/// and the two cells would alias inside a single world.
#[test]
fn the_persistent_write_cannot_resurrect_an_id_from_a_sibling() {
    let base = World::new();
    let mut a = base.fork();
    let stranger = a.alloc(Value::Int(1));

    let b = base.fork().with(stranger, Value::Int(99));

    assert!(!b.contains(stranger));
    assert_eq!(b.high_water(), base.high_water());
    assert_eq!(int_of(&a, stranger), 1);
}

// ------------------------------------------- 2. the machine on real source

const SMUGGLE: &str = r#"
test "a closure carries the cell out of its region" {
  let read = with_cell[log](41) { c -> || cell_get(c) };
  assert_eq(read(), 41)
}

test "a record of closures carries a readable and a writable view out" {
  let ops = with_cell[log](1) { c -> {get: || cell_get(c), put: |v| cell_set(c, v)} };
  let get = ops.get;
  let put = ops.put;
  put(9);
  assert_eq(get(), 9)
}

test "a smuggled closure writes the world the test is running in" {
  let bump = with_cell[log](0) { c -> || cell_set(c, cell_get(c) + 1) };
  bump();
  bump();
  bump();
  let read = with_cell[log](0) { c -> || cell_get(c) };
  assert_eq(read(), 0)
}
"#;

/// A closure is the one carrier the region check does not inspect: it looks at
/// the body's *type*, and a function type hides the cell in its row. So this
/// runs, and what it must not do is read anything but this run's own world.
#[test]
fn a_cell_smuggled_out_of_its_region_through_a_closure_reads_this_runs_world() {
    let compiled = Compiled::new(SMUGGLE);
    compiled
        .run_both("a closure carries the cell out of its region")
        .expect("the smuggled read answers the region's initial value");
    compiled
        .run_both("a record of closures carries a readable and a writable view out")
        .expect("a smuggled write is visible to a smuggled read of the same cell");
    compiled
        .run_both("a smuggled closure writes the world the test is running in")
        .expect("a second region is a second cell, not the first one again");
}

/// The escape is visible in the footprint, which is what the scheduler then
/// exempts. Worth pinning: ADR 0005 §5 says a `cell` atom in a test's footprint
/// needs a captured continuation, and a closure reaches it without one.
#[test]
fn a_smuggled_cell_leaves_its_atom_in_the_tests_footprint() {
    let compiled = Compiled::new(SMUGGLE);
    let atoms: Vec<String> = compiled
        .footprint("a closure carries the cell out of its region")
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(atoms, vec!["cell.read[log]".to_string()]);

    let mixed: Vec<String> = compiled
        .footprint("a record of closures carries a readable and a writable view out")
        .atoms()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        mixed,
        vec!["cell.read[log]".to_string(), "cell.write[log]".to_string()]
    );
}

/// Two runs of one machine allocate the *same* ids, because each forks the same
/// base world. That is the collision of
/// `a_foreign_id_is_answered_by_the_reading_world_and_never_by_its_owner`,
/// reached through the front door — so the second run must start from the seed
/// and not from whatever the first left behind, even though the first run
/// smuggled a live cell into a closure.
#[test]
fn a_second_run_of_one_machine_reuses_the_ids_and_none_of_the_state() {
    let compiled = Compiled::new(SMUGGLE);
    let mut machine = compiled.machine();
    let index = compiled.index_of("a smuggled closure writes the world the test is running in");

    machine.eval_test(index).expect("the first run passes");
    let first: Vec<(CellId, String)> = machine
        .world()
        .cells()
        .map(|(id, v)| (id, v.render()))
        .collect();
    assert_eq!(
        first,
        vec![(CellId(0), "3".into()), (CellId(1), "0".into())]
    );

    machine.eval_test(index).expect("the second run passes");
    let second: Vec<(CellId, String)> = machine
        .world()
        .cells()
        .map(|(id, v)| (id, v.render()))
        .collect();
    assert_eq!(second, first, "the ids repeat and the values start over");
}

/// A cell in a *constructor argument* is not caught by the region check at all:
/// the variant's field type holds the `Cell`, so the region's result type is
/// `Held` and mentions no region. The escape therefore happens, and what has to
/// hold is the same thing that holds for every other carrier — it is a key into
/// the world this run owns.
#[test]
fn a_cell_in_a_constructor_argument_escapes_its_region_and_still_reads_its_own_world() {
    let compiled = Compiled::new(
        r#"
type Held = Held(Cell<Int>)

test "a constructor carries the cell out of its region" {
  let h = with_cell[log](1) { c -> Held(c) };
  match h { Held(c) -> { cell_set(c, 2); assert_eq(cell_get(c), 2) } }
}
"#,
    );
    compiled
        .run_both("a constructor carries the cell out of its region")
        .expect("the region check does not see a cell inside a variant's field");

    let mut machine = compiled.machine();
    for _ in 0..2 {
        machine.eval_test(0).expect("the test passes");
        assert_eq!(
            machine
                .world()
                .cells()
                .map(|(_, v)| v.render())
                .collect::<Vec<_>>(),
            vec!["2".to_string()],
            "each run wrote its own cell and inherited nothing"
        );
    }
}

/// The boundary of that hole: the region variable in a declared `Cell<T>` field
/// is fixed by the first region that fills it, so a second region using the same
/// type is a mismatch rather than a silent alias between two regions' cells.
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

/// A cell in a list element or a record field *is* caught, because both keep the
/// `Cell` type in the region's result type where `mentions_region` finds it.
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

const RESUMED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

type Saved = Nothing | Just((Bool) -> Int)

test "two resumptions write one cell in one world" {
  with_cell[trace](0) { c -> {
    let total = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { cell_get(c) } else { cell_get(c) * 10 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 21);
    assert_eq(cell_get(c), 2)
  } }
}

test "each resumption allocates its own region cell" {
  with_cell[tally](0) { t -> {
    let total = handle {
      let b = amb.flip[coin]();
      with_cell[scratch](0) { s -> {
        cell_set(s, cell_get(s) + 1);
        cell_set(t, cell_get(t) + cell_get(s));
        cell_get(s)
      } }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(total, 2);
    assert_eq(cell_get(t), 2)
  } }
}

test "a continuation resumed after its region returned reads that region's cell" {
  with_cell[slot](Nothing) { s -> {
    let inner = with_cell[log](7) { c ->
      handle {
        let b = amb.flip[coin]();
        if b { cell_get(c) } else { 0 }
      } with {
        amb.flip[coin]() resume k -> { cell_set(s, Just(k)); 0 },
        return x -> x
      }
    };
    assert_eq(inner, 0);
    match cell_get(s) {
      Just(k) -> assert_eq(k(true), 7),
      Nothing -> assert(false)
    }
  } }
}
"#;

/// ADR 0005 §3.2's "resumes twice", with the second resumption's write landing
/// on the first one's: one threaded world, not a snapshot per resumption. The
/// first branch sees `1`, the second sees `2`, and `21 = 1 + 2 * 10`.
#[test]
fn two_resumptions_of_one_handler_write_one_cell_in_one_world() {
    let compiled = Compiled::new(RESUMED);
    let index = compiled.index_of("two resumptions write one cell in one world");
    let mut machine = compiled.machine();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("the threaded-world reading must hold: {d:#?}"));
    assert_eq!(machine.world().len(), 1, "one region, one cell");
}

/// A `with_cell` *inside* a handled body runs once per resumption, and each run
/// has to allocate its own cell: two resumptions sharing one region cell would
/// be the two branches of a search seeing each other's scratch state. The tally
/// discriminates — it is `1 + 1` when each branch starts from zero and `1 + 2`
/// when the second inherits the first's.
#[test]
fn each_resumption_allocates_its_own_region_cell() {
    let compiled = Compiled::new(RESUMED);
    let index = compiled.index_of("each resumption allocates its own region cell");
    let mut machine = compiled.machine();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("each branch must get its own scratch cell: {d:#?}"));
    assert_eq!(
        machine.world().len(),
        3,
        "the tally, and one scratch cell per resumption — the world is monotone"
    );
}

/// A continuation parked in an enclosing region's cell and resumed after the
/// region whose cell it reads has returned. The world is monotone, so this is a
/// success rather than a dangling read — and the value it answers with is this
/// run's, never a neighbour's.
#[test]
fn a_continuation_resumed_after_its_region_returned_reads_this_runs_cell() {
    let compiled = Compiled::new(RESUMED);
    let index = compiled
        .index_of("a continuation resumed after its region returned reads that region's cell");

    let mut machine = compiled.machine();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("resuming outside the region must succeed: {d:#?}"));
    let first: Vec<String> = machine.world().cells().map(|(_, v)| v.render()).collect();

    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("the second run must also succeed: {d:#?}"));
    let second: Vec<String> = machine.world().cells().map(|(_, v)| v.render()).collect();
    assert_eq!(first, second, "the second run started from the seed again");
}

/// The one place a value *can* cross two worlds is the host API: `call` resets
/// the world and then accepts arguments the caller built during an earlier run.
/// Nothing in the tree does that, and this pins the half of it the runtime can
/// detect — an id the new world does not hold is named rather than read.
///
/// The other half is not detectable and must not be relied on: had the second
/// run already allocated `#0`, the smuggled closure would have read *that* cell
/// and answered plausibly. A `CellId` carries no lineage, so "nothing carries a
/// value across an entry point" is the invariant, not "the runtime will catch
/// it".
#[test]
fn a_cell_carried_across_two_runs_of_one_machine_is_named_and_not_read() {
    let compiled = Compiled::new(
        r#"
fn leak() = with_cell[log](11) { c -> |x: Int| cell_get(c) + x }

fn use_it(f) = f(0)
"#,
    );
    let mut machine = compiled.machine();

    let leaked = machine
        .call("m.leak", vec![], ply_span::Span::DUMMY)
        .expect("a closure over the region's cell");
    let smuggled = machine
        .call("m.use_it", vec![leaked], ply_span::Span::DUMMY)
        .expect_err("the second run has no cell #0");

    assert_eq!(smuggled.code, ply_span::codes::INTERNAL_ERROR);
    assert!(
        smuggled.message.contains("does not belong to the world"),
        "{smuggled:#?}"
    );
}

/// The teeth behind every "they never observed each other" assertion elsewhere:
/// tests that fork one world all allocate their first cell at *the same key*.
/// Isolation is therefore doing real work rather than being an accident of two
/// tests happening to name different ids.
#[test]
fn separate_tests_write_the_very_same_cell_id_in_their_own_worlds() {
    let mut src = String::new();
    for i in 0..4 {
        src.push_str(&format!(
            "test \"contender {i}\" {{ with_cell[table]({i}) {{ c -> cell_set(c, {i} * 7) }} }}\n"
        ));
    }
    let compiled = Compiled::new(&src);
    let mut machine = compiled.machine();

    for i in 0..4 {
        machine.eval_test(i).expect("the test passes");
        let cells: Vec<(CellId, String)> = machine
            .world()
            .cells()
            .map(|(id, v)| (id, v.render()))
            .collect();
        assert_eq!(
            cells,
            vec![(CellId(0), (i * 7).to_string())],
            "every contender owns cell #0 and nobody else's value"
        );
    }
}

/// The base world is the fixture, and every entry point forks it. A test that
/// writes must therefore leave the base exactly as it found it, or the *next*
/// test inherits the write — the interference this milestone exists to make
/// impossible.
#[test]
fn a_seeded_base_world_survives_every_test_that_forks_it() {
    let compiled = Compiled::new(SMUGGLE);
    let mut seed = World::new();
    let seeded = seed.alloc(Value::Int(1_000));

    let mut machine = compiled.machine();
    machine.set_base_world(seed.fork());

    for _ in 0..3 {
        for name in [
            "a closure carries the cell out of its region",
            "a smuggled closure writes the world the test is running in",
        ] {
            let index = compiled.index_of(name);
            machine.eval_test(index).expect("the test passes");
            assert_eq!(
                int_of(machine.world(), seeded),
                1_000,
                "{name} disturbed the seed it forked"
            );
        }
    }
    assert_eq!(int_of(&seed, seeded), 1_000, "the base itself is untouched");
}

// --------------------------------------------------- 3. the types themselves

/// A test can only sample the executions somebody thought of. This is the
/// argument from the definitions: a `Value` that could be shared across two
/// worlds would need either interior mutability — a `RefCell`, a `Cell`, an
/// atomic — or a way to reach one world from another thread. It has neither, and
/// `Value: !Send` is what makes the second half unrepresentable rather than
/// merely unwritten.
///
/// `rpds` is parameterized over the shared-pointer kind and `World` uses the
/// `Rc` one, so a `World` that became `Send` would be a data race on a
/// non-atomic refcount, not merely a scheduling surprise. If someone adds an
/// `unsafe impl Send`, this fails.
#[test]
fn a_world_and_the_values_in_it_cannot_cross_a_thread() {
    assert!(!is_send!(World), "World must stay thread-confined");
    assert!(!is_send!(Value), "Value must stay thread-confined");
    assert!(!is_send!(ply_eval::Continuation));
    assert!(!is_send!(ply_eval::Stack));
    assert!(!is_send!(ply_eval::Env));
    assert!(!is_send!(ply_eval::Fixture));
    assert!(!is_send!(Machine<'static>));
    assert!(!is_send!(Interp<'static>));
    // The sanity half: the probe reports `true` for something that is `Send`.
    assert!(is_send!(CellId));
    assert!(is_send!(Engine));
}

/// Autoref specialization: the inherent method exists only when `T: Send`, and
/// the trait method on `&Probe<T>` needs one more autoref step, so it is chosen
/// exactly when the inherent one does not apply. It has to be a macro rather
/// than a generic function, because resolution inside a generic body happens
/// once, against a `T` that satisfies nothing.
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
