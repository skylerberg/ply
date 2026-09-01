//! Adversarial probes for the one property the region model may not break.

use ply_core::check_program;
use ply_eval::{Machine, Plan, Seed, explore};
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("adversarial.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("adversarial"), src)]) {
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

const AMB: &str = r#"
effect amb { read flip[coin]() -> Bool }
"#;

const SAVED: &str = r#"
type Saved = Nothing | Just((Bool) -> Int)
"#;

/// Four leaves, from two captures nested inside one another.
#[test]
fn four_leaves_from_nested_captures_thread_one_cell() {
    holds(&format!(
        r#"{AMB}
test "two nested flips leave the trace cell at four" {{
  with_cell[trace](0) {{ c -> {{
    let total = handle {{
      let a = amb.flip[coin]();
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      cell_get(c)
    }} with {{
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    }};
    assert_eq(total, 10);
    assert_eq(cell_get(c), 4)
  }} }}
}}
"#
    ));
}

/// Eight leaves, so that a defect symmetric in two or four branches has nowhere left to hide.
#[test]
fn eight_leaves_from_three_nested_captures_thread_one_cell() {
    holds(&format!(
        r#"{AMB}
test "three nested flips leave the trace cell at eight" {{
  with_cell[trace](0) {{ c -> {{
    let total = handle {{
      let a = amb.flip[coin]();
      let b = amb.flip[coin]();
      let d = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      cell_get(c)
    }} with {{
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    }};
    assert_eq(total, 36);
    assert_eq(cell_get(c), 8)
  }} }}
}}
"#
    ));
}

/// One continuation resumed four times from a single clause body.
#[test]
fn one_capture_resumed_four_times_moves_the_cell_four_times() {
    holds(&format!(
        r#"{AMB}
test "four resumptions of one capture" {{
  with_cell[trace](0) {{ c -> {{
    let total = handle {{
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b {{ cell_get(c) }} else {{ cell_get(c) * 10 }}
    }} with {{
      amb.flip[coin]() resume k -> k(true) + k(false) + k(true) + k(false),
      return x -> x
    }};
    assert_eq(total, 1 + 20 + 3 + 40);
    assert_eq(cell_get(c), 4)
  }} }}
}}
"#
    ));
}

/// Two continuations parked in cells and resumed in the **reverse** of the order they were captured
/// in.
#[test]
fn two_parked_continuations_resumed_in_reverse_capture_order_thread_one_cell() {
    holds(&format!(
        r#"{AMB}{SAVED}
test "the later capture resumed first still threads the one cell" {{
  with_cell[first](Nothing) {{ sa ->
  with_cell[second](Nothing) {{ sb ->
  with_cell[trace](0) {{ c -> {{
    let a = handle {{
      let x = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      cell_get(c)
    }} with {{
      amb.flip[coin]() resume k -> {{ cell_set(sa, Just(k)); 0 }},
      return x -> x
    }};
    let b = handle {{
      let y = amb.flip[coin]();
      cell_set(c, cell_get(c) + 10);
      cell_get(c)
    }} with {{
      amb.flip[coin]() resume k -> {{ cell_set(sb, Just(k)); 0 }},
      return x -> x
    }};
    assert_eq(a, 0);
    assert_eq(b, 0);
    assert_eq(cell_get(c), 0);
    let rb = match cell_get(sb) {{ Just(k) -> k(true), Nothing -> 0 }};
    assert_eq(rb, 10);
    let ra = match cell_get(sa) {{ Just(k) -> k(true), Nothing -> 0 }};
    assert_eq(ra, 11);
    assert_eq(cell_get(c), 11)
  }} }} }} }}
}}
"#
    ));
}

/// The same continuation applied twice from outside the handler that made it, long after that
/// handler returned.
#[test]
fn a_parked_continuation_applied_twice_from_outside_threads_both_times() {
    holds(&format!(
        r#"{AMB}{SAVED}
test "one parked continuation, two applications" {{
  with_cell[slot](Nothing) {{ s ->
  with_cell[trace](0) {{ c -> {{
    let out = handle {{
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      cell_get(c)
    }} with {{
      amb.flip[coin]() resume k -> {{ cell_set(s, Just(k)); 0 }},
      return x -> x
    }};
    assert_eq(out, 0);
    match cell_get(s) {{
      Just(k) -> {{ assert_eq(k(true), 1); assert_eq(k(false), 2) }},
      Nothing -> assert(false)
    }};
    assert_eq(cell_get(c), 2)
  }} }} }}
}}
"#
    ));
}

/// **The reclamation probe.**
#[test]
fn a_slot_a_parked_continuation_reads_survives_the_regions_opened_over_it() {
    holds(&format!(
        r#"{AMB}{SAVED}
fn scratch(n: Int) -> Int = with_cell[reuse](n) {{ d -> {{ cell_set(d, n * 2); cell_get(d) }} }}

fn churn(n: Int) -> Int = if n == 0 {{ 0 }} else {{ scratch(n) + churn(n - 1) }}

test "a region opened over a reclaimed position does not steal a pinned slot" {{
  with_cell[slot](Nothing) {{ s -> {{
    let a = with_cell[first](7) {{ c ->
      handle {{
        let b = amb.flip[coin]();
        if b {{ cell_get(c) }} else {{ 0 }}
      }} with {{
        amb.flip[coin]() resume k -> {{ cell_set(s, Just(k)); 0 }},
        return x -> x
      }}
    }};
    assert_eq(a, 0);
    assert_eq(churn(300), 90300);
    match cell_get(s) {{
      Just(k) -> assert_eq(k(true), 7),
      Nothing -> assert(false)
    }}
  }} }}
}}
"#
    ));
}

/// A cell of an **enclosing** region, written from a resumption that runs after the *inner*
/// region's lexical close.
#[test]
fn a_resumption_after_an_inner_close_writes_the_enclosing_regions_cell() {
    holds(&format!(
        r#"{AMB}{SAVED}
test "the inner region's cell is read and the outer region's is written" {{
  with_cell[outer](0) {{ o ->
  with_cell[slot](Nothing) {{ s -> {{
    let inner = with_cell[log](7) {{ c ->
      handle {{
        let b = amb.flip[coin]();
        cell_set(o, cell_get(o) + cell_get(c));
        cell_get(o)
      }} with {{
        amb.flip[coin]() resume k -> {{ cell_set(s, Just(k)); 0 }},
        return x -> x
      }}
    }};
    assert_eq(inner, 0);
    assert_eq(cell_get(o), 0);
    match cell_get(s) {{
      Just(k) -> assert_eq(k(true), 7),
      Nothing -> assert(false)
    }};
    assert_eq(cell_get(o), 7);
    match cell_get(s) {{
      Just(k) -> assert_eq(k(true), 14),
      Nothing -> assert(false)
    }};
    assert_eq(cell_get(o), 14)
  }} }} }}
}}
"#
    ));
}

/// A region entered once per iteration of a recursion, with a continuation captured in the
/// **first** iteration resumed after the last one has closed.
#[test]
fn a_continuation_from_the_first_iteration_reads_its_cell_after_the_last() {
    holds(&format!(
        r#"{AMB}{SAVED}
fn step(n: Int) -> Int = with_cell[loop](n) {{ c -> {{ cell_set(c, cell_get(c) + 1); cell_get(c) }} }}

fn spin(n: Int) -> Int = if n == 0 {{ 0 }} else {{ step(n) + spin(n - 1) }}

test "the first iteration's cell outlives every iteration after it" {{
  with_cell[slot](Nothing) {{ s -> {{
    let a = with_cell[held](41) {{ c ->
      handle {{
        let b = amb.flip[coin]();
        if b {{ cell_get(c) }} else {{ 0 }}
      }} with {{
        amb.flip[coin]() resume k -> {{ cell_set(s, Just(k)); 0 }},
        return x -> x
      }}
    }};
    assert_eq(a, 0);
    assert_eq(spin(50), 1325);
    match cell_get(s) {{
      Just(k) -> assert_eq(k(true), 41),
      Nothing -> assert(false)
    }}
  }} }}
}}
"#
    ));
}

/// A capture that crosses a region's **close**, resumed twice — so the close runs twice against one
/// open.
#[test]
fn a_capture_that_crosses_a_close_may_be_resumed_after_that_close_has_run() {
    holds(&format!(
        r#"{AMB}{SAVED}
effect ask {{ read get[env]() -> Int }}

fn scratch(n: Int) -> Int = with_cell[reuse](n) {{ d -> {{ cell_set(d, n * 2); cell_get(d) }} }}

fn churn(n: Int) -> Int = if n == 0 {{ 0 }} else {{ scratch(n) + churn(n - 1) }}

test "a close crossed by a capture runs once per resumption" {{
  with_cell[slot](Nothing) {{ s -> {{
    let a = handle {{
      with_cell[r](7) {{ c ->
        handle {{ cell_set(c, cell_get(c) + 1); ask.get[env]() + cell_get(c) }}
        with {{ ask.get[env]() -> if amb.flip[coin]() {{ 100 }} else {{ 200 }} }}
      }}
    }} with {{
      amb.flip[coin]() resume k -> {{ cell_set(s, Just(k)); 0 }},
      return x -> x
    }};
    assert_eq(a, 0);
    assert_eq(churn(300), 90300);
    match cell_get(s) {{ Just(k) -> assert_eq(k(true), 108), Nothing -> assert(false) }};
    match cell_get(s) {{ Just(k) -> assert_eq(k(false), 208), Nothing -> assert(false) }}
  }} }}
}}
"#
    ));
}

/// A region left **open** because the frame that would close it was parked, and a second region
/// opened nested inside it while it is in that state.
#[test]
fn a_region_nested_under_a_parked_close_survives_that_close_firing_beneath_it() {
    holds(&format!(
        r#"{AMB}{SAVED}
effect ask {{ read get[env]() -> Int }}

test "a sibling region closed by an enclosing close still reads its own cell" {{
  with_cell[slot](Nothing) {{ s -> {{
    let a = handle {{
      with_cell[r](11) {{ c ->
        handle {{ ask.get[env]() + cell_get(c) }}
        with {{ ask.get[env]() -> if amb.flip[coin]() {{ 10 }} else {{ 20 }} }}
      }}
    }} with {{
      amb.flip[coin]() resume k -> {{ cell_set(s, Just(k)); 0 }},
      return x -> x
    }};
    assert_eq(a, 0);
    let out = with_cell[q](13) {{ d -> {{
      let r1 = match cell_get(s) {{ Just(k) -> k(true), Nothing -> -1 }};
      assert_eq(r1, 21);
      cell_get(d)
    }} }};
    assert_eq(out, 13)
  }} }}
}}
"#
    ));
}

/// A list held by a binding **and** by a cell, grown once per resumption.
#[test]
fn a_list_a_resumption_still_holds_is_not_rewritten_by_the_next_one() {
    holds(&format!(
        r#"{AMB}
test "the alias stays one element long while the cell grows to three" {{
  with_cell[xs]([3]) {{ c -> {{
    let held = cell_get(c);
    let total = handle {{
      let b = amb.flip[coin]();
      cell_set(c, push(cell_get(c), if b {{ 7 }} else {{ 9 }}));
      len(held) * 10 + len(cell_get(c))
    }} with {{ amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }};
    assert_eq(total, 25);
    assert_eq(held, [3]);
    assert_eq(cell_get(c), [3, 7, 9])
  }} }}
}}
"#
    ));
}

/// The capture sits in a region's **initialiser**, so a resumption re-enters the region itself: the
/// open runs once per resumption and each run gets its own cell, while the enclosing region's cell
/// threads across both.
#[test]
fn a_capture_in_an_initialiser_opens_the_region_once_per_resumption() {
    holds(&format!(
        r#"{AMB}
test "each resumption opens the region its capture was in" {{
  with_cell[t](0) {{ t -> {{
    let total = handle {{
      with_cell[inner](if amb.flip[coin]() {{ 4 }} else {{ 8 }}) {{ c -> {{
        cell_set(c, cell_get(c) + 1);
        cell_set(t, cell_get(t) + cell_get(c));
        cell_get(c)
      }} }}
    }} with {{ amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }};
    assert_eq(total, 14);
    assert_eq(cell_get(t), 14)
  }} }}
}}
"#
    ));
}

/// A `return` clause that reads and writes the region's own cell, on a handler resumed twice.
#[test]
fn a_return_clause_writing_the_regions_cell_runs_once_per_resumption() {
    holds(&format!(
        r#"{AMB}
test "the return clause threads with everything else" {{
  with_cell[t](3) {{ c -> {{
    let total = handle {{
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b {{ 1 }} else {{ 2 }}
    }} with {{
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> {{ cell_set(c, cell_get(c) * 2); x + cell_get(c) }}
    }};
    assert_eq(total, 29);
    assert_eq(cell_get(c), 18)
  }} }}
}}
"#
    ));
}

/// Evaluation order, recorded rather than inferred.
#[test]
fn regions_do_not_move_relative_to_the_effects_around_them() {
    holds(
        r#"
effect note { write at[j](what: String) -> Unit }

fn mark(what: String) -> Int / {note.write[j]} = { note.at[j](what); 0 }

fn both(a: Int, b: Int) -> Int = a + b

test "a region's open and close sit where they always did" {
  with_cell[journal]([]) { j ->
    handle {
      let outer = with_cell[a](mark("init-a")) { c -> {
        mark("body-a");
        both(
          with_cell[b](mark("init-b")) { d -> mark("body-b") },
          with_cell[e](mark("init-e")) { f -> mark("body-e") })
      } };
      assert_eq(outer, 0);
      let nested = with_cell[g](with_cell[h](mark("init-h")) { i -> mark("body-h") }) { k ->
        mark("body-g")
      };
      assert_eq(nested, 0);
      assert_eq(cell_get(j), [
        "init-a", "body-a", "init-b", "body-b", "init-e", "body-e",
        "init-h", "body-h", "body-g"])
    } with {
      note.at[j](w) -> cell_set(j, push(cell_get(j), w)),
    }
  }
}
"#,
    );
}

/// Evaluation order **through** a multi-shot resumption.
#[test]
fn a_resumption_replays_the_suffix_in_the_order_it_was_captured() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }
effect note { write at[j](what: String) -> Unit }

