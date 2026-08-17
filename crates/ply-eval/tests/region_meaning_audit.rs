//! The observables that decide whether the region model preserved meaning.
//!
//! ADR 0017's governing property is that **program meaning does not change**:
//! representation and cost move, semantics do not. That property is only
//! checkable against programs whose answer *differs* between the two candidate
//! readings, so this file is exactly those programs and nothing else.
//!
//! Every test here asserts the answer ADR 0005 §3 fixed and the current engines
//! produce. Each one names, in its own doc comment, the number the alternative
//! reading — snapshot the region's arena at capture and restore it at every
//! resumption — would produce instead. A file of programs that answer the same
//! under both readings would prove nothing; these are the discriminators.
//!
//! **`--engine both` cannot stand in for this.** That oracle compares the
//! tree-walker against the machine. Both hold the same state representation, so
//! a change to the memory model moves both of them together and the comparison
//! stays green whatever it did to meaning. The only oracle for "meaning did not
//! move" is a fixed expected value, written down before the change, which is
//! what this file is.

use ply_core::check_program;
use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Interp, Machine, Plan, Seed, explore};
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("meaning.ply", src.to_string());
    let program = match parse_program([(id, ModuleName::from_dotted("meaning"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&program).expect("the probe must resolve");
    (program, resolved)
}

/// Runs every test on both engines and requires all of them to pass.
///
/// A probe that fails to compile, or that both engines refuse in the same way,
/// would "agree" and prove nothing about the observable it was written for, so
/// passing is asserted separately from agreeing.
#[track_caller]
fn holds(src: &str) {
    let (program, resolved) = load(src);
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}\n--- program ---\n{src}");

    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    let count = machine.test_count();
    assert!(count > 0, "this probe declares no test\n{src}");
    for i in 0..count {
        if let Err(d) = machine.eval_test(i) {
            panic!(
                "probe {i} (`{}`) must pass: [{}] {}\n{src}",
                machine.test_name(i).unwrap_or("?"),
                d.code,
                d.message
            );
        }
    }
}

const STATE: &str = r#"
effect state {
  read  get[s]()        -> Int
  write put[s](v: Int)  -> Unit
}
"#;

/// **ADR 0005 §3.1's canonical state handler, in the general clause form.**
///
/// The `put` clause writes the cell and *then* resumes. Threaded, the resumed
/// computation runs after the write and `get` answers `5`. Under
/// snapshot-at-capture the clause's own write is discarded before the
/// computation that asked for it ever runs, and the answer is `0`.
///
/// ADR 0017 §3 lists exactly this shape as its "one resumption — the ordinary
/// case" and claims a `shared` region's snapshot reading is "indistinguishable
/// from ADR 0005". This program is the counterexample to that claim, and it is
/// the reason ADR 0005 §3.1 rejected snapshotting: it does not merely cost
/// something, it makes a cell-backed state handler unwritable.
#[test]
fn a_general_clause_write_is_visible_to_the_computation_it_resumes() {
    holds(&format!(
        r#"{STATE}
test "put then get answers what was put" {{
  with_cell[s](0) {{ c ->
    assert_eq(
      handle {{ state.put[s](5); state.get[s]() }} with {{
        state.get[s]() resume k -> k(cell_get(c)),
        state.put[s](v) resume k -> {{ cell_set(c, v); k(()) }},
        return x -> x
      }},
      5)
  }}
}}
"#
    ));
}

/// The same handler in the **tail-resumptive** form, which is the shape every
/// handler in the standard library and the examples is written in.
///
/// `op(x) -> e` is `op(x) resume k -> k(e)`, so the machine captures a
/// continuation here too — ADR 0005 §1.3's `Perform` rule runs `K.capture(n)`
/// for both clause forms. A region kind inferred from "is a continuation
/// captured across this region" therefore classifies **every** region
/// containing a `handle` as `shared`, and snapshot-at-capture then discards
/// this clause's write exactly as it discards the general clause's above.
///
/// Threaded: `5`. Snapshot: `0`.
#[test]
fn a_tail_resumptive_clause_write_is_visible_to_the_computation_it_resumes() {
    holds(&format!(
        r#"{STATE}
test "a tail-resumptive put is seen by the following get" {{
  with_cell[s](0) {{ c ->
    assert_eq(
      handle {{ state.put[s](5); state.get[s]() }} with {{
        state.get[s]() -> cell_get(c),
        state.put[s](v) -> cell_set(c, v),
        return x -> x
      }},
      5)
  }}
}}
"#
    ));
}

/// **ADR 0005 §3.2's "resumes twice", which ADR 0017 §3 as amended requires the
/// same answer for.**
///
/// One threaded state: the first branch increments the cell to `1`, the second
/// resumption starts from that and reaches `2`, so the total is `1 + 2*10 = 21`
/// and the cell ends at `2`. Under snapshot-at-capture both branches see `0`,
/// the total is `1 + 1*10 = 11` and the cell ends at `1`.
///
/// The threaded numbers are the ones asserted. ADR 0017 §3's first draft asked
/// for the snapshot ones and was retracted, because restoring the region at a
/// resumption discards the clause's own write and makes every cell-backed state
/// handler unwritable; a reader tempted to re-propose it should read the
/// amendment rather than this file.
#[test]
fn two_resumptions_thread_one_state_rather_than_branching_it() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

test "the second resumption starts from the first one's writes" {
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
"#,
    );
}

