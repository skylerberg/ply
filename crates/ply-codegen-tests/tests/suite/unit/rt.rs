use ply_codegen::c::Native;
use ply_codegen::heap::{Heap, imm};
use ply_eval::Value;

/// Each `pub fn` runs `n` rounds of one way a body is left without returning, then either answers
/// their sum or, asked to spin, nests until the calls run out, which counts the calls it had left.
const LEFT: &str = r#"
effect parse {
  raise refused(at: Int)
}

effect stop {
  write now() -> Unit
}

effect ask {
  read q() -> Int
}

effect amb {
  read flip() -> Bool
}

type Saved = Nothing | Just((Int) -> Int)

fn nest(i: Int) -> Int / {diverges} = 1 + nest(i + 1)

fn climbs(depth: Int) -> Int = if depth <= 0 { 0 } else { 1 + climbs(depth - 1) }

fn refuse(n: Int, depth: Int) -> Int / {parse.refused} =
  if depth <= 0 { parse.refused(n) } else { 1 + refuse(n, depth - 1) }

fn panics(n: Int, depth: Int) -> Int / {abort.raise} =
  if depth <= 0 { panic("deep") } else { 1 + panics(n, depth - 1) }

fn stops(n: Int, depth: Int) -> Int / {stop.write} =
  if depth <= 0 {
    stop.now();
    n
  } else { 1 + stops(n, depth - 1) }

fn asks(depth: Int) -> Int / {ask.read} = if depth <= 0 { ask.q() } else { 1 + asks(depth - 1) }

fn or_at(r: Result<Int, Int>) -> Int = match r { Ok(v) -> v, Err(at) -> at }

fn ended(sum: Int, spin: Int) -> Int / {diverges} = if spin == 1 { nest(0) } else { sum }

fn refusals(n: Int) -> Int =
  fold(range(0, n), 0, |acc: Int, i: Int| acc + or_at(try { refuse(i, 20) }))

pub fn tried(n: Int, spin: Int) -> Int / {diverges} = ended(refusals(n), spin)

pub fn named(n: Int, spin: Int) -> Int / {abort.raise, diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + or_at(try[parse.refused] { if i < 0 { panics(i, 1) } else { refuse(i, 20) } })),
    spin,
  )

pub fn answered(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + (handle { panics(i, 120) } with { abort.raise(m) -> climbs(120) })),
    spin,
  )

pub fn abandoned(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + (handle { stops(i, 20) } with { stop.now() resume k -> i })),
    spin,
  )

pub fn bracketed(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc
        + or_at(try {
          bracket(|| i, |held: Int| {
            climbs(120);
            ()
          }, |held: Int| refuse(held, 120))
        })),
    spin,
  )

pub fn reentered(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc
        + (handle { asks(20) } with {
          ask.q() resume k -> k(or_at(try { refuse(i, 20) })),
        })),
    spin,
  )

pub fn detached(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc
        + (handle { asks(20) } with {
          ask.q() resume k -> if i % 2 == 0 { 0 } else { 1 + k(i) },
        })),
    spin,
  )

pub fn resumed(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc
        + (handle { asks(20) + refuse(i, 120) } with {
          ask.q() resume k -> k(i),
          parse.refused(at) -> climbs(120),
        })),
    spin,
  )

fn retried(i: Int, depth: Int) -> Int / {ask.read} =
  if depth <= 0 {
    fold(range(0, 3), 0, |acc: Int, round: Int|
      acc + or_at(try { ask.q() + refuse(i, 120) }) + climbs(120))
  } else { 1 + retried(i, depth - 1) }

pub fn again(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + (handle { retried(i, 20) } with { ask.q() resume k -> k(i) })),
    spin,
  )

pub fn again_apart(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + (handle { retried(i, 20) } with { ask.q() resume k -> 1 + k(i) })),
    spin,
  )

pub fn apart(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc
        + (handle { asks(1) + refuse(i, 120) } with {
          ask.q() resume k -> 1 + k(i),
          parse.refused(at) -> climbs(120),
        })),
    spin,
  )

fn flips(n: Int, depth: Int) -> Int / {amb.read} =
  if depth <= 0 {
    or_at(try {
      let heads = amb.flip();
      if heads { refuse(n, 120) } else { refuse(n + 1, 110) }
    })
      + climbs(120)
  } else { 1 + flips(n, depth - 1) }

pub fn twice(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + (handle { flips(i, 20) } with { amb.flip() resume k -> k(true) + k(false) })),
    spin,
  )

fn guarded(depth: Int) -> Int / {ask.read} =
  if depth <= 0 {
    or_at(try {
      let v = ask.q();
      refuse(v, 120)
    })
      + climbs(120)
  } else { 1 + guarded(depth - 1) }