test "the replayed suffix keeps its order" {
  with_cell[journal]([]) { j ->
    handle {
      let total = handle {
        note.at[j]("enter");
        let b = amb.flip[coin]();
        with_cell[scratch](0) { s -> {
          note.at[j]("after");
          cell_set(s, 1);
          if b { 10 } else { 20 }
        } }
      } with {
        amb.flip[coin]() resume k -> k(true) + k(false),
        return x -> x
      };
      assert_eq(total, 30);
      assert_eq(cell_get(j), ["enter", "after", "after"])
    } with {
      note.at[j](w) -> cell_set(j, push(cell_get(j), w)),
    }
  }
}
"#,
    );
}

/// The five handler shapes the standard library is written in, stacked, with a region between every
/// pair.
#[test]
fn the_standard_handler_shapes_stack_without_losing_a_write() {
    holds(
        r#"
effect ask   { read  get[env]()            -> Int }
effect tell  { write say[out](what: String) -> Unit }
effect state { read  get[s]()               -> Int
               write put[s](v: Int)         -> Unit }
effect fail  { read  stop[f]()              -> Int }
effect amb   { read  flip[coin]()           -> Bool }

fn work() -> Int / {ask.read[env], tell.write[out], state.read[s], state.write[s], amb.read[coin]} = {
  let base = ask.get[env]();
  tell.say[out]("start");
  state.put[s](base);
  let b = amb.flip[coin]();
  state.put[s](state.get[s]() + 1);
  tell.say[out](if b { "left" } else { "right" });
  state.get[s]()
}

test "reader, writer, state, abort and amb over four nested regions" {
  with_cell[out]([]) { journal ->
    handle {
      with_cell[s](0) { st ->
        handle {
          let total = handle {
            handle { work() } with {
              ask.get[env]() -> 100,
              return x -> x
            }
          } with {
            amb.flip[coin]() resume k -> k(true) + k(false),
            fail.stop[f]() -> 0,
            return x -> x
          };
          assert_eq(total, 101 + 102);
          assert_eq(cell_get(st), 102)
        } with {
          state.get[s]()  resume k -> k(cell_get(st)),
          state.put[s](v) resume k -> { cell_set(st, v); k(()) },
          return x -> x
        }
      };
      assert_eq(cell_get(journal), ["start", "left", "right"])
    } with {
      tell.say[out](w) -> cell_set(journal, push(cell_get(journal), w)),
    }
  }
}
"#,
    );
}

