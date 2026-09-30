//! A region belongs to the stack that opened it: however the opens and closes of a detached body,
//! a clause, a task and the stack that spawned it interleave, a close reaches only its own stack's
//! regions, and a body restored from a snapshot runs again in the regions open at its stop. Each
//! program forces its interleaving whatever the schedule.

use crate::fixture::Compiled;
use ply_eval::sim::DEFAULT_STEPS;
use ply_eval::{Seed, Span, Value};

const PROGRAMS: &str = r#"
effect gen {
  write step() -> Unit
  write give(v: Int) -> Unit
}

effect abort {
  write stop(code: Int) -> Int
}

effect amb {
  write flip() -> Bool
}

type Saved = Nothing | Just((Bool) -> Int)

pub fn raised(n: Int) -> Int =
  handle {
    handle {
      with_cell[acc](n) { c -> {
        gen.step();
        abort.stop(cell_get(c))
      } }
    } with {
      gen.step() resume k -> {
        k(());
        0
      },
    }
  } with {
    abort.stop(code) resume k -> code,
  }

pub fn escaped(n: Int) -> Int =
  with_cell[opened](0) { opened -> handle {
    simulate {
      let t = task.spawn(|| with_cell[mine](1) { a -> {
        cell_set(opened, 1);
        task.yield();
        cell_get(a)
      } });
      iterate(0, 1000, |i: Int| if cell_get(opened) == 1 { Stop(i) } else {
        task.yield();
        Continue(i + 1)
      });
      abort.stop(n) + task.join(t)
    }
  } with {
    abort.stop(code) resume k -> code,
  } }

pub fn stopped(n: Int) -> Int =
  with_cell[total](n) { t -> {
    let first = handle {
      with_cell[state](1) { c -> {
        gen.give(cell_get(c));
        gen.give(5);
        cell_get(c)
      } }
    } with {
      gen.give(v) resume k -> if v > 3 { 0 } else { v + k(()) },
    };
    first + cell_get(t)
  } }

pub fn resumed(n: Int) -> Int =
  handle {
    with_cell[inner](n) { b -> {
      gen.step();
      cell_get(b)
    } }
  } with {
    gen.step() resume k -> with_cell[outer](10) { x -> {
      let answered = k(());
      answered + cell_get(x)
    } },
  }

pub fn handshake(n: Int) -> Int =
  with_cell[opened](0) { opened -> with_cell[closed](0) { closed -> simulate {
    let held = with_cell[root](n) { b -> {
      let t = task.spawn(|| with_cell[mine](1) { a -> {
        cell_set(opened, 1);
        iterate(0, 1000, |n: Int| if cell_get(closed) == 1 { Stop(n) } else {
          task.yield();
          Continue(n + 1)
        });
        cell_get(a)
      } });
      iterate(0, 1000, |n: Int| if cell_get(opened) == 1 { Stop(n) } else {
        task.yield();
        Continue(n + 1)
      });
      { own: cell_get(b), task: t }
    } };
    cell_set(closed, 1);
    held.own + task.join(held.task)
  } } }

pub fn unwound(n: Int) -> Int =
  with_cell[go](0) { go -> with_cell[opened](0) { opened -> with_cell[closed](0) { closed -> simulate {
    let t = task.spawn(|| {
      iterate(0, 1000, |n: Int| if cell_get(go) == 1 { Stop(n) } else {
        task.yield();
        Continue(n + 1)
      });
      with_cell[mine](2) { a -> {
        cell_set(opened, 1);
        iterate(0, 1000, |n: Int| if cell_get(closed) == 1 { Stop(n) } else {
          task.yield();
          Continue(n + 1)
        });
        cell_get(a)
      } }
    });
    let got = handle {
      cell_set(go, 1);
      iterate(0, 1000, |n: Int| if cell_get(opened) == 1 { Stop(n) } else {
        task.yield();
        Continue(n + 1)
      });
      with_cell[gone](n) { g -> abort.stop(cell_get(g)) }
    } with {
      abort.stop(code) resume k -> code,
    };
    cell_set(closed, 1);
    got + task.join(t)
  } } } }

pub fn threaded(n: Int) -> Int = {
  let got = handle {
    with_cell[inner](5) { c -> {
      let b = amb.flip();
      cell_set(c, cell_get(c) + (if b { 1 } else { 2 }));
      cell_get(c)
    } }
  } with {
    amb.flip() resume k -> k(true) + k(false),
  };
  n + got
}

pub fn later(n: Int) -> Int = {
  let got = handle {
    with_cell[inner](5) { c -> {
      let b = amb.flip();
      with_cell[step](if b { 1 } else { 2 }) { d -> {
        cell_set(c, cell_get(c) + cell_get(d));
        cell_get(c)
      } }
    } }
  } with {
    amb.flip() resume k -> k(true) + k(false),
  };
  n + got
}

