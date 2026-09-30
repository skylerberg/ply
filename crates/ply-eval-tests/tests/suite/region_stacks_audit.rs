//! A region belongs to the stack that opened it: however the opens and closes of a detached body,
//! a clause, a task and the stack that spawned it interleave, a close reaches only its own stack's
//! regions. Each program forces its interleaving whatever the schedule.

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
    assert_eq!(tier.declines().touched_cells, 0, "{:?}", tier.declines());
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

/// An unwind leaving a detached `handle` abandons the body, which stopped once and so keeps a
/// snapshot: the body's region goes back as the unwind passes, not only when the entry ends.
#[test]
fn an_unwind_leaving_a_detached_handle_frees_the_bodys_region_as_it_passes() {
    answers("m.raised", 0, 0..4);

    let compiled = Compiled::new(PROGRAMS);
    ply_codegen::c::producer::ensure_default();
    let front: &'static ply_eval::Front = Box::leak(Box::new(compiled.front.clone()));
    let source: &'static ply_codegen::Source = Box::leak(Box::new(
        ply_codegen::Source::from_front(front).with_texts(compiled.texts.clone()),
    ));
    let names = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, _) = ply_codegen::c::build(source, &refs).expect("the unit builds");
    let entry = native.entry("m.raised").expect("`m.raised` compiled");
    let tables = native.tables().clone();
    let mut ctx = native.context();
    ctx.begin(10_000);
    let before = ctx.cell_extent();
    let arg = ctx.heap.to_word(&tables.layouts, &Value::Int(7));
    let answer = unsafe { entry(&mut ctx, [arg].as_ptr()) };
    assert_eq!(ctx.failed, 0, "{:?}", ctx.diagnostic);
    assert_eq!(
        ply_codegen::heap::Heap::to_value(&tables.layouts, answer),
        Value::Int(7)
    );
    assert_eq!(
        ctx.cell_extent(),
        before,
        "the abandoned body still holds its region while the entry runs on"
    );
    ctx.end();
}