/// An `abort` handler that discards a continuation captured **across** a region whose cell the
/// abandoned computation was about to write.
#[test]
fn an_abort_past_an_open_region_keeps_exactly_the_writes_that_preceded_it() {
    holds(
        r#"
effect fail { read stop[f](why: String) -> Int }

test "a region abandoned mid-flight keeps the writes made before the perform" {
  with_cell[journal]([]) { j -> {
    let out = handle {
      cell_set(j, push(cell_get(j), "before"));
      with_cell[scratch](0) { s -> {
        cell_set(j, push(cell_get(j), "opened"));
        cell_set(s, 1);
        let n = fail.stop[f]("no");
        cell_set(j, push(cell_get(j), "never"));
        n
      } }
    } with {
      fail.stop[f](why) resume k -> { cell_set(j, push(cell_get(j), "clause")); 7 },
      return x -> x
    };
    assert_eq(out, 7);
    assert_eq(cell_get(j), ["before", "opened", "clause"])
  } }
}
"#,
    );
}

/// **A route out of a region that the escape brand says is closed, and is not.**
#[test]
fn a_general_clause_inside_a_region_carries_that_regions_atoms_out_of_it() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }

fn leaks(n: Int) -> Int =
  with_cell[t](n) { c ->
    handle { let b = amb.flip[coin](); cell_set(c, cell_get(c) + 1); cell_get(c) }
    with { amb.flip[coin]() resume k -> k(true), return x -> x }
  }

