//! Adversarial probes for the one property ADR 0017 may not break.
//!
//! `region_meaning_audit.rs` pins the readings the two ADRs disagree about.
//! This file attacks the *wiring*: a region now opens an arena scope, closes it
//! at its lexical end, and hands its slots back unless a pin says a continuation
//! can still reach them. Every program here is one where that machinery could
//! plausibly answer differently from the persistent world it replaced, with the
//! number the forkable world produced written down.
//!
//! The three shapes being hunted, none of which `--engine both` can see:
//!
//! 1. **A slot handed back too early.** The world was monotone, so a cell read
//!    through a continuation resumed after its region returned always
//!    succeeded. An arena that truncates at the close makes that read a stale
//!    slot, and a later region reusing the index makes it a wrong answer rather
//!    than an error.
//! 2. **A resumption that does not observe the previous one.** Threaded state,
//!    beyond the two resumptions the worked example uses: four, eight, and
//!    resumed in an order the capture did not choose.
//! 3. **Evaluation order.** A region open and a region close are now events on
//!    the evaluation path. If either moved relative to the effects around it,
//!    the final value can still match while the trace does not.

use ply_core::check_program;
use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Interp, Machine, Plan, Seed, explore};
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

/// Runs every test on both engines and requires all of them to pass.
///
/// Agreement alone proves nothing — both engines hold the same arena, so a
/// change to the memory model moves them together — so passing is asserted
/// separately, against the integers written into each probe.
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

const AMB: &str = r#"
effect amb { read flip[coin]() -> Bool }
"#;

const SAVED: &str = r#"
type Saved = Nothing | Just((Bool) -> Int)
"#;

// ----------------------------------------------- 1. more than two resumptions

/// Four leaves, from two captures nested inside one another.
///
/// Three resumptions is already pinned; two nested flips is the first shape
/// where a resumption of the *outer* capture re-enters the *inner* one, so the
/// second capture happens twice and both of its resumptions run against a cell
/// the first pair already moved. Threaded the branches read 1, 2, 3, 4 and the
/// total is 10; snapshot-at-capture gives every leaf 1 and a total of 4, and a
/// per-capture arena that were reset on re-entry would give 1, 2, 1, 2 and 6.
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

/// Eight leaves, so that a defect symmetric in two or four branches has nowhere
/// left to hide.
///
/// Threaded, the leaves read 1..8 and sum to 36 with the cell at 8. Snapshot
/// gives 8 and 1. A pattern where only the *first* resumption of each capture
/// sees its predecessor gives neither.
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
///
/// `k(true) + k(false) + k(true) + k(false)` is one capture spliced four times
/// onto four different stacks. Each resumption starts from the cell the
/// previous one left, so the leaves read 1, 2, 3, 4 and the cell ends at 4.
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

// ------------------------------------- 2. resumed in an order nobody captured

/// Two continuations parked in cells and resumed in the **reverse** of the
/// order they were captured in.
///
/// Nothing about the arena knows which capture came first, so the threaded
/// answer depends only on the order the resumptions run: the second capture's
/// body moves the cell to 10 and the first capture's body then moves it to 11.
/// A model that restored anything at a resumption would run both bodies from 0
/// and answer `10` and `1`.
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

/// The same continuation applied twice from outside the handler that made it,
/// long after that handler returned.
///
/// A parked continuation is an ordinary value, so nothing stops a program
/// applying it more than once. Each application splices the same segments onto
/// the live stack and runs against the live arena, so the two applications read
/// 1 and 2 rather than 1 and 1.
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

// -------------------------------- 3. slots a live continuation can still read

/// **The reclamation probe.** A region closes, several more regions open and
/// close over the same bump-pointer positions, and only then is a continuation
/// captured inside the first one resumed to read its cell.
///
/// Under the persistent world this was free: entries were never removed, so the
/// read found `7` however many cells were allocated after it. Under the arena
/// it is only true if the pin the capture took kept the first region's slots
/// out of the bump pointer's reach. A close that ignored the pin answers either
/// a stale-slot diagnostic or — worse and silently — the `99` the region that
/// reused the index wrote.
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

/// A cell of an **enclosing** region, written from a resumption that runs after
/// the *inner* region's lexical close.
///
/// Two slots are in play and they are reclaimed on different schedules: the
/// inner region's is retained by the pin the capture took, the enclosing one is
/// simply still open. The resumption reads the first and writes the second, so
/// a close that freed either at the wrong moment shows up as a wrong integer
/// rather than as an error.
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

/// A region entered once per iteration of a recursion, with a continuation
/// captured in the **first** iteration resumed after the last one has closed.
///
/// `with_cell` in a loop is the shape ADR 0005 §2 called out as the persistent
/// world's cost — one retained entry per iteration — and it is the shape the
/// arena is supposed to make free by truncating at each close. The truncation
/// is only sound if the one iteration a continuation was taken in is exempt
/// from it.
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

/// A capture that crosses a region's **close**, resumed twice — so the close
/// runs twice against one open.
///
/// The inner handler is tail-resumptive, so its `Resume` frame sits below the
/// outer handler's prompt and above `CloseRegion`; the outer capture therefore
/// takes both. Resuming once closes the region, and resuming again re-runs a
/// close on a region that is no longer open while reading a cell inside it.
/// Both resumptions read `8`, so the answers are `108` and `208` — the numbers
/// the persistent world gave, where there was no close to run at all.
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