/// **The same discriminator with `handle` and `with_cell` swapped**, which is
/// the shape ADR 0017 §3 writes its two-resumption example in.
///
/// The cell is allocated before the capture, so one cell serves both
/// resumptions and the machine confirms both write it: the total is `21`,
/// exactly as when the region encloses the handler. This is the program
/// `ply_eval::region_kind` used to answer `unique` for — a claim that the
/// region's slots may go back to the bump pointer at its close, made about a
/// region the enclosing handler resumes into twice.
#[test]
fn two_resumptions_thread_one_region_the_enclosing_handler_answers_for() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

test "the region under an enclosing handler is written by both resumptions" {
  let total = handle {
    with_cell[trace](0) { c -> {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { cell_get(c) } else { cell_get(c) * 10 }
    } }
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  };
  assert_eq(total, 21)
}
"#,
    );
}

/// Per-resumption state is what a handler *builds*, not what the machine
/// imposes — ADR 0005 §3.3.
///
/// The direction matters more than either answer: a handler can build snapshot
/// semantics out of threaded semantics in four lines, and cannot build threaded
/// semantics out of snapshot semantics at all. That asymmetry is the argument
/// against making snapshot the default, and it is only checkable if the
/// save-and-restore idiom keeps working, which is what this asserts.
#[test]
fn a_handler_builds_per_branch_state_out_of_threaded_state() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

test "save and restore around each resumption gives both branches one start" {
  with_cell[trace](0) { c -> {
    let total = handle {
      cell_set(c, cell_get(c) + 1);
      if amb.flip[coin]() { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> {
        let before = cell_get(c);
        let a = k(true);
        cell_set(c, before);
        let b = k(false);
        a + b
      },
      return x -> x
    };
    assert_eq(total, 30);
    assert_eq(cell_get(c), 1)
  } }
}
"#,
    );
}

/// **ADR 0005 required test 6, which ADR 0017 §2 turns into a compile error.**
///
/// A continuation is parked in an enclosing region's cell and applied after the
/// `with_cell` whose cell it reads has already returned its value. The world is
/// monotone, so the read succeeds and the answer is `7`.
///
/// ADR 0017 §2 says "capturing it in a closure that outlives the region" is an
/// escape and therefore a type error reported where the value would escape.
/// This program is that shape, it is landed, and it passes. Refusing it is a
/// change of meaning from "answers 7" to "does not compile", which is the one
/// thing §"The property this ADR must not break" forbids — so if the brand is
/// to refuse it, ADR 0005's required test 6 has to be retired by an explicit
/// decision recorded in ADR 0017, not by an implementation that happens to
/// reject it.
#[test]
fn a_continuation_outliving_its_region_still_reads_that_regions_cell() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

type Saved = Nothing | Just((Bool) -> Int)

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
"#,
    );
}

/// A `with_cell` inside a handled body runs once per resumption and each run
/// allocates its own cell, so the two branches of a search do not share their
/// scratch state.
///
/// The tally discriminates: `1 + 1 = 2` when each branch starts from its own
/// zero, and `1 + 2 = 3` when the second branch inherits the first's. This is
/// the property a region arena has to keep when a `shared` region is entered
/// twice — an arena reused across resumptions would answer `3`.
#[test]
fn each_resumption_allocates_its_own_region_cell() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

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
"#,
    );
}

/// A cell reached through a continuation captured inside `map`'s callback.
///
/// `map` is a frame rather than host recursion precisely so this is expressible
/// (ADR 0005 §1.2). The cell count is the discriminator again: `2` threaded,
/// `1` snapshotted.
#[test]
fn a_continuation_captured_inside_map_threads_the_same_state() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

test "a continuation captured inside a map callback produces two lists" {
  with_cell[out](0) { c -> {
    let total = handle {
      let ys = map([1, 2, 3], |x| if x == 2 { if amb.flip[coin]() { 100 } else { 200 } } else { x });
      cell_set(c, cell_get(c) + 1);
      len(ys) * 1000 + fold(ys, 0, |a, b| a + b)
    } with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x };
    assert_eq(total, 3104 + 3204);
    assert_eq(cell_get(c), 2)
  } }
}
"#,
    );
}