fn discharges(n: Int) -> Int =
  with_cell[t](n) { c ->
    handle { cell_set(c, cell_get(c) + 1); cell_get(c) }
    with { amb.flip[coin]() -> true, return x -> x }
  }

test "through a general clause" { assert_eq(leaks(1), 2) }
test "through a tail-resumptive one" { assert_eq(discharges(1), 2) }
"#;
    let (program, resolved) = load(src);
    let check = check_program(&program, &resolved).expect("the probe must typecheck");
    let atoms = |name: &str| -> Vec<String> {
        let at = check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"));
        check.tests[at]
            .footprint
            .atoms()
            .map(|a| a.to_string())
            .collect()
    };
    assert_eq!(
        atoms("through a general clause"),
        vec!["cell.read[t]".to_string(), "cell.write[t]".to_string()],
        "if this is ever empty, the escape brand's claim has become true and should be re-read \
         rather than this test deleted"
    );
    assert_eq!(
        atoms("through a tail-resumptive one"),
        Vec::<String>::new(),
        "the shape every handler in `examples/` is written in still discharges"
    );
}

/// A capture inside `simulate`, where the search re-executes prefixes.
#[test]
fn every_seed_of_a_simulated_capture_starts_from_the_same_region() {
    let src = r#"
effect counter {
  read  get[n]() -> Int
  write put[n](v: Int) -> Unit
}

fn bump() -> Unit / {counter.read[n], counter.write[n], clock.read} = {
  let seen = counter.get[n]();
  let _ = clock.now();
  counter.put[n](seen + 1)
}

test "two tasks over one region cell" {
  with_cell[n](0) { c ->
    handle {
      simulate {
        let a = task.spawn(|| bump());
        let b = task.spawn(|| bump());
        task.join(a);
        task.join(b);
        assert(cell_get(c) == 1 || cell_get(c) == 2)
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
    assert!(
        explored.exploration.failure.is_none(),
        "no interleaving may leave the counter outside 1..2, and one did: {:?}",
        explored.exploration.failure
    );
    assert!(
        explored.exploration.exhaustive,
        "the search must have emptied its frontier for the absence of a failure to mean anything"
    );
    assert!(
        explored.exploration.explored > 1,
        "the probe must have had more than one interleaving to re-execute from"
    );
}

/// **The re-execution probe with the widest reach**: one machine runs the same test many times,
/// exactly as a property search, a bisection hybrid and `--jobs 1` replay all do.
#[test]
fn a_hundred_runs_of_one_machine_answer_identically() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }

test "the trace cell ends at two, every time" {
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
"#;
    let (program, resolved) = load(src);
    let check = check_program(&program, &resolved).expect("the probe must typecheck");
    let mut machine = Machine::new(&program, &resolved, &check);
    for run in 0..100 {
        machine
            .eval_test(0)
            .unwrap_or_else(|d| panic!("run {run} must answer as run 0 did: {d:#?}"));
    }
}
