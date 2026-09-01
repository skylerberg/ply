//! The observables that decide whether the region model preserved meaning.

use ply_core::check_program;
use ply_eval::{Machine, Plan, Seed, explore};
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("meaning.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("meaning"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// Runs every test in a probe and requires all of them to pass.
#[track_caller]
fn holds(src: &str) {
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

/// **the canonical state handler's canonical state handler, in the general clause form.**
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

/// The same handler in the **tail-resumptive** form, which is the shape every handler in the
/// standard library and the examples is written in.
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

/// **the two-resumption example's "resumes twice", which the region-kind rule as amended requires the same answer for.**
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

/// **The same discriminator with `handle` and `with_cell` swapped**, which is the shape the region-kind rule
/// writes its two-resumption example in.
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

/// Per-resumption state is what a handler *builds*, not what the machine imposes — per-resumption state, built by the handler.
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

/// **the escape case — a continuation resumed after the region that made its cell returned, which the escape brand turns into a compile error.**
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

/// A `with_cell` inside a handled body runs once per resumption and each run allocates its own
/// cell, so the two branches of a search do not share their scratch state.
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

/// The shape `std.db.transaction` is: a handler whose clause **does not** resume, installed by a
/// function called from inside a region whose cell the clauses write.
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

/// **The invariant with the highest stakes: a race is found and reproduced from its seed.**
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