fn later(s: Saved, v: Int, depth: Int) -> Int =
  if depth <= 0 { match s { Just(k) -> k(v), Nothing -> 0 } } else { 1 + later(s, v, depth - 1) }

pub fn elsewhere(n: Int, spin: Int) -> Int / {diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc
        + with_cell[slot](Nothing) { s -> {
          let first = handle { guarded(20) } with {
            ask.q() resume k -> {
              cell_set(s, Just(k));
              0
            },
          };
          first + later(cell_get(s), i, 30)
        } }),
    spin,
  )

pub fn branched(n: Int, spin: Int) -> Int / {diverges} = {
  let (left, right) = parallel { refusals(n), refusals(n) };
  ended(left + right, spin)
}

fn yields(n: Int, depth: Int) -> Int / {parse.refused, task.write} =
  if depth <= 0 {
    task.yield();
    parse.refused(n)
  } else { 1 + yields(n, depth - 1) }

fn worker(n: Int, spin: Int) -> Int / {task.write, diverges} =
  ended(fold(range(0, n), 0, |acc: Int, i: Int| acc + or_at(try { yields(i, 20) })), spin)

pub fn interleaved(n: Int, spin: Int) -> Int / {abort.raise, sim.read, diverges} =
  simulate {
    let beside = task.spawn(|| worker(n, 0));
    let mine = worker(n, 0);
    let theirs = task.join(beside);
    let last = task.spawn(|| worker(n, spin));
    mine + theirs + task.join(last)
  }

pub fn served(n: Int, spin: Int) -> Int / {task.write, abort.raise, diverges} = {
  let beside = task.spawn(|| worker(n, 0));
  let mine = worker(n, 0);
  ended(mine + task.join(beside), spin)
}

fn sleeps(depth: Int) -> Int / {clock.write} =
  if depth <= 0 {
    clock.sleep(Duration(10));
    0
  } else { 1 + sleeps(depth - 1) }

pub fn cancelled(n: Int, spin: Int) -> Int / {sim.read, diverges} =
  simulate {
    let sum = fold(range(0, n), 0, |acc: Int, i: Int| {
      let t = task.spawn(||
        bracket(|| i, |held: Int| {
          climbs(120);
          ()
        }, |held: Int| sleeps(120)));
      task.yield();
      task.cancel(t);
      acc + (match task.await(t) { Some(v) -> v, None -> 1 })
    });
    ended(sum, spin)
  }

fn region(i: Int, depth: Int) -> Int / {sim.read, abort.raise} =
  if depth <= 0 {
    simulate {
      let t = task.spawn(|| panics(i, 20));
      task.join(t)
    }
  } else { 1 + region(i, depth - 1) }

pub fn outside(n: Int, spin: Int) -> Int / {sim.read, diverges} =
  ended(
    fold(range(0, n), 0, |acc: Int, i: Int|
      acc + (handle { region(i, 20) } with { abort.raise(m) -> 1 })),
    spin,
  )

fn joins(t: Task<Int>, depth: Int) -> Int / {task.write, abort.raise} =
  if depth <= 0 { task.join(t) } else { 1 + joins(t, depth - 1) }

pub fn joined(n: Int, spin: Int) -> Int / {sim.read, diverges} =
  simulate {
    let sum = fold(range(0, n), 0, |acc: Int, i: Int| {
      let t = task.spawn(|| sleeps(5));
      task.yield();
      task.cancel(t);
      acc + (handle { joins(t, 20) } with { abort.raise(m) -> 1 })
    });
    ended(sum, spin)
  }
"#;

/// The nested calls an entry below is allowed. The rounds leave through far more frames than
/// this between them, and a clause, a release or a resumed body that ran as deep as the frames a
/// failure left would not fit in it either.
const CALLS: i64 = 200;

const ROUNDS: i64 = 60;

/// How an entry ended: its answer, or why it has none; the calls it had left; the calls it made.
struct Ended {
    answer: Result<Value, String>,
    left: i64,
    made: i64,
}

