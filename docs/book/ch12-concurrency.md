# 12. Tasks, channels and simulation

Ply's concurrency is an effect like any other, and its handler is a scheduler
that lives in the program. `simulate { ... }` runs a body against a seeded
scheduler: tasks you spawn, virtual time, and a search over the interleavings
that could change the answer. This chapter covers the effect, the scheduling
rules, and the one place Ply runs two computations at once for speed rather than
to explore a schedule.

## `simulate`

```ply
simulate {
  let a = task.spawn(|| worker(1));
  let b = task.spawn(|| worker(2));
  assert_eq(task.join(a), 2);
  assert_eq(task.join(b), 4)
}
```

The `task`, `clock` and `random` effects are handled by the region:

| operation | answers |
| --- | --- |
| `task.spawn(body)` | a `Task<a>`; `body` runs on the region's scheduler |
| `task.join(t)` | what the task answered; raises if it was cancelled |
| `task.await(t)` | `Some` of the answer, or `None` once cancelled |
| `task.yield()` | lets another task run |
| `task.cancel(t)` | `Bool`; `false` if the task had ended or was already cancelled |
| `task.channel(n)` | a `Chan<a>` holding up to `n` values; `0` is a rendezvous |
| `task.send(c, x)` / `task.recv(c)` | queue or take a value, waiting when full or empty |
| `task.close(c)` | end sending: a waiting receiver hears `None` |
| `task.select(arms, wait)` | go on one of several channel operations |

A task performs against the handlers around its `task.spawn`, and the region's
row gains `sim.read`, which a deterministic test may carry.

## Channels

A channel carries values between tasks. A send waits while the channel is full
and answers `true` once its value is queued or handed over; a receive waits while
it is empty and answers the oldest value sent. Closing ends sending: a waiting
receiver hears `None` and a waiting sender `false`.

```ply
test "a channel carries values between tasks" {
  simulate {
    let c = task.channel(1);
    let producer = task.spawn(|| task.send(c, 41));
    let got = task.recv(c);
    assert(task.join(producer));
    assert_eq(got, Some(41))
  }
}
```

`task.select(arms, wait)` goes on one of several channel operations. An arm is a
channel beside `None` to receive from it or `Some(x)` to send to it. The first
arm, in the order given, that can go without waiting goes, and the answer is
`Some((index, got))`. The order of the arms is the only priority, so a caller that
wants a later arm to have its turn rotates them — which is what `std.chan` does.
A select is ordered against every operation on each of its arms' channels, so the
search tries each order two senders or two receivers could take.

A race is one channel: each worker sends what it answers, the first receive wins,
and cancelling the others stops them.

## Cancellation

`task.cancel(t)` stops `t` where it stands: a sleep, a join, a channel or a host
operation it waits on is let go, an operation that was answered and that it has
not yet run on loses its answer, and it performs nothing more but the `release`
of each bracket it stands in. When it next runs it only unwinds.

```ply
test "a cancelled task performs nothing after the cancel" {
  simulate {
    let t = task.spawn(|| { clock.sleep(Duration(1000)); 1 });
    assert(task.cancel(t));
    assert_eq(task.await(t), None)
  }
}
```

`task.cancel` answers `false` for a task that had already ended, whose answer it
leaves alone, and for one a cancel has already reached. `task.await(t)` is a join
that answers `Some` of what `t` answered or `None` once it was cancelled;
`task.join` of a cancelled task has nothing to answer and raises. A task cannot
cancel itself or the region's body. A deadline is the two together: one task
sleeps and cancels the other, which a third awaits.

A cancel is read against the step its task took last and each step after it, and
a task cancelled before it ran takes a step that runs nothing, so the search
tries the cancel a step earlier and a step later, and from each of those runs the
next, until it has landed everywhere it could.

## Virtual time and randomness

Virtual time advances only when no task is enabled, so `clock.sleep` costs no wall
clock, and `random` draws from the region's seed. Tasks interleave only at `task`,
`clock` and `random` operations: any two allocations, and two accesses to one
cell with a write, are ordered. A read-then-write with no scheduler operation
between runs as one step, so put a `task.yield()` there when a race should be
possible.

## The search

A `simulate` test does not run one schedule; it searches the schedules that could
change the answer. The default is footprint-guided partial-order reduction
(`dpor`), and when its frontier is exhausted the result holds for **every**
interleaving — the run says so:

```text
   ok      a cancelled task performs nothing after the cancel      0.6ms
       2 interleavings · exhaustive
```

| flag | meaning |
| --- | --- |
| `--sim dpor` | footprint-guided partial-order reduction (default) |
| `--sim random` | one interleaving per seed |
| `--sim once` | exactly one interleaving |
| `--seeds N` | seeds per test, from 0 (default 1 under `dpor`, 64 under `random`) |
| `--sim-roots FROM..TO` | the seeds from `FROM` up to but not including `TO` |
| `--sim-budget N` | interleavings per seed (`dpor` only) |
| `--sim-steps N` | steps per interleaving before `E0414` |
| `--seed 7:3.0.2` | replay one interleaving; implies `--sim once` |
| `--measure-reduction` | also run the search unpruned and blind, and report each count |