pub fn parked(n: Int) -> Int = with_cell[slot](Nothing) { s -> {
  let first = with_cell[log](41) { c ->
    handle {
      let b = amb.flip();
      if b { cell_get(c) } else { 0 }
    } with {
      amb.flip() resume k -> {
        cell_set(s, Just(k));
        0
      },
    }
  };
  let second = match cell_get(s) {
    Just(k) -> k(true),
    Nothing -> 0,
  };
  n + first + second
} }

pub fn dropped(n: Int) -> Int = with_cell[slot](Nothing) { s -> {
  let first = with_cell[log](n) { c ->
    handle {
      with_cell[mine](1) { m -> {
        let b = amb.flip();
        if b { cell_get(c) + cell_get(m) } else { 0 }
      } }
    } with {
      amb.flip() resume k -> {
        cell_set(s, Just(k));
        cell_get(c)
      },
    }
  };
  first + 1
} }

pub fn nested(n: Int) -> Int = with_cell[slot](Nothing) { s -> {
  let first = with_cell[log](41) { c ->
    handle {
      handle {
        let b = amb.flip();
        if b { cell_get(c) } else { 0 }
      } with {
        amb.flip() resume j -> {
          cell_set(s, Just(j));
          0
        },
      }
    } with {
      gen.step() resume k -> k(()) + 0,
    }
  };
  let second = match cell_get(s) {
    Just(j) -> j(true),
    Nothing -> 0,
  };
  n + first + second
} }

pub fn tasked(n: Int) -> Int = with_cell[slot](Nothing) { s -> {
  let first = with_cell[log](41) { c -> simulate {
    let t = task.spawn(|| handle {
      let b = amb.flip();
      if b { cell_get(c) } else { 0 }
    } with {
      amb.flip() resume j -> {
        cell_set(s, Just(j));
        0
      },
    });
    task.join(t)
  } };
  let second = match cell_get(s) {
    Just(j) -> j(true),
    Nothing -> 0,
  };
  n + first + second
} }
"#;

/// Runs `name(n)` for each seed `n` in `seeds` and checks it answers `n + more`, the tier having
/// found every region and slot given back rather than declining the entry. An `Int` argument is
/// never a memo word, so every call runs.
#[track_caller]
fn answers(name: &str, more: i64, seeds: std::ops::Range<u64>) {
    let compiled = Compiled::new(PROGRAMS);
    let (mut machine, tier) = compiled.machine_and_tier();
    for root in seeds {
        let n = root as i64;
        machine.set_seed(Seed::root(root), DEFAULT_STEPS);
        match machine.call(name, vec![Value::Int(n)], Span::DUMMY) {
            Ok(Value::Int(got)) => assert_eq!(got, n + more, "`{name}` at seed {root}"),
            other => panic!("`{name}` at seed {root}: {other:?}"),
        }
    }
    assert_eq!(tier.declines().total(), 0, "{:?}", tier.declines());
}

/// The body closes the region it opened before it stopped; the clause opened one after, on the
/// stack that resumed the body, and reads it once the body has finished.
#[test]
fn a_clause_keeps_its_cell_when_the_body_it_resumed_closes_an_older_region() {
    answers("m.resumed", 10, 0..4);
}

/// The root opens its region and spawns; the task opens its own and signals; the root closes its
/// region, the older of the two, and signals back; only then does the task read its cell.
#[test]
fn a_task_keeps_its_cell_when_the_root_closes_the_region_it_opened_first() {
    answers("m.handshake", 1, 0..16);
}

/// The task opens its region after the root installs a `handle`, and the root's zero-shot clause
/// abandons a body holding a cell of its own: the unwind reclaims that cell, the task reads its.
#[test]
fn an_unwind_reclaims_the_abandoned_stacks_region_and_no_other() {
    answers("m.unwound", 2, 0..16);
}

/// The root unwinds out of the simulated region while a task it spawned is suspended in a region
/// of its own: the task will not run again, so its region goes back when the simulated one ends.
#[test]
fn a_task_its_region_never_finished_gives_its_region_back_as_the_region_ends() {
    answers("m.escaped", 0, 0..16);
}

/// A clause that never resumes leaves its body suspended in the region it opened: nothing can
/// resume it once the entry has returned, so the region does not count against the entry.
#[test]
fn a_body_its_clause_abandons_holds_no_region_once_the_entry_returns() {
    answers("m.stopped", 1, 0..4);
}