/// The shape `std.db.transaction` is: a handler whose clause **does not**
/// resume, installed by a function called from inside a region whose cell the
/// clauses write.
///
/// Two things are pinned. The journal ends `["begin", "one", "abort"]`: the
/// body's writes before the `perform` survive, the clause's own write survives,
/// and the discarded continuation's writes never happened. And the capture that
/// decides this region's kind is **not lexically inside it** — it is in
/// `scoped`, which a region-kind inference has to reach through a call to see.
/// An inference that only looks at a region's own syntax infers `unique` here
/// and skips a snapshot that its own rules said was required.
#[test]
fn a_non_resuming_clause_keeps_the_writes_that_preceded_it() {
    holds(
        r#"
effect tx {
  write begin()               -> Int
  write abort()               -> Int
  write step(name: String)    -> Int
  write rollback(why: String) -> Unit
}

type Outcome = Committed(Int) | RolledBack(String)

fn scoped(body: () -> Int / {tx.write}) -> Outcome / {tx.write} =
  handle { let v = { tx.begin(); body() }; Committed(v) }
  with { tx.rollback(why) resume k -> { tx.abort(); RolledBack(why) } }

test "a rollback discards the continuation and keeps what preceded it" {
  with_cell[journal]([]) { j ->
    handle {
      let out = scoped(|| {
        tx.step("one");
        tx.rollback("no stock");
        tx.step("two");
        7
      });
      assert_eq(out, RolledBack("no stock"));
      assert_eq(cell_get(j), ["begin", "one", "abort"])
    } with {
      tx.begin()   -> { cell_set(j, push(cell_get(j), "begin")); 0 },
      tx.abort()   -> { cell_set(j, push(cell_get(j), "abort")); 0 },
      tx.step(n)   -> { cell_set(j, push(cell_get(j), n)); 0 },
    }
  }
}
"#,
    );
}

/// W5's collecting trace sink, reduced to its discriminating core.
///
/// Every clause writes the sink cell and tail-resumes, so the record list is
/// built entirely out of writes that a snapshot at each `perform` would throw
/// away. Threaded, the three operations produce three records in order.
/// Snapshotted, each clause's write is undone before the next `perform`, the
/// sink is still empty at the end, and the test reads a length of `0`.
///
/// This is the whole of `std.trace`'s twin, and the same shape appears 30 times
/// across the standard library.
#[test]
fn a_collecting_sink_accumulates_across_handler_boundaries() {
    holds(
        r#"
effect log {
  write note[c](name: String) -> Unit
  write open[c](name: String) -> Int
  write shut[c](id: Int)      -> Unit
}

fn work() -> Unit / {log.write[orders]} = {
  let span = log.open[orders]("place");
  log.note[orders]("counted");
  log.shut[orders](span)
}

test "a collecting handler accumulates every record it was handed" {
  with_cell[sink]([]) { s -> {
    handle { work() } with {
      log.open[orders](n) -> { cell_set(s, push(cell_get(s), n)); 1 },
      log.note[orders](n) -> cell_set(s, push(cell_get(s), n)),
      log.shut[orders](i) -> cell_set(s, push(cell_get(s), "closed")),
    };
    assert_eq(cell_get(s), ["place", "counted", "closed"])
  } }
}
"#,
    );
}

/// **The invariant with the highest stakes: a race is found and reproduced from
/// its seed.**
///
/// Two tasks each read a counter, cross a scheduling point, and write back. The
/// search finds the lost update only because both tasks reach **one** cell in
/// **one** world, so the second task's read can observe — or fail to observe —
/// the first task's write. ADR 0006's first required test is this program.
///
/// ADR 0017 §5 gives every task its own region stack and says values cannot
/// cross tasks. ADR 0017 §3 makes the region `shared`, since a continuation is
/// captured at every one of these `perform`s, and gives each resumption the
/// arena as of capture. Under either rule the two tasks stop being able to
/// clobber each other, the assertion holds under every interleaving, and the
/// search reports `exhaustive` with no failure.
///
/// That is not a slower answer or a wider suspect set. It is a green run on a
/// program with a race in it, produced by a memory model that made the race
/// unrepresentable — the false-green shape this project has found five times.
#[test]
fn two_tasks_sharing_one_cell_can_still_lose_an_update() {
    let src = r#"
effect counter {
  read  get[r]() -> Int
  write put[r](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  let _ = clock.now();
  counter.put[n](seen + 1)
}

test "two increments" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        assert_eq(cell_get(c), 2)
      }
    } with {
      counter.get[n]() -> cell_get(c),
      counter.put[n](v) -> cell_set(c, v),
    }
  }
}
"#;
    let (program, resolved) = load(src);
    let check = check_program(&program, &resolved).expect("the probe must typecheck");
    let at = |seed: &Seed| {
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_seed(seed.clone(), 10_000);
        let outcome = machine.eval_test(0);
        machine
            .simulated()
            .expect("the probe reaches a `simulate` region")
            .interleaving(&outcome)
    };
    let explored = explore(&Plan::default(), &mut |seed: &Seed| at(seed));

    let seed = explored.exploration.failure.clone().expect(
        "the lost update must be found; a memory model that cannot represent it reports green",
    );
    assert!(
        !matches!(at(&seed).verdict, ply_eval::explore::Verdict::Passed),
        "the reported seed must reproduce the failure exactly"
    );
}