A failure prints the racing steps and a replay command such as
`ply test --seed 0:0.1.0.2 --filter 'no account is ever overdrawn'`. Replay is
meant to be exact; `E0415` says it was not, and that is Ply's fault.

Results are cached per search plan, and a search that spends its budget passes
but is not cached.

## `parallel`

`parallel { a, b, .. }` answers the tuple `(a, b, ..)` and means exactly that: the
branches evaluated left to right. The runtime runs them at once, on threads of
its own, wherever that cannot change the answer:

```ply
fn both(xs: List<Int>) -> (Int, Int) =
  parallel { fold(xs, 0, |a: Int, x: Int| a + x), len(filter(xs, |x: Int| x > 0)) }
```

The checker admits a block only when no two branches' rows **conflict**, where two
atoms conflict when they name the same resource of the same effect and one is a
`write`:

```text
Error[E0309]: two branches of this `parallel` block may touch one resource
  --> p.ply:2:73
   | fn bad() -> (Unit, Unit) / {log.note[a]} = parallel { log.note[a]("x"), log.note[a]("y") }
   |                                                                         ^^^^^^^^^^^^^^^^ this branch performs `p.log.note[a]`
  --> p.ply:2:55
   | fn bad() -> (Unit, Unit) / {log.note[a]} = parallel { log.note[a]("x"), log.note[a]("y") }
   |                                                       ^^^^^^^^^^^^^^^^ and this one performs `p.log.note[a]`
   = branches run at once, so two of them may not perform operations of one effect on one resource where either is a `write`
   = bind the branches with `let` in the order they must run, or move what they share out of the block
   compilation failed (1 error)
```

- The block's row is the union of its branches', and it is as deterministic as
  they are, so a deterministic test may hold one and its result may be cached.
- The leftmost failing branch is the block's failure, whichever failed first. A
  branch to its right may already have performed its effects.
- The branches spend one step budget between them.
- A block inside a `simulate` region runs in turn, so the scheduler sees nothing
  of it. One whose branches hold a cell, a task or a continuation also runs in
  turn.

`std.parallel` splits a list across nested blocks.

## What cannot leave a region

A task or a channel is branded by its region, exactly as a cell is (chapter 11):

```text
Error[E0413]: a task escapes the `simulate` region that spawned it
  --> e.ply:1:39
   | fn escape() -> Task<Int> = simulate { task.spawn(|| 1) }
   |                                       ^^^^^^^^^^^^^^^^ this has type `Task<Int>`
   = a `Task` is a key into the region's scheduler, and the scheduler ends with the region
   = `join` the task inside the region and return its value instead
```

`E0413` covers a task or channel that escapes in the region's answer, directly or
inside a value or closure, and one that enters from outside. A closure's type
shows a captured task only as the `task.join`, `task.await` or `task.cancel` in
its row, a captured channel as its `task.send`, `task.recv`, `task.close` or
`task.select`, and either as the `sim.read` of a `simulate` it opens to use one.
Nested regions are `E0416`:

```text
Error[E0416]: a `simulate` region inside another `simulate` region
  --> n.ply:1:17
   | test "nested" { simulate { simulate { assert(true) } } }
   |                 ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this region's body reaches `sim.read`
  --> n.ply:1:28
   | test "nested" { simulate { simulate { assert(true) } } }
   |                            ^^^^^^^^^^^^^^^^^^^^^^^^^ the region it reaches is here
   = two schedulers means two notions of `runnable`, and a task in the inner region blocking the outer one
   = hoist the inner region out, or drop it and let the outer one schedule
   compilation failed (1 error)
```

A host operation inside a region is `E0425`, and `E0414` is deadlock or a spent
step budget. Outside a region, `--host` answers `task` and `clock` from the real
world instead: tasks run in turn on the thread that spawned them, and one that
waits is parked alone while the others run (chapter 19).

> **Try it.** Write a race: two tasks each increment a cell without a
> `task.yield()` between the read and the write, and an assertion that the final
> count is two. The search will pass it exhaustively and explain why — the pair of
> accesses is one step. Insert a `task.yield()` and watch the same assertion fail
> with a racing-steps report and a seed to replay.

## Summary

- `simulate { ... }` handles `task`, `clock` and `random` with a seeded scheduler;
  a task performs against the handlers around its `spawn`.
- Channels queue values; `task.close` ends sending; `task.select` chooses among
  arms by order.
- `task.cancel` stops a task where it stands; `task.await` answers `None` for a
  cancelled task.
- Tasks interleave only at `task`, `clock` and `random` operations; virtual time
  costs no wall clock.
- The search is exhaustive by default and reports when it is; `--seed` replays.
- `parallel { a, b }` runs branches at once where their rows do not conflict
  (`E0309`), and is otherwise exactly the tuple of its branches.
- A task or channel cannot leave its region (`E0413`); regions do not nest
  (`E0416`).

Next: making a test suite out of all this, and the caching that makes it fast.