/// The arena's extent as a bare entry of `name(n)` begins, once it returns, and once it has ended:
/// between the last two, what a capture pins is still held.
fn extents(name: &str, n: i64, want: i64) -> [(usize, usize); 3] {
    let compiled = Compiled::new(PROGRAMS);
    ply_codegen::c::producer::ensure_default();
    let front: &'static ply_eval::Front = Box::leak(Box::new(compiled.front.clone()));
    let source: &'static ply_codegen::Source = Box::leak(Box::new(
        ply_codegen::Source::from_front(front).with_texts(compiled.texts.clone()),
    ));
    let names = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, _) = ply_codegen::c::build(source, &refs).expect("the unit builds");
    let entry = native
        .entry(name)
        .unwrap_or_else(|| panic!("`{name}` compiled"));
    let tables = native.tables().clone();
    let mut ctx = native.context();
    ctx.begin(10_000);
    let began = ctx.cell_extent();
    let arg = ctx.heap.to_word(&tables.layouts, &Value::Int(n));
    let answer = unsafe { entry(&mut ctx, [arg].as_ptr()) };
    assert_eq!(ctx.failed, 0, "`{name}`: {:?}", ctx.diagnostic);
    assert_eq!(
        ply_codegen::heap::Heap::to_value(&tables.layouts, answer),
        Value::Int(want),
        "`{name}({n})`"
    );
    let returned = ctx.cell_extent();
    ctx.end();
    [began, returned, ctx.cell_extent()]
}

/// An unwind leaving a detached `handle` abandons the body, which stopped once and so keeps a
/// snapshot: the body's region leaves its stack as the unwind passes, and the cell the snapshot
/// reaches is held until the entry ends.
#[test]
fn an_unwind_leaving_a_detached_handle_takes_the_bodys_region_off_its_stack_as_it_passes() {
    answers("m.raised", 0, 0..4);

    let [began, returned, ended] = extents("m.raised", 7, 7);
    assert_eq!(
        returned,
        (began.0, began.1 + 1),
        "the abandoned body still holds its region, or the snapshot lost its cell"
    );
    assert_eq!(ended, began);
}

/// The body opens its region before it stops and the clause resumes it twice: the second run is
/// restored into the region the first run closed, and reads what the first run wrote.
#[test]
fn a_body_resumed_twice_runs_again_in_the_region_it_opened_before_the_stop() {
    answers("m.threaded", 14, 0..4);

    let [began, returned, ended] = extents("m.threaded", 0, 14);
    assert_eq!(returned, (began.0, began.1 + 1));
    assert_eq!(ended, began);
}

/// A region the body opens after its stop is no snapshot's: each run's close gives its cell back,
/// and only the cell opened before the stop is held once the entry returns.
#[test]
fn a_region_the_body_opens_after_its_stop_goes_back_at_its_own_close() {
    answers("m.later", 14, 0..4);

    let [began, returned, ended] = extents("m.later", 0, 14);
    assert_eq!(
        returned,
        (began.0, began.1 + 1),
        "a region opened after the stop outlived its close"
    );
    assert_eq!(ended, began);
}

/// The clause parks `k` in an older cell and answers without it, and `k` runs once `log`, the
/// region around the `handle`, has closed: the regions around a `handle` keep their cells for as
/// long as its body can run, and give them back when the entry ends.
#[test]
fn a_continuation_resumed_after_the_region_around_its_handle_closed_reads_that_regions_cell() {
    answers("m.parked", 41, 0..4);

    let [began, returned, ended] = extents("m.parked", 0, 41);
    assert_eq!(
        returned,
        (began.0, began.1 + 2),
        "`log`'s cell and `slot`'s, the regions around the `handle`, are held until the entry ends"
    );
    assert_eq!(ended, began);
}

/// The clause parks `k` and nothing resumes it: the body stays suspended inside its own region,
/// and the entry's end drops it with every region it pins.
#[test]
fn a_body_nothing_resumes_gives_back_all_it_holds_when_the_entry_ends() {
    answers("m.dropped", 1, 0..4);

    let [began, returned, ended] = extents("m.dropped", 3, 4);
    assert_eq!(
        returned,
        (began.0 + 1, began.1 + 3),
        "the suspended body holds its own region open, and the closed regions around it their cells"
    );
    assert_eq!(ended, began);
}

/// The inner body parks its `k`; the outer body never stops, so it finishes with no snapshot and
/// lets go of `log`. The inner body still holds `log`, which is around its own `handle` too, so
/// its `k` reads the cell once `log` has closed.
#[test]
fn a_nested_bodys_continuation_reads_the_region_around_the_outer_handle() {
    answers("m.nested", 41, 0..4);

    let [began, returned, ended] = extents("m.nested", 0, 41);
    assert_eq!(
        returned,
        (began.0, began.1 + 2),
        "`log`'s cell and `slot`'s are held by the inner body once the outer one is gone"
    );
    assert_eq!(ended, began);
}

/// A task opens the `handle`, so the regions its body reaches are around the `simulate` the task
/// runs in; its `k` runs once the region has ended and `log` has closed.
#[test]
fn a_tasks_continuation_reads_the_region_around_its_simulate() {
    answers("m.tasked", 41, 0..4);

    let [began, returned, ended] = extents("m.tasked", 0, 41);
    assert_eq!(
        returned,
        (began.0, began.1 + 2),
        "`log`'s cell and `slot`'s, around the `simulate`, are held by the body the task opened"
    );
    assert_eq!(ended, began);
}