/// `name(rounds, spin)` under [`CALLS`]; `bound` is the host a production region's tasks need.
fn entered(native: &Native, name: &str, rounds: i64, spin: bool, bound: Option<&Bound>) -> Ended {
    let entry = native
        .entry(name)
        .unwrap_or_else(|| panic!("`{name}` compiled"));
    let mut ctx = native.context();
    if let Some(bound) = bound {
        ctx.set_host(std::sync::Arc::clone(bound), None, None);
    }
    ctx.begin(CALLS);
    let args = [imm(rounds), imm(i64::from(spin))];
    let mut out = unsafe { entry(&mut ctx, args.as_ptr()) };
    let left = ctx.fuel;
    if ctx.sims.last().is_some_and(|sim| sim.is_production()) {
        out = unsafe { ply_codegen::simulate::finish_root(&mut ctx, out) };
    }
    let answer = if ctx.failed == 0 {
        Ok(Heap::to_value(&native.tables().layouts, out))
    } else {
        Err(ctx
            .take_failure()
            .map_or_else(|| format!("failed as {}", ctx.failed), |d| d.message))
    };
    let made = ctx.ticks;
    ctx.end();
    Ended { answer, left, made }
}

type Bound = std::sync::Arc<ply_eval::HostBinding>;

/// The calls `name` has left once its rounds are over, counted by nesting until none are.
fn room(native: &Native, name: &str, rounds: i64, bound: Option<&Bound>) -> Result<i64, String> {
    let spun = entered(native, name, rounds, true, bound);
    match spun.answer {
        Err(why) if why.contains("nested calls") => {
            Ok(spun.made - entered(native, name, rounds, false, bound).made)
        }
        other => Err(format!("spinning ended as {other:?}")),
    }
}

/// What is wrong with how `name` accounts for its calls, if anything: it answers `sum` of its
/// rounds, returns with every call it was given, and has as many left once its rounds are over as
/// it had after none.
fn miscounted(native: &Native, name: &str, sum: i64, bound: Option<&Bound>) -> Option<String> {
    let ended = entered(native, name, ROUNDS, false, bound);
    if ended.answer != Ok(Value::Int(sum)) {
        return Some(format!("`{name}` answered {:?}, not {sum}", ended.answer));
    }
    if ended.left != CALLS {
        return Some(format!(
            "`{name}` returned with {} of its {CALLS} calls",
            ended.left
        ));
    }
    let (fresh, used) = match (
        room(native, name, 0, bound),
        room(native, name, ROUNDS, bound),
    ) {
        (Ok(fresh), Ok(used)) => (fresh, used),
        (fresh, used) => return Some(format!("`{name}`: {fresh:?} and {used:?}")),
    };
    if fresh <= 0 || fresh >= CALLS {
        return Some(format!("`{name}` counted {fresh} calls left of {CALLS}"));
    }
    (used != fresh).then(|| {
        format!("`{name}` had {used} calls left after {ROUNDS} rounds and {fresh} after none")
    })
}

fn triangle(n: i64) -> i64 {
    n * (n - 1) / 2
}

/// An entry of the fixture, and what that many of its rounds sum to.
type Way = (&'static str, fn(i64) -> i64);

/// Each way a body is left without returning.
const WAYS: &[Way] = &[
    ("m.tried", triangle),
    ("m.named", triangle),
    ("m.answered", |n| 120 * n),
    ("m.abandoned", triangle),
    ("m.bracketed", triangle),
    ("m.reentered", |n| 20 * n + triangle(n)),
    ("m.detached", |n| {
        (0..n).filter(|i| i % 2 == 1).map(|i| 21 + i).sum()
    }),
    ("m.resumed", |n| 120 * n),
    ("m.again", |n| 380 * n + 3 * triangle(n)),
    ("m.again_apart", |n| 383 * n + 3 * triangle(n)),
    ("m.apart", |n| 121 * n),
    ("m.twice", |n| 281 * n + 2 * triangle(n)),
    ("m.elsewhere", |n| 170 * n + triangle(n)),
    ("m.branched", |n| 2 * triangle(n)),
    ("m.interleaved", |n| 3 * triangle(n)),
    ("m.cancelled", |n| n),
    ("m.outside", |n| n),
    ("m.joined", |n| n),
];

/// A frame a failure returns through gives its call back where the failure is caught, and a body
/// left suspended on a stack of its own takes nothing with it. That an entry has as many calls
/// left after its rounds as after none is what a restore handing back too much would break.
#[test]
fn a_body_left_without_returning_keeps_none_of_the_calls_it_nested() {
    let Some((source, native)) = crate::fixture::unit(LEFT) else {
        return;
    };
    let mut wrong: Vec<String> = WAYS
        .iter()
        .filter_map(|(name, sum)| miscounted(&native, name, sum(ROUNDS), None))
        .collect();
    // Tasks the host schedules, whose root is the entry's own stack.
    let bound: Bound = std::sync::Arc::new(super::simulate::tasks_bound(source.front));
    wrong.extend(miscounted(
        &native,
        "m.served",
        2 * triangle(ROUNDS),
        Some(&bound),
    ));
    assert!(wrong.is_empty(), "{wrong:#?}");
}