/// A region left **open** because the frame that would close it was parked, and
/// a second region opened nested inside it while it is in that state.
///
/// Resuming from inside the second region runs the first region's close, which
/// closes the second one with it — a region a live binding is still standing in.
/// The slot that binding names has to survive that, and the value it answers
/// afterwards is `13`, exactly as it was before the close existed.
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
///
/// ADR 0017 §4 mutates a uniquely-owned value in place, and a region model that
/// changed who owns a cell's payload would show up here rather than in a
/// counter: `held` must still be one element long after both resumptions have
/// pushed onto the cell, and the cell must reach three elements summing to 19
/// because the second resumption started from what the first one left.
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

/// The capture sits in a region's **initialiser**, so a resumption re-enters the
/// region itself: the open runs once per resumption and each run gets its own
/// cell, while the enclosing region's cell threads across both.
///
/// The inner cells read 5 and 9 — their own initialisers plus one — and the
/// outer accumulates 14. A per-resumption arena reset would leave the outer at
/// 9; a shared inner cell would leave the branches reading 5 and 6.
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

/// A `return` clause that reads and writes the region's own cell, on a handler
/// resumed twice.
///
/// The return clause runs once per resumption and its write is part of the one
/// threaded state: the branches answer 9 and 20 and the cell ends at 18. Under
/// snapshot-at-capture the second branch would start from the first branch's
/// starting value and answer 9 twice.
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

// ------------------------------------------------------- 4. evaluation order

/// Evaluation order, recorded rather than inferred.
///
/// A region open and a region close are now steps the evaluator takes, and a
/// step that moved relative to the `perform`s around it is a change of meaning
/// even where every value still matches. The journal pins the order of a
/// region's cell initialiser against its body, of two sibling regions in the
/// arguments of one call, and of a region nested in another region's
/// initialiser — the one place a region opens while another is mid-way through
/// deciding what to allocate.
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
///
/// The journal is the observable rather than the sum: two resumptions replay
/// the same suffix, so the record is `enter` once and `after` twice, in that
/// order. A model that snapshotted would still produce two `after`s, so this
/// discriminates a re-ordering rather than a re-reading — and re-ordering is
/// what a `CloseRegion` frame spliced into the wrong place would cause.
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

// --------------------------------------------- 5. the standard handler shapes

/// The five handler shapes the standard library is written in, stacked, with a
/// region between every pair.
///
/// `reader` and `writer` are tail-resumptive, `state` writes before resuming,
/// `abort` discards its continuation, and `amb` resumes twice. Each layer's
/// backing cell sits in its own region, so the arena holds four live scopes at
/// the innermost `perform` and unwinds them in order. The discriminator is the
/// writer's journal, which only accumulates because every one of those clauses'
/// writes survived the resumption that followed it.
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

/// An `abort` handler that discards a continuation captured **across** a region
/// whose cell the abandoned computation was about to write.
///
/// The region has no lexical close to run — the clause returns straight past
/// it — so the arena's scope is unwound by the machine rather than by a
/// `CloseRegion` frame. What must not change is the value: the write before the
/// `perform` survives and the one after it never happened.
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

// -------------------------------------------- 6. what still leaves a region

/// **A route out of a region that ADR 0017 §2 says is closed, and is not.**
///
/// §2 concludes that "with every other route closed, a written row is the only
/// way a `cell` atom reaches a published footprint", and
/// `region_isolation_audit.rs::a_declared_cell_atom_is_what_reaches_a_tests_footprint`
/// asserts the discharged half of that. A *general* clause written inside the
/// region breaks it: `k`'s row carries the region's `cell` atoms, the clause
/// body's row absorbs them when it applies `k`, and they resolve after the
/// region has already subtracted its own brand — so the enclosing definition
/// carries `cell.read[t]` and `cell.write[t]` out of a region whose whole
/// purpose is to discharge them.
///
/// The direction is safe — a test that contends where it need not is slow, not
/// wrong — but it is the number ADR 0017 §6 says must be measured before this
/// lands, and it is not zero for a corpus written in this shape. The
/// tail-resumptive form of the same handler discharges correctly, which is why
/// `examples/` never sees it.
///
/// This is **not** a change R2 made: the pre-region binary reports the same two
/// atoms on the same program. It is pinned here so the ADR's claim is checkable
/// rather than believed.
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
        "if this is ever empty, ADR 0017 §2's claim has become true and should be re-read \
         rather than this test deleted"
    );
    assert_eq!(
        atoms("through a tail-resumptive one"),
        Vec::<String>::new(),
        "the shape every handler in `examples/` is written in still discharges"
    );
}

// ------------------------------- 7. re-execution: search, properties, replay

/// A capture inside `simulate`, where the search re-executes prefixes.
///
/// Every seed is a fresh run from the fixture, so a counter cell driven by two
/// tasks must read exactly `2` under every interleaving the search enumerates.
/// A region whose slots survived from one exploration into the next would show
/// up as a seed that answers 3 or 4, and the search reports it rather than
/// hiding it — which is the point of asserting `exhaustive` and no failure
/// rather than asserting a single run.
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

/// **The re-execution probe with the widest reach**: one machine runs the same
/// test many times, exactly as a property search, a bisection hybrid and
/// `--jobs 1` replay all do.
///
/// Every run must answer identically. A region whose slots leaked from one run
/// into the next is a value that grows, and it is the shape a seeded replay
/// would fail to reproduce.
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
