# 6. Deterministic simulation

Status: accepted — **implemented**
Date: 2026-08-13

> **Corrected by the W6 documentation audit.** This line read "the seed, the
> plan and the dependence relation landed; everything that runs is outstanding".
> Everything that runs has since landed, verified against the tree: the
> deterministic scheduler is `crates/ply-eval/src/sched.rs`, the DPOR search in
> the backtrack-set formulation is `crates/ply-eval/src/explore.rs` — which sets
> `exhaustive` from the search rather than asserting it — and `--simulation` is
> a real option group on `ply test`, `ply prove` and `ply review`
> (`crates/ply-cli/src/cli.rs`). `Isolation` and the ambient-atom rule are
> `crates/ply-test/src/schedule.rs`.
>
> Two entries in "Not in M7" have also stopped being true; each is marked there
> rather than here.
Builds on: `0005-control-stack-and-world.md`, whose threaded world and explicit
control stack are the two things this milestone is impossible without.

**What is landed**, so that six concurrent implementations cannot disagree about
the parts a disagreement would be silent in: `ply_span::codes`' four new codes;
`ply_eval::sim`'s `Seed`, `Stream`, `Plan`, `Access`, `StepFootprint`, `Race` and
`Exploration`; `ply_test`'s ambient-atom rule and the two cache keys. Everything
that *runs* — the grammar, the typing rule, the prelude effects, the scheduler,
the search — is specified here and implemented against this.

## Context

ROADMAP.md's M7 reads: *"Virtualized scheduler, clock, network, and RNG driven by
a seed. The repro artifact handed to an agent becomes a seed rather than a stack
trace. Thousands of adversarial interleavings per change."*

**Ply has no concurrency primitive.** Tests run concurrently at the runner level
through `rayon`, but a Ply program cannot spawn anything. There is nothing to
interleave. So the first thing M7 has to do is introduce concurrency, and *how*
it is introduced decides whether the rest of the milestone is a language feature
or a bolted-on debugger.

The answer the language already implies: **concurrency is an effect.** DESIGN.md
§2 argues that a test double must satisfy the same declared signature as the real
resource, because that is what stops the two from drifting. Apply the argument to
the scheduler itself. `task.spawn` is an operation with one declared signature;
in production a handler runs tasks on threads, in simulation a seeded handler
interleaves them deterministically, and neither can drift from the other because
the signature is written once. A scheduler is a test double for the operating
system.

That framing is also what makes M6 the prerequisite rather than a coincidence of
ordering. A task is a suspended machine state. Suspending and resuming at effect
boundaries is exactly what the explicit control stack bought, and the threaded
world of §3 of that ADR — a resumption observes the world as of the handler's
call to `resume`, never as of the capture — is precisely the semantics of shared
memory. Had ADR 0005 chosen snapshot-at-capture, two tasks would not see each
other's writes and this milestone would be unimplementable. The hardest decision
in M6 is the one M7 spends.

## Decision

### 0. The rule everything else follows from

> **A simulated run is a pure function of its definition set and its seed.**
>
> Every source of nondeterminism a Ply program can reach is an effect, and
> simulation is a handler for it. Nothing else enters: no wall clock, no thread
> identity, no address, no iteration order that is not itself specified.

Everything below is a consequence. §7 is the load-bearing one — it is what lets a
time-dependent, randomized, concurrent test be an ordinary `det`, cacheable test
— and §8 is the one that decides whether the cache is honest about it.

---

### 1. Concurrency is an effect

#### 1.1 The prelude effects

Four effects are declared by the language rather than by a module. They are
written here in Ply's own syntax because that is what they mean; §1.4 says which
part of it the parser does not yet accept.

```ply
nondet effect task {
  write spawn<a | e>(body: () -> a / e) -> Task<a> / e
  write join<a>(t: Task<a>) -> a
  write yield() -> Unit
}

nondet effect clock {
  read  now() -> Int
  write sleep(nanos: Int) -> Unit
}

nondet effect random {
  write next() -> Int
  write below(bound: Int) -> Int
}

effect sim {
  read seed() -> Int
}
```

Every operation is a singleton-resource operation: there is one scheduler, one
clock and one random stream per simulated region, so `[r]` would name a
distinction that does not exist. The atoms are `task.write`, `clock.read`,
`clock.write`, `random.write` and `sim.read`.

The mode annotations are not decoration and each one is load-bearing in §6:

| atom | mode | why |
| --- | --- | --- |
| `task.write` | write | spawning, joining and yielding all mutate the scheduler |
| `clock.read` | read | `now()` observes virtual time; it does not move it |
| `clock.write` | write | `sleep` changes when this task is next runnable, which changes what `now()` answers elsewhere |
| `random.write` | write | drawing *advances the stream*, so two tasks drawing in the other order get the other values |

`random.next` being a **write** is the sharpest of these. It means two tasks that
both draw conflict, so their order is a real difference, so §6 explores it rather
than pruning it. A design that declared drawing a `read` would have quietly
hidden a whole class of order dependence. The effect system asked the right
question and the honest answer prunes less.

An effect declaration whose *program-wide* name equals one of `task`, `clock`,
`random` or `sim` is `DUPLICATE_DEFINITION`, pointing at the prelude. This bites
only in an anonymous module — `examples/clock.ply` declares `nondet effect clock`
inside module `clock`, so its program-wide name is `clock.clock` and it shadows
the prelude by the ordinary resolution order, unchanged and uninvolved.

#### 1.2 `task` is `nondet`, and that is the thesis restated

Concurrency without a specified scheduler *is* nondeterminism, so the type says
so. A `det` test that spawns a task and installs no scheduler is `E0412`, with
the same message shape a `clock.now()` gets today:

```
error[E0412]: nondeterministic effect in a deterministic test
  ┌─ src/ledger.ply:31:11
31│   let t = task.spawn(|| settle(order));
  │           ^^^^^^^^^^^^^^^^^^^^^^^^^^^ performs `task.write`, and `task` is declared `nondet`
  = handle it here, e.g. `simulate { <body> }`
  = or declare this `test/nondet`, which opts out of the cache and re-runs every time
```

*A deterministic test may not contain unscheduled concurrency.* That sentence is
worth more than the feature it describes: it is the first time the language's
central claim — nondeterminism is in the type, so a flake fails to compile — has
had something to say about the largest real source of flakes.

#### 1.3 Structured, and what structure costs

**Spawn is structured. A task spawned inside a scheduler's scope is joined before
that scope's value is delivered.** Structure is not a new construct: **the
handler is the scope.**

```ply
simulate {
  let a = task.spawn(|| transfer(alice, bob, 50));
  let b = task.spawn(|| transfer(bob, alice, 30));
  task.join(a);
  task.join(b);
  assert_eq(balance(alice) + balance(bob), 100)
}
```

The `simulate` region delimits the scheduler exactly as `handle` delimits any
other handler, and ADR 0005's "handle is the only delimiter" is what makes that
the whole of the structure rule. When the region's body returns, the scheduler
drains whatever is still runnable before delivering the value; a task that
outlives every join still finishes inside the region that made it. No task, no
timer and no pending draw survives the `}`.

**What structured costs.** Three things, stated rather than glossed:

- **No daemon tasks.** A background worker that outlives its scope cannot be
  written. The region ends when the last task ends, so a task that never
  terminates deadlocks the region rather than being abandoned at the boundary.
  In a simulation that is the right trade — an abandoned task is an unexplored
  interleaving — but it is a real restriction on what production code the model
  can mirror.
- **No task escaping in a value.** A `Task<a>` is a key into the scheduler's
  state and the scheduler dies with the region, so a `Task` in the region's
  result type is `E0413`, the same result-type check `with_cell` uses.
- **Cancellation is not in M7.** Structured concurrency usually comes with it,
  and a cancel is an interleaving point with its own semantics. It is deferred
  rather than approximated.

**What structured buys, which is why it wins.** The scheduler's state is a
region-local value, so it contributes nothing to any row that escapes; the set of
live tasks is finite and known at every scheduling point, which is what makes an
*enabled set* computable and therefore what makes §6's exploration a
finite-branching search at all; and "every task is blocked" becomes a decidable
question, which is what turns a hang into the `E0414` diagnostic in §5. Free
spawn gives up all three, and gives up the last one permanently: with tasks
outliving their creator there is no point at which the absence of progress is
observable.

#### 1.4 The one type-system extension, and its limit

`spawn` cannot be typed by today's `OpInfo`. Two things it needs that a
user-declared operation does not have:

- **type polymorphism** — `Task<a>` for whatever the body returns;
- **an effect row on the operation itself** — `/ e`, so that the effects of the
  spawned body appear in the row of the code that spawned it.

That second one is not a convenience. If `spawn`'s row did not carry `e`, a test
that spawns a task writing `db.write[orders]` would report an empty footprint,
the cross-test conflict graph would run it beside a test reading `orders`, and
the scheduling story would be silently wrong. The spawned body's effects *do*
happen, inside the region, so the row must say so.

**The extension is `OpInfo::scheme: Option<Scheme>`, and nothing else.**

- `None` for every operation a user declares. Those stay monomorphic and their
  typing is unchanged.
- `Some(scheme)` for a prelude operation, whose signature is constructed by
  `ply-core` rather than parsed. `Scheme` already carries `ty_vars`, `row_vars`
  and a `Type`, and `Type::Fn` already carries an `effects: Row` — so the shape
  needed exists and nothing in `ply-core::ty`, which is pinned, moves.
- At a perform site: if a scheme is present, instantiate it, unify the arguments
  against its parameters, and union its instantiated row with the performed
  atom. Today's monomorphic case is that rule with an empty row, so there is one
  code path.

**Surface syntax for declaring a polymorphic operation is not in M7.** `task` is
the only client, a general feature serving one client is a feature designed
against one example, and the day a second client appears the generalization is a
grammar change over machinery that already exists. Deliberately conservative;
say so in the ADR rather than discovering it in the diff.

The other half of the same decision: **a user may still *handle* `task`.** That
needs no declaration at all, and a sequential scheduler is eight lines of
ordinary Ply:

```ply
handle body() with {
  task.spawn(f)  resume k -> k(done(f())),
  task.join(t)   resume k -> k(value_of(t)),
  task.yield()   resume k -> k(()),
  return x -> x
}
```

Three handlers for one signature — this one, the seeded one M7 ships, and the
threaded one M9 will — and no way for them to drift, because the signature is
written once and every one of them is checked against it. That is DESIGN.md §2's
argument, applied to the operating system.

#### 1.5 `Task<a>` is a key, not a pointer

```rust
pub struct TaskId(pub u32);          // Display: "@3"
```

`Value::Task(TaskId)` mirrors `Value::Cell(CellId)` exactly, and for the reason
ADR 0005 gives: a key cannot dangle, two keys cannot alias across a fork, and
identity comparison is integer comparison. Ids are assigned from a per-region
counter at the `spawn` perform, so they are a function of the interleaving and
therefore of the seed — which is what lets a failure artifact name `@2` and mean
the same task on replay.

Joining a task whose region has ended is `E0413` at run time. The static
result-type check catches the ordinary mistake; a `Task` smuggled out through a
captured continuation is the case no result type mentions, and it gets a
diagnostic rather than a wrong answer.

#### 1.6 Control that crosses the region's delimiter

A `handle` written *outside* a `simulate` region may answer an effect performed
*inside* one, and the capture then crosses the region's delimiter. That is
ordinary, useful and common — it is how a task's own effects are discharged by a
test double the test installed. Three cases, and only the first is quiet:

- **Resumed once.** The region carries on. Its **anchor moves**: the stack it
  eventually delivers its value onto is the one the splice put it over, not the
  one `simulate` was entered on. Restoring the entry stack instead silently
  discards whatever the resuming clause still had pending, which is a wrong
  answer with no diagnostic and an exhaustiveness claim made over a program the
  machine did not run correctly.
- **Never resumed.** The region is abandoned with tasks still runnable, which
  §1.3 forbids for exactly the reason it gives: an abandoned task is an
  unexplored interleaving. Every step past the abandonment is missing from the
  recording, so DPOR's completeness precondition — every explored execution runs
  all processes to completion — is violated, and the search reports `exhaustive`
  over schedules it cut short. **A run that ends with a region still live is
  `E0413`**, naming the unfinished tasks.
- **Resumed twice.** The second resumption re-enters a region that has already
  delivered its value; its scheduler is gone, and re-running its tasks against a
  fresh one would be a different program. Forking a live scheduler needs the
  world snapshot ADR 0005 refused as "a capability with no type-level account",
  so **a continuation re-entering an ended region is `E0413`** — the diagnostic
  §1.5 promises rather than the wrong answer. Multi-shot continuations across a
  region delimiter are the one place M6's machinery does not reach into M7, and
  saying so is cheaper than a silent half of it.

---

### 2. `simulate`

#### 2.1 Syntax

```
simulate := "simulate" block
```

`simulate` is a **contextual** keyword, recognized only immediately before a `{`,
exactly as `with_cell` is recognized only immediately before a `[` and `resume`
only between a clause's `)` and its `->`. `lexer::is_ident("simulate")` stays
true and no `Kw::Simulate` is added.

There is no seed in the syntax. A seed written in source would be part of the
definition's hash, which would make every seed a different definition and every
widening of the search a rewrite of the program. The seed arrives as an effect.

#### 2.2 The typing rule

```
Γ ⊢ body : T / ρ_b
handled = { task.write, clock.read, clock.write, random.write }
sim.read ∉ ρ_b                                          (else E0416)
T mentions no Task<_>                                   (else E0413)
────────────────────────────────────────────────────────────────────
Γ ⊢ simulate { body } : T / ( (ρ_b \ handled) ∪ {sim.read} )
```

It is the `handle` rule with a fixed clause set, plus one atom of its own. Three
consequences worth naming:

- **`sim.read` is the seed dependency, in the type.** `fn f() -> Int /
  {sim.read}` says *this is a function of the seed*, which is exactly what it is.
  The atom propagates through calls by the ordinary row rules, so a test whose
  closure reaches a `simulate` region carries it with no new analysis, no new
  field on `CheckOutput`, and no cooperation from the incremental front end
  beyond what footprints already get. §8 keys the cache off it.
- **`sim` is not `nondet`,** because a seed is an input rather than a
  nondeterminism. A `det` test may carry `sim.read`. This is the entire type-level
  content of §7.
- **`simulate` discharges the three effects it can simulate and nothing else.**
  A user's own `nondet effect http` inside a `simulate` region still trips
  `E0412`. The language does not get to claim it simulated an effect it has never
  heard of, and that is the safety property that survives.

`cell` is deliberately not in `handled`. Cells are world state; `with_cell`
discharges them at its own region boundary as always, and a `with_cell` *outside*
a `simulate` region holding state that tasks inside share is exactly how tasks
share memory.

#### 2.3 Nesting is refused

`sim.read ∈ ρ_b` means the body itself reaches a `simulate` region, whether
lexically or through a call. That is `E0416`, and the test is exact and
transitive for free.

Two schedulers means two notions of "runnable", a task in the inner region
blocking the outer one, and an exploration whose state space is a product nobody
has an account of. Refusing it costs a program shape nobody has asked for, and
the rule is one row-membership test.

#### 2.4 The value a region delivers does not depend on the budget

> **The value and the world a `simulate` region delivers are those of the
> interleaving its seed names. Every other interleaving explored is a search, and
> its world is discarded.**

Without that rule, raising `--sim-budget` would change what a program *means*,
and the budget is a search parameter rather than a semantics. With it, the budget
and the mode change only the thoroughness of a test.

Exploration is therefore a **test-time** activity and is implemented as
whole-test replay: `ply test` runs the entire test once per interleaving, each
from a fresh fork of the base world, which is what every entry point already
does. `ply run` explores exactly one interleaving — the one its seed names — and
under `ply run` a `simulate` region is an ordinary deterministic scheduler.

That choice is worth a sentence of justification, because the alternative looks
cheaper. Re-running only the *region* would need the world as of region entry
restored per interleaving, which is the world snapshot/restore builtin ADR 0005
explicitly refused as "a capability with no type-level account". Whole-test
replay needs no such capability: a test is re-run, so its writes are re-done
rather than un-done, and ADR 0005's monotone world survives M7 untouched. It
costs re-doing whatever setup precedes the region, per interleaving. That is the
price of not putting an un-do into a language whose rows report every do.

---

### 3. The scheduler runs on the control stack

#### 3.1 A task is a continuation

The seeded scheduler is a **native prompt**: a delimiter on the M6 stack whose
clauses are Rust rather than Ply. It is installed by `simulate` and it handles
`task.*`, `clock.*` and `random.*`.

```
Delimiter ::= Ply(Prompt)          -- a `handle` expression, as today
            | Sim(SimPrompt)       -- the seeded scheduler
```

`Segment::under` gains a native form and `Stack::find_handler` consults both.
Everything else in `ply-eval::cont` is unchanged: capture is still one entry per
enclosing delimiter crossed, resume is still a splice, and the world is still
threaded through both.

A task is `(TaskId, Continuation, TaskState)`. When a task performs a
scheduler-visible operation, the machine captures the continuation up to the
`Sim` delimiter exactly as it would for a general Ply clause, hands it to the
scheduler, and the scheduler decides which task to resume next. Resuming task *T*
with value *v* is `⟨Return(v), K.resume(k_T), W⟩` — the one transition ADR 0005
§1.3 already specifies for applying a continuation, with no addition.

That is the whole of "the scheduler runs on the M6 control stack", and it is
worth noticing how little there is to it. Every mechanism it needs — capture,
splice, deep handlers, one threaded world — was landed for multi-shot
continuations and none of it was designed for this.

#### 3.2 Steps and scheduling points

> A **step** of task *T* runs from the scheduler's resumption of *T* up to and
> including *T*'s next scheduler-visible perform, or *T*'s completion.
>
> A **scheduling point** is the boundary between two steps. The scheduler's
> choice at a scheduling point is the only choice a simulated run makes.

A step's **access set** is every atom the tracer recorded and every cell the
world recorded during that step, **excluding the terminating `task.*` /
`clock.*` / `random.*` atom itself**. That exclusion is not a fudge and §6.1
justifies it: the scheduler is the explorer, not a participant, and counting its
own bookkeeping as a shared access would make every pair of steps dependent and
delete the reduction the milestone exists to demonstrate.

#### 3.3 Interleaving points are *scheduler-visible* performs, and what that costs

Two tasks cannot be interleaved more finely than their `task.*`, `clock.*` and
`random.*` performs. §3.2 is the definition and this section is what it means;
the two must be read together, because the difference between "a perform" and "a
perform the scheduler answers" is the whole of the milestone's exhaustiveness
claim.

**A scheduler cannot suspend a task at a perform it does not answer.** The
scheduler's only power is to decide who runs next at a point where control has
already reached it, and control reaches it exactly when a task performs one of
the three simulated effects. A `db.get[counter]` answered by a handler *outside*
the region never crosses the region's delimiter, so there is no moment at which
the scheduler could have chosen someone else.

The reduction claim is therefore over that model and not over a hypothetical
instruction-granular one:

> `exhaustive: true` means every interleaving **at scheduler-visible
> granularity** has been executed.

**What that costs, said plainly rather than glossed.** A task that reads shared
state and writes it back with no `task.*`, `clock.*` or `random.*` perform in
between runs the two as *one step*, and no schedule separates them. The classic
lost update is therefore **not** found unless something in the window is
scheduler-visible:

```ply
let n = db.get[counter](k);      // answered outside the region: not a step end
db.put[counter](k, n + 1)        // so no schedule runs the other task between
```

```ply
let n = db.get[counter](k);
task.yield();                    // a scheduling point — now the race is reachable
db.put[counter](k, n + 1)
```

Three ways to make such a window explorable, in the order to reach for them:

- **`task.yield()`** between the read and the write, which is what production
  code's real preemption point corresponds to and what every in-tree lost-update
  fixture uses;
- **a `clock.now()` stamp**, which real code writes anyway — `examples/bank.ply`
  is exactly this shape, and it is why the bug there survives review;
- **push the check into the resource**, which is the fix rather than the test:
  `bank.take` decides and debits in one operation and no schedule can separate
  them, because there is nothing to separate.

**Why not interleave at every perform.** It is not free and it is not obviously
more honest. Every user perform becoming a scheduling point means the scheduler
must suspend a task at an operation it does not answer and re-issue it on
resumption, which is machinery; `cell_get` / `cell_set` are *builtins* rather
than performs, so shared world state would still not be covered without making
every builtin a scheduling point too; and the state space grows by a factor of
the perform count per task, which turns `exhaustive` from the common case into
the rare one. The rule that makes the milestone's headline true — `exhaustive`
is a proof — is worth more than a wider model whose searches never finish. M7
takes the narrower model and says so; widening it is a milestone of its own.

#### 3.4 The world is threaded, and that is why this works

There is exactly one current world at every point of a simulated run, and
resuming a task does not touch it. So task *B* sees task *A*'s writes when *B*
next runs, and does not see them before. That is shared memory, and it is
ADR 0005 §3's rule with no addition:

> State is a value the machine threads. Control is a value the machine splices.

Had the machine restored the world at each resumption, every task would run
against the world as of its own suspension, tasks would never observe each other,
and no interleaving would ever differ from any other. The reason to re-read
ADR 0005 §3.1 before touching this code is that the wrong reading of it makes
this milestone silently vacuous rather than loudly broken.

#### 3.5 Enabledness: join, sleep, and stuck

A task is **enabled** when the scheduler may resume it. It is not enabled when:

- it is blocked in `task.join(t)` and `t` has not completed;
- it is blocked in `clock.sleep(d)` and virtual time has not reached its
  deadline.

Enabledness — not the dependence relation — is how synchronization is
represented. An implementer who tries to encode "join happens after the child
finishes" as a conflict will get a search that explores impossible schedules and
then prunes them; the enabled set makes them ungenerated. The two mechanisms
answer different questions and §6 depends on keeping them apart: enabledness says
what *could* run, dependence says whether the order *matters*.

When no task is enabled:

- if at least one is blocked on a timer, virtual time advances (§5);
- otherwise the region is **stuck**, which is `E0414`.

#### 3.6 One choice sequence per entry point, not per region

> **Scheduling point *i* is the *i*th of the run.** One `Seed::path`, one `sched`
> stream, one `rand` counter and one step record are shared by every `simulate`
> region an entry point enters, and each step records which region took it.

Nesting is refused (§2.3); **sequence is not**, and it cannot be. A test may
write two regions one after another, and an ordinary function whose body is a
region reaches one twice with no syntax pointing at it. That shape is legal,
well typed and unremarkable, so everything downstream has to be correct over it.

Two consequences, and both are silent when they are wrong:

- **The search's input is the whole entry point.** A record covering one region
  gives the search a trace describing that region alone. The other regions'
  choice points are never branched on, their races are never explored, and the
  run is still reported `exhaustive: true` and cached green. `exhaustive` is the
  number §6.4 invites a project to watch go up, so a wrong one is the worst
  artifact this milestone can produce.
- **A path entry means one thing.** With a per-region counter, `path[0]` names
  the first choice of *every* region, so a backtrack point aimed at one region
  silently re-aims the others. When a later region's shape depends on what an
  earlier one raced to, the enabled set at the point the seed names is a
  different set on replay, `E0415` fires, and an ordinary program is reported as
  a defect in Ply.

Because two regions run in sequence and never interleave, a pair of steps from
different regions can never be reordered — and a `TaskId` means a different task
in each — so `StepRecord::region` is carried and the search skips any pair that
crosses it. Virtual time still restarts per region (§5.1): it is time since
*that* region was entered. The `rand` stream does not restart, because a draw is
a draw of the run.

---

### 4. The seed

#### 4.1 What a seed is

```rust
pub struct Seed { pub root: u64, pub path: Vec<u16> }
```

- `root` seeds the random streams (§4.2).
- `path` is a **choice-sequence prefix**: at scheduling point *i*, if
  `i < path.len()` the scheduler resumes `enabled[path[i]]`; otherwise it draws
  from the `sched` stream. A bare seed has an empty path.

Canonical text form: `7` when the path is empty, `7:3.0.2` when it is not.
`Seed::parse` accepts both, and `ply test --seed 7:3.0.2` replays exactly.

**Why a seed is not just a `u64`.** A systematic search (§6) needs to say "the
interleaving that is like this one but takes the other branch at point 3", and in
general no `u64` produces exactly that. The choice is between a single artifact
whose grammar has two fields and two artifacts a consumer has to carry together.
One artifact wins: it is one string in the JSON, one flag on the command line,
one value in `Failure`, and the common case — a randomly sampled failure — still
prints as a bare number.

#### 4.2 The streams

Two independent streams are derived from the root by domain separation:

```
draw(domain, counter) = blake3( b"ply.sim.stream.1" ‖ root_le_u64
                              ‖ domain_u8 ‖ counter_le_u64 )[0..8]  as u64 le
```

| domain | byte | drawn for |
| --- | --- | --- |
| `sched` | 0 | which enabled task to resume |
| `rand` | 1 | `random.next` / `random.below` |

Each domain has its own counter, incremented per draw. **They must not share
one**, or adding a `random.next()` call to a program would shift the interleaving
and the two axes of the search would be entangled — a change to the data would
silently become a change to the schedule, and a bisection over it would name the
wrong definition.

Counter-mode BLAKE3 rather than a PRNG crate, because "the same seed produces the
same result on any machine" is a cross-version promise and a dependency's
generator is not. BLAKE3 is already in the workspace, is byte-specified, and is
its own test vector.

Range reduction is **rejection sampling**, specified exactly because "unbiased" is
not a specification: for a bound `n > 0`, let `limit = (u64::MAX / n) * n`; draw
until `x < limit`; answer `x % n`. `random.below(n)` with `n <= 0` is
`RUNTIME_ERROR`.

**There is no `clock` stream.** Virtual time is not drawn; it advances by the
rule in §5, which is a function of the sleeps that have been requested and
therefore of the interleaving and therefore of the seed. Jitter would buy a
dimension of search at the price of §5.2's exact-timeout property, which is worth
more.

#### 4.3 Hygiene, as normative requirements

A seeded run is a pure function of definitions and seed only if nothing else
leaks in. These are requirements on `ply-eval::sim`, not advice:

1. No hash-based collection may be named anywhere in the module. Not "no hash map
   iteration" — a rule about how a type is used is a rule nobody enforces. Run
   queues are `Vec` in insertion order; sets are `BTreeSet`.
2. No `std::time`, no thread identity, no `rayon`, no `rand`.
3. No pointer value, address, `Rc::as_ptr`, refcount or allocation order may be
   observed by any decision.
4. Task ids come from the region's own counter, assigned at the `spawn` perform.
5. `World::cells` iterates ascending by id, which it already guarantees, and
   nothing else may iterate the world.

Requirement 1 is enforced by a test that greps the module's sources, which is
blunt and is the kind of check that actually catches the regression six months
later. Requirements 2–5 are pinned by the replay tests in §10.

#### 4.4 Replay

`ply test --seed <SEED>` runs every simulated test at exactly that seed, once,
with no exploration. That is the reproduction path an agent is handed, and it is
the same path `--sim once` takes.

A failure reports its seed, and that seed goes into M5's failure artifact:
`Failure::seed: Option<Seed>`, `"seed": "7:3.0.2"` in `--json`, and a replay line
in the terminal summary.

---

### 5. The virtual clock

#### 5.1 Time advances only when nothing can run

`clock.now()` answers the current virtual time, in **nanoseconds since the region
was entered**, starting at `0`. It does not advance it. `clock.sleep(d)` blocks
the calling task until virtual time reaches `now + d`; `d <= 0` is a yield.

> **Virtual time advances at exactly one moment: when no task is enabled and at
> least one is blocked on a timer. It jumps to the earliest deadline among them,
> and every task whose deadline equals that time becomes enabled at once.**

Three things follow, and each is a property a real test suite wants:

- **A simulated run's virtual duration is a function of its sleeps, not of the
  machine.** A loaded CI box and an idle laptop agree.
- **A sleeping test costs no wall clock.** `clock.sleep(30_000_000_000)` is a
  jump, not thirty seconds. Retry-with-backoff logic becomes testable at its real
  timings.
- **Tasks that wake together race, and that race is explored.** Their relative
  order at the shared deadline is a scheduler choice like any other, so §6 covers
  it. This is the timer-coalescing bug that is nearly impossible to hit on a real
  clock.

No wall-clock read reaches a simulated run because there is no operation that
performs one: `clock` is a `nondet` effect and the region handles it. A program
wanting real time outside a region declares its own effect and gets `E0412` in a
`det` test, exactly as today.

#### 5.2 What a timeout means

M7 has no timeout primitive. A timeout is `clock.sleep` racing something else,
and it fires when virtual time reaches its deadline — which happens **only at an
idle point**. So:

> **A simulated timeout never fires early.** It cannot pre-empt work that could
> still run, because time does not move while anything is runnable.

That is the exact opposite of a wall-clock timeout, whose whole failure mode is
firing because the machine was busy. A test asserting "this completes within five
seconds of simulated time" is an assertion about the program rather than about
the hardware, and it is not flaky — it is exact.

#### 5.3 What virtual time cannot tell you, stated plainly

Virtual time does not advance for computation. A run in which every task is CPU
bound takes **zero** virtual nanoseconds. So a simulated test cannot detect that
an implementation got slower, cannot distinguish an O(n) step from an O(n²) one,
and must not be read as a performance test. Benchmarks measure wall clock and
live in `benches/`; simulation measures order and time-of-arrival and lives in
tests. Conflating them would produce a benchmark that is deterministic and
meaningless.

---

### 6. Footprint-guided exploration

This is the payoff of the whole language design, so it gets the most exact
treatment.

#### 6.1 The dependence relation, which the language already has

> Two steps are **dependent** iff their access sets conflict. Two steps that are
> not dependent commute: executing them in either order from the same
> configuration reaches the same world and the same result. Exploring both orders
> is therefore redundant, and a scheduler that explores both is doing work it can
> prove is useless.

Partial-order reduction algorithms spend most of their complexity *approximating*
this relation from an alias analysis. Ply computes it exactly, at resource
granularity, and has been computing it since M2 — it is `Footprint::conflicts_with`,
the same predicate that decides which tests may run concurrently. DESIGN.md's
claim that resource granularity is "exactly the information needed to decide
whether two tests can run concurrently" turns out to have been one instance of a
more general fact.

**The access set is finer than a `Footprint`, in exactly one place.** A step's
accesses are:

```rust
pub enum Access {
    Atom(EffectAtom),
    Cell { id: CellId, mode: Mode },
    Alloc,
}
```

- Two `Atom`s conflict by `EffectAtom::conflicts_with`, unchanged.
- Two `Cell`s conflict iff they name the **same `CellId`** and at least one is a
  `Write`.
- Two `Alloc`s always conflict.
- Anything else never conflicts.

**`Alloc` is the case the soundness condition above rules on and the type
system cannot.** `with_cell` takes the next id from the world's own counter, and
allocation has no location to name — that is the point of it. Two tasks that each
open a private cell therefore look like tasks that touch nothing, and run in the
other order they reach a **different world**, because the two ids are swapped.
"Not dependent ⟹ reaches the same world" is false of them. No surface construct
observes a `CellId` today, so this cannot yet flip an assertion; the world is
what §2.4 and §10 pin, it is what `--engine both` compares, and `Access`'s own
`Display` already prints an id that depends on allocation order. A relation that
is right only until someone can observe it is not a relation, it is a coincidence.

Cell granularity is `CellId` rather than the `[r]` label because a cell is a
location and the label is a name several locations may share. Using the label
would be sound and coarser; using the id is sound and exact, and it costs
nothing, because a `cell_get` already holds the id.

**The machine does not record cell accesses today and must learn to.** ADR 0005's
tracer records a `perform`, and it explicitly excuses itself from cells: a
`cell_get` is a builtin over a `CellId` "that carries no resource label, and the
world comparison below is the stronger statement about those effects anyway".
That was true when the only consumer was `--engine both`, where comparing final
worlds does subsume it. It is false here — a step's accesses are needed *during*
the run, per step, and a cell is the main way two tasks share state. A build that
leaves cells out of the relation explores one interleaving of every cell-backed
race in the corpus and reports a large reduction for having done it.

**The correction that would otherwise be a silent unsoundness.** ADR 0005 §5
exempts world-backed atoms from the conflict graph: two tests that both write
`cell.write[users]` do not conflict, because each test holds its own forked
world. That exemption is about **two tests**, and it does not survive being
applied to **two tasks**:

> Two tests hold two worlds. Two tasks in one simulated run hold **one** world.
> `ply_test::shared_footprint` is for the scheduler of tests and drops `cell`
> atoms. The simulation's dependence relation keeps them, at cell granularity,
> and a build that reuses `shared_footprint` here prunes away every shared-memory
> race in the corpus while reporting a larger reduction for having done it.

That is the single most expensive mistake available in this milestone, because
its symptom is a *better* number.

**Why the terminating scheduler op is excluded from the access set.** Every step
ends in a `task.*` / `clock.*` / `random.*` perform, and all of those are writes
to a singleton resource. Included, every pair of steps would be dependent and the
reduction would be exactly 1×. The exclusion is sound because the scheduler is
the *explorer*: its state is a function of the choice sequence, and the search
enumerates choice sequences. Two steps that touch disjoint program resources
commute even though both went through the scheduler. Synchronization is not lost
by this — it is carried by enabledness (§3.5), which is a different mechanism on
purpose.

`random.write` is the exception that proves the rule and must not be excluded
with the rest: the value a draw returns is observed by the *program*, not by the
scheduler, so a draw is a genuine read-modify-write of shared state. Two steps
that both draw are dependent. The rule is therefore: exclude the terminating
`task.*` and `clock.*` atom; keep `random.write` in the access set of the step
that performed it.

#### 6.2 The search

Dynamic partial-order reduction, in the backtrack-set formulation, with the
dependence relation of §6.1 substituted for the alias analysis the literature has
to approximate.

```
explore(prefix):
    run the test at Seed { root, path: prefix }
    record steps s₁..sₙ with access sets A₁..Aₙ and enabled sets E₁..Eₙ
    if the run failed: report it, with this seed
    for i = n down to 1:
        for each j < i such that
              Aⱼ ⋈ Aᵢ                                   -- dependent
              and task(sⱼ) ≠ task(sᵢ)
              and no k in (j, i) has task(s_k) = task(sᵢ)  -- not scheduled between
              and task(sᵢ) ∈ Eⱼ:                        -- and could have run then
            backtrack[j] ∪= { task(sᵢ) }
    for each j, for each t in backtrack[j] not already explored at j:
        explore(prefix[0..j] ++ [index of t in Eⱼ])
```

Notes an implementer needs:

- **A dependent pair is not yet a race.** The pseudocode above asks whether two
  steps conflict and whether the later task could have run earlier. It does not
  ask whether the two could ever have run in the other order, and on the
  ordinary shape of a concurrent test — spawn, join, then assert on what the
  children wrote — the answer is usually no: the join already ordered every
  child step before every assertion. Queueing those pairs is not conservative,
  it is wasted, and it was measured at 992 interleavings where one was correct.

  The scheduler therefore carries a **vector clock per task**, advanced one tick
  per step, inherited by a child at `spawn` and merged into a joiner when its
  target finishes. `StepRecord::stamp` is the acting task's clock as of that
  step, and a dependent pair whose earlier step happens-before the later one is
  not a backtrack point. Those are the region's only two synchronization edges;
  a timer waking a task adds none, because time advancing is the scheduler's
  decision rather than the program's, so two tasks that wake together are racing
  and must be treated as racing.

  This is a filter over reachability and not a second dependence relation. It
  cannot hide a race — the reordering it refuses to queue is one no schedule
  produces — and `Dependence::All` does not consult it, so the naive baseline
  still counts every schedule. `ply-corpus sim` reports both counts beside a
  third that withholds the clocks, which is what the search did before it had
  them.

- **Sleep sets are not in M7.** They are a further reduction on top of backtrack
  sets and they are where DPOR implementations get subtly wrong. Backtrack sets
  alone are sound and already produce the reduction the milestone claims.
- **The search is bounded** by `--sim-budget` interleavings per root. When the
  frontier empties before the budget, the search is **exhaustive** over the
  Mazurkiewicz-trace equivalence classes, and that is reported.
- **Replay is self-checking.** Re-running a prefix must reproduce the same
  enabled set at every choice point the prefix names. A mismatch means the run
  was not a function of the seed, which is `E0415` — Ply's fault, not the
  program's, and the same class of defect as an engine divergence.

Modes:

| `--sim` | roots | what it does |
| --- | --- | --- |
| `once` | 1 | one interleaving, the one the seed names. The replay path. |
| `random` | 64 | one interleaving per root. No state, embarrassingly parallel. |
| `dpor` | 1 | the search above. **Default.** |

`dpor` is the default because it never runs two interleavings from one
equivalence class, so every unit of work is new information, and because when
the space is small it finishes *exhaustively* — which is a proof rather than a
sample.

**It does not dominate sampling at finding a bug fast, and measurement says so.**
A systematic search enumerates equivalence classes from one end; a sample jumps
around. On `tests/fixtures/bank_race.ply` the two are level — a median of two
interleavings each over 128 trials — and `dpor` wins only on the worst case,
3 against 18. On a race that lives in a corner of the space rather than most of
it, sampling wins outright: a four-task lost-update fixture asserting that at
most two updates are lost is found at a median of 8 sampled interleavings and 96
searched, worst case 50 against 573. `random` is what to reach for when the goal
is to find a bug in a space too large to exhaust; `dpor` is what to reach for
when the goal is to prove there is none. Those are different goals and the modes
are not ordered.

#### 6.3 The reduction, measured

A number that is not measured is a slogan, so:

> **`--measure-reduction` runs the same search a second time with the dependence
> relation forced to `true`, and reports both counts.** Forcing it degenerates
> DPOR into exhaustive enumeration of every schedule respecting per-task order
> and enabledness, which is exactly the naive scheduler the reduction is claimed
> against. Same code, one flag, no second implementation to disagree with the
> first.

```
$ ply test --explain --measure-reduction
   ✓ transfers are atomic under any interleaving
       12 interleavings · exhaustive · naive 720 · 60× reduction        3.1ms
```

When the naive search spends its own (much larger) budget, the count is reported
as a lower bound — `naive >= 4096` — and never as an exact number that was not
observed.

It is off by default: the claim is a benchmark, not something every run should
pay double for. It is on in the audit test and in `benches/`, where the corpus
number lives.

**What it measures, on the corpus.** `ply-corpus sim` sweeps task count against
conflict density. At one step per task, five tasks over five shards is one
interleaving against a naive search that spends a 20,000 budget; the same five
tasks over one shard is 630 against the same bound. The ratio is a function of
contention and falls to nothing at full contention with several steps each,
where every pair of steps really is dependent and there is nothing to prune —
five tasks × two steps on one shard spends its budget on both sides. That is the
honest shape of the claim: the reduction is large exactly where the resource
labels say the work is independent, which is what resource granularity was for,
and it is 1× where they say it is not.

#### 6.4 Exhaustiveness is the headline, not the count

```
   simulated: 3 of 47 · 61 interleavings · 3 exhaustive
```

`exhaustive: true` on a test means **every interleaving of that test has been
executed** — not sampled, not fuzzed, enumerated up to an equivalence that
provably preserves outcomes. Concurrency testing does not usually get to say
that, and it is available here only because the equivalence comes from the type
system rather than from a heuristic.

"Every interleaving" is at §3.3's granularity — every schedule the *scheduler*
could have chosen — and §3.3 states what that excludes. It covers every region of
the test, not one of them: the whole entry point is one choice sequence (§3.6),
so a test with two `simulate` regions in sequence is exhaustive over the product
of both regions' choice points or over neither.

`exhaustive` is per test, is reported in `--explain` and in `--json`, and is
aggregated in the summary. A project watching that number go up is watching its
concurrency get proved rather than probed.

#### 6.5 The race goes in the failure artifact

When the search flips a passing interleaving to a failing one, it knows exactly
which backtrack point did it, and therefore exactly which two steps had to be
reordered. That is a far better answer than a shorter schedule:

```
   ✗ balance never goes negative                                        8.2ms
       seed: 0:1.0.3 · 47 interleavings
       race: @1  apply_debit   db.write[accounts]   src/ledger.ply:31:5
             @2  apply_debit   db.write[accounts]   src/ledger.ply:31:5
       replay: ply test --seed 0:1.0.3 --filter "balance never goes negative"
       culprit: apply_debit
```

`Failure::race: Option<Race>` is `Some` only when the search actually observed
the flip. Under `--sim once` or `--sim random` there is nothing to observe and it
is `None` — never inferred, never guessed. ADR 0004's rule holds: a field an
agent cannot act on does not go in the artifact, and a field the run did not
observe is not reported as though it had.

**Schedule minimization is not in M7.** Shrinking a choice path is not the same
problem as shrinking a value: truncating a path changes what the suffix means,
and deleting a choice renumbers every later one. The race pair is the actionable
half of what a minimizer would produce, it is exact, and it is free.

---

### 7. `E0412` under simulation

This is the subtle one, and it resolves a real tension: today `nondet effect
clock` in a `det` test is a compile error, and under simulation `clock` *is*
deterministic.

#### 7.1 The rule does not change

> **`nondet` is discharged by handling, and by nothing else.**

A `simulate` region is a handler. It installs clauses for exactly the operations
of exactly the effects it can simulate, and the `handle` rule removes those atoms
from the region's row as it would for any hand-written handler. `E0412` therefore
gains no new case: it fires iff a `nondet` atom survives into a `det` test's
footprint, which is what it has always meant.

There is no rule that says "a nondet effect is fine if it happens to be
simulated". There is no analysis that asks whether a region is "sufficiently
simulated". There is a handler, and handlers already discharge. Every temptation
to write a special case here should be read as a signal that the handler is in
the wrong place.

What actually changed is that the handler is now **supplied by the language**, so
`clock.now()` becomes testable without a user writing `clock.now() -> 0` — and
without the drift that a hand-written stub returning a constant introduces, since
the simulated clock has real ordering semantics that a constant does not.

#### 7.2 Why a simulated test's result is legitimately a function of definitions and seed

Three steps, and each is a property something enforces rather than a hope:

1. **The region contributes nothing outside itself.** The `handle` rule's
   `⋃ row(clause_i)` term is what makes a handler honest — a handler backed by a
   socket reports network access. The seeded scheduler's clauses read and write
   only the region's own state, which is created at the region's entry and
   destroyed at its exit, so that term is empty. The region's row is
   `(ρ_b \ handled) ∪ {sim.read}` and the `sim.read` is the seed, declared.

2. **Every value the handler produces is a function of the seed and the request
   sequence.** The interleaving choice is `sched` stream plus the enabled set; the
   enabled set is a function of the run so far; virtual time is a function of the
   sleeps requested; a draw is the `rand` stream. By induction over scheduling
   points, the whole run is a function of `(definition set, seed)`.

3. **Nothing else can enter,** because §4.3's hygiene rules are enforced and
   §10's replay tests pin them. The induction in step 2 is only as good as the
   base case, and the base case is a discipline about hash maps and clocks in one
   Rust module.

The `det` test that results is cacheable for exactly the reason any `det` test
is: re-running it cannot reveal anything new, because its inputs have not changed.
Its inputs are now two rather than one, and §8 keys the cache on both.

#### 7.3 What is still nondeterministic, exactly

An effect is nondeterministic in a test iff **no handler in that test discharges
its atoms**. Precisely:

- A `nondet` effect the language does not simulate — anything a user declared —
  is untouched by `simulate` and still `E0412`. Required test.
- A `nondet` effect the language simulates, performed **outside** any `simulate`
  region, is still `E0412`.
- `sim.read` surviving into a test's footprint is **not** an error, because `sim`
  is not `nondet` — but it *is* what makes the test a seeded test, and §8 refuses
  to cache it under its bare hash.
- A user handler that answers `sim.seed()` with a constant closes `sim.read` out
  of the row. That is legitimate and useful: it pins one known-interesting seed
  as an ordinary regression test whose outcome is a function of the definition
  set alone, and it caches under the bare test hash like any other test. The
  mechanism explains itself, which is the sign that the atom is in the right
  place.

#### 7.4 What is actually weakened, said plainly

Before M7, a `det` test could not depend on time, order or randomness at all, and
DESIGN.md's fourth row — *nondeterminism is in the type; a flaky test fails to
compile* — was true without qualification.

After M7 it needs one:

> A test that depends on time, order or randomness no longer fails to compile. It
> becomes a test **over a seed set**, and a green run is a claim about the seeds
> that were actually run.

**The residual risk is real: `ply test` can now go green on a program that a
different seed would have failed.** That was previously impossible for a `det`
test. Four things make it a trade worth making rather than a hole:

1. The risk is **visible**. `simulated: 3 of 47 · 61 interleavings · 3
   exhaustive` is printed on every run and carried in `--json`. Wall-clock
   flakiness was never visible until run 400.
2. The risk is **countable**, and often **zero**. An exhaustive search is a proof
   over all interleavings, and small concurrent tests — which is most of them —
   are exhaustive in a few dozen interleavings.
3. The risk is **addressable in one flag**. Widening the search is
   `--sim-budget`, and §8 makes widening cheap rather than a full re-run of the
   corpus.
4. The alternative is not safety. The alternative is that concurrent and
   time-dependent code is written anyway, tested by `test/nondet` which is never
   cached and never selected against, or not tested at all. The property being
   weakened protected the language from a class of program rather than protecting
   the user from a class of bug.

The honest summary: M7 does not eliminate flakiness in time-dependent code. It
converts it from an unbounded, invisible risk into a bounded, reported,
reproducible one, and it makes the bound a number a project can raise.

---

### 8. Caching

A simulated test's outcome depends on the definition set **and** on the search
that was performed. Getting this wrong in one direction re-runs everything
forever; in the other it caches a pass that a different seed would have failed.

#### 8.1 The plan is part of the key

```rust
pub struct Plan {
    pub mode: SimMode,     // Once | Random | Dpor
    pub roots: Vec<u64>,   // ascending, deduplicated
    pub budget: u32,       // interleavings per root
    pub steps: u32,        // scheduling steps per interleaving
}
```

```
sim_key(test_hash, plan) =
    blake3( b"ply.sim.key.1" ‖ test_hash ‖ mode_u8 ‖ budget_le_u32
          ‖ steps_le_u32 ‖ roots_len_le_u32 ‖ roots_le_u64* )
```

- A test whose footprint does **not** contain `sim.read` is keyed by `test_hash`,
  exactly as today. Nothing about the existing cache changes for it.
- A test whose footprint **does** contain `sim.read` is keyed by
  `sim_key(test_hash, plan)` and is **never written under its bare `test_hash`**.
  That is the rule that stops a run with one plan from reading a pass another
  plan earned.

The key is a `DefHash`, so `Store` needs no new shape — the same trick ADR 0003's
`blake3(component_hash ‖ index)` already uses, with a domain tag so the two
namespaces cannot collide.

#### 8.2 Two modes, two claims, and only one of them decomposes

- Under **`random`**, interleavings are independent, so a per-seed key is a true
  standalone claim. The run additionally writes
  `blake3(b"ply.sim.seed.1" ‖ test_hash ‖ seed_bytes)` per root, and a *widened*
  plan runs only the roots whose per-seed key is absent. Sixty-four roots become
  a hundred and twenty-eight for the cost of sixty-four runs.
- Under **`dpor`**, a root's exploration is not decomposable: the interleavings it
  visits depend on what the earlier ones observed, so "seed *s* passed" is not a
  fact that survives being lifted out of its search. Only the plan key is
  written, and widening the budget re-runs the root.

Two rules rather than one because they are two different claims, and conflating
them is precisely the failure the milestone must not ship.

#### 8.3 What must never be written

- Never a `Pass` under the bare `test_hash` for a seeded test.
- Never a per-seed key under `dpor`.
- Never a `Pass` for a plan that spent its budget without emptying its frontier,
  under either mode — an exhausted search proved nothing about the interleavings
  it did not reach. `exhausted: true` means the run is reported green and **not
  cached**, and the summary says so. This is the one place where a green run
  re-runs next time, and it is correct that it does.
- A failing test is not cached, unchanged.

#### 8.4 Bisection runs at the failing seed

M5's hybrids must pin the seed of the failure they are attributing:
`BodyHybrid` runs at `Plan { mode: Once, roots: [failing_seed.root], .. }` with
the failing path. A hybrid that explores its own interleavings is answering a
different question from the one the search asked, and a bisection over it names
whichever definition the *other* interleaving happened to run through.

This is a required test, and it is the kind of defect that produces confidently
wrong culprits rather than obvious breakage.

---

### 9. Diagnostics

Four new codes. `ply_span::codes` is append-only and existing numbers do not
move.

| code | constant | when | whose fault |
| --- | --- | --- | --- |
| E0413 | `TASK_ESCAPES_SCOPE` | a `Task<_>` in a `simulate` region's result type (static); a `join` of a task whose region has ended; a region abandoned with tasks unfinished; a continuation re-entering a region that has ended (§1.6) | the program's |
| E0414 | `DEADLOCK` | no task is enabled and no timer can fire, **or** the per-interleaving step budget was spent | the program's |
| E0415 | `SIMULATION_DIVERGENCE` | replaying a seed did not reproduce the recorded enabled sets | **Ply's** |
| E0416 | `NESTED_SIMULATION` | `sim.read` is in a `simulate` body's row | the program's |

`E0414` deliberately covers both deadlock and livelock under one code with two
messages. From the program's side both are "this stopped making progress", the
fix is in the same place, and an agent matching on a code wants that class rather
than that distinction. The message names the blocked tasks and what each waits
on; the note distinguishes the two.

E0413, E0414 and E0416 join ADR 0005's `E0501`/`E0502` row: the program is at
fault, `Failure::defect` is `false`, and they are attributed and bisected like
any other failure. `AssertionKind` gains `Deadlock` so M5's classifier keeps
partitioning the space exhaustively.

E0415 joins `E0503`'s row: Ply's fault, `Status::Panicked`, `Skipped::Panicked`,
no bisection. Same reasoning — the run knows the two answers disagree and nothing
in the definition graph decides which one the program meant.

**`simulate` is machine-only.** The tree-walker cannot capture a continuation, so
it refuses a `simulate` region with `E0504` exactly as it refuses a `resume`
clause, and `ply_eval::machine_only_clauses` learns to scan for it. Under
`--engine both` such a test runs once, on the machine, and `--explain` records it
as `machine-only`. No new mechanism.

---

### 10. Validating it

The property being validated is that a run is a function of its inputs, and the
way that property breaks is never loudly.

- **Same seed, twice in one process**: identical outcome, identical recorded step
  sequence, identical final world. Comparing the outcome alone would pass on a
  run whose interleaving differed and whose assertions happened not to notice.
- **Same seed, `--jobs 1` and `--jobs 16`**: byte-identical `--json`. This is the
  test that catches a scheduler decision reading anything that varies with thread
  count.
- **Same seed, two different orders of the corpus** (`--filter` narrowing to one
  test versus running the whole suite): byte-identical artifact for that test.
- **The hygiene grep** over `ply-eval::sim`, per §4.3.
- **`--sim-budget 1` and `--sim-budget 256` deliver the same value and world**
  for a passing program, per §2.4.

**The failing half of the demo is `tests/fixtures/bank_race.ply`, not
`examples/`.** `examples/bank.ply` asserts only what survives every interleaving
— its header says so — because a cold `ply test examples/` passing is a headline
invariant and a deliberately red file in `examples/` would break it. The two
files are the same program with two different assertions: `bank.ply` asserts
conservation, which holds under every schedule, and `bank_race.ply` asserts
non-negativity, which does not. So `ply test examples/bank.ply --seeds 200` is
green by design and reports `3 exhaustive`, which is the *other* half of the
demo — the proof — and `ply test tests/fixtures/bank_race.ply` is the failure
that reports `seed: 0:0.1.0.2` and the race pair. A demo script that expects
`examples/` to go red is reading the wrong file.

`RUNTIME_VERSION` bumps to `0.5.0`: the evaluator gains semantics, and a cached
`Pass` is a claim about what the evaluator did. `FRONTEND_VERSION` and
`BODY_ENCODING` bump because `Simulate` enters the AST and the normalizer.

## Consequences

- **No headline invariant moves.** Renaming a function still selects zero tests;
  moving a definition still changes no hash; incremental and `--no-incremental`
  still agree byte for byte; a `nondet` atom in a `det` test is still E0412;
  bisection still names the culprit; `--engine both` still reports no divergence;
  `Store::open` is untouched. M7 adds an input to a test, not a change to what a
  definition is.
- **DESIGN.md §4's determinism paragraph needs a qualification** and gets one:
  a `det` test may now carry `sim.read`, and its cache entry is keyed on the
  plan. §7.4 is the honest statement and it belongs in DESIGN.md, not only here.
- **`Isolation` widens.** `sim.read` is a read of an input no test can write, so
  it must not make a test `Shared` — otherwise every simulated test drops out of
  the `isolated: n of m` number for no reason. `world_isolated` becomes "no atom
  contends", where an atom contends unless it is world-backed or ambient, and
  `sim` is the one ambient effect.
- **The world is still monotone and still never snapshotted.** Whole-test replay
  is what buys that (§2.4), and it costs re-doing a test's setup per
  interleaving.
- **A green run can now be non-cacheable.** An exhausted DPOR search reports green
  and writes nothing, so it re-runs next time. That is new — every previous green
  `det` test was cacheable — and it is correct.
- **`ply-eval` gains `blake3`.** Already a workspace dependency at 1.8.5; the
  seed's streams are counter-mode BLAKE3 so that "the same seed anywhere" does
  not depend on a generator crate's version.
- **Two evaluators, one of which cannot run the feature.** `simulate` is
  machine-only from the day it lands, so the `--engine both` audit covers strictly
  less of the corpus than it did. That is an argument for finishing M6's deletion
  of the tree-walker, not against `simulate`.

## Required tests

Concurrency and structure:

1. Two tasks incrementing one cell-backed counter through a handler: some
   interleaving loses an update, and the search finds it.
2. A `det` test spawning a task with no scheduler is `E0412` naming `task.write`.
3. A user-written sequential `task` handler discharges `task`, the test is `det`,
   and it caches under its bare `test_hash`.
4. A `Task` in a `simulate` region's result type is `E0413`.
5. Joining a task through a continuation resumed after its region ended is
   `E0413` at run time, not a wrong answer.
6. A task still runnable when the region's body returns is run to completion
   before the region delivers its value.
7. A region where every task blocks on a join cycle is `E0414`, naming both
   tasks and what each waits on.

Seeds and replay:

8. The same seed run twice in one process produces identical outcome, identical
   step sequence and identical final world.
9. `--jobs 1` and `--jobs 16` produce byte-identical `--json` at one seed.
10. `Seed::parse` round-trips `7` and `7:3.0.2`, and rejects everything else.
11. Adding a `random.next()` call to a program does not change the interleaving
    chosen at any scheduling point that precedes it — the two streams are
    independent.
12. `random.below(n)` is unbiased by the specified rejection rule, and
    `n <= 0` is `RUNTIME_ERROR`.
13. `ply-eval::sim` names no hash-based collection.

The clock:

14. `clock.sleep(30_000_000_000)` costs no measurable wall clock and advances
    virtual time by exactly that much.
15. Virtual time does not advance while any task is enabled.
16. Two tasks sleeping to the same deadline both become enabled at it, and their
    order is explored.
17. A timeout does not fire while another task can still run.

Exploration:

18. Two tasks touching disjoint resources produce **one** explored interleaving,
    and the naive count for the same program is greater than one.
19. Two tasks writing one resource produce both orders.
20. A read/read pair produces one interleaving; a read/write pair produces two.
21. Two tasks writing two different `CellId`s that share a `[r]` label do **not**
    conflict — the relation is at cell granularity.
22. A `simulate` region reusing `ply_test::shared_footprint` fails a test written
    for exactly that mistake: `cell` accesses must be in the relation.
23. `--measure-reduction` reports an exact naive count on a fixture small enough
    to enumerate by hand, and a `>=` bound when the naive budget is spent.
24. A search that empties its frontier reports `exhaustive: true`; one that
    spends its budget reports `exhaustive: false` and `exhausted: true`.
25. A replay whose enabled set does not match the recording is `E0415`, and is
    classified as a defect rather than bisected.

Typing:

26. `simulate` inside `simulate`, lexically and through a call, is both `E0416`.
27. A user's own `nondet effect` inside a `simulate` region is still `E0412`.
28. `clock.now()` outside any region in a `det` test is still `E0412`.
29. A handler answering `sim.seed()` closes `sim.read` out of the row and the
    test caches under its bare hash.
30. Spawning a task that performs `db.write[orders]` puts `db.write[orders]` in
    the spawner's row, and therefore in the test's footprint and the cross-test
    conflict graph.
31. `sim.read` does not make a test `Shared`: adding N simulated,
    otherwise-isolated tests changes the group count by zero.

Caching:

32. A seeded test is never written under its bare `test_hash`.
33. Changing `--sim-budget` changes the cache key and re-runs; changing nothing
    re-runs nothing.
34. Under `random`, widening the roots from 64 to 128 runs 64 tests, not 128.
35. Under `dpor`, an exhausted search writes no `Pass` and re-runs next time.
36. A bisection over a simulated failure runs every hybrid at the failing seed.

Integration:

37. `simulate` under `--engine treewalk` is `E0504`, naming the region, and does
    not evaluate.
38. Under `--engine both`, a test containing `simulate` runs once, on the
    machine, and `--explain` records it as `machine-only`.
39. Adding, removing or reordering a `simulate` region changes the enclosing
    definition's hash; reformatting it does not.
40. `--sim-budget 1` and `--sim-budget 256` deliver the same value and final
    world for a passing program.

## Alternatives considered

**`spawn` as syntax rather than an effect operation.** A `spawn { .. }` block
would need no op polymorphism and no prelude effect. It also makes the scheduler
unswappable: there is no signature to write a second handler against, so the
production scheduler and the simulated one are two implementations of a shape
that exists only in prose, which is exactly the drift DESIGN.md §2 exists to
prevent. It would also put concurrency outside the effect row, so a function that
spawns would not say so in its type and the cross-test conflict graph would not
see the spawned body's effects.

**Free spawn, with an explicit nursery construct for those who want structure.**
More expressive and it is what most languages ship. Rejected because every one of
§1.3's three benefits is lost: the scheduler's state escapes its region, the set
of live tasks is unbounded so the enabled set is not computable at every
scheduling point, and "no progress is possible" stops being decidable — a hang
becomes a hang rather than `E0414`. Structured is also the easier thing to
un-restrict later; free is not restrictable later.

**General polymorphic operation signatures, with surface syntax.** The principled
version of §1.4, and it is where this ends up. Deferred because `task` is the only
client today and a general feature designed against one example is a feature
designed twice. `OpInfo::scheme: Option<Scheme>` is the same capability with the
grammar left out, and the grammar is the part that is cheap to add later.

**A `u64` seed, with DPOR's alternatives named some other way.** Honors "the seed
is the repro artifact" more literally, at the cost of a second artifact — a
choice-sequence — that a consumer has to carry alongside the seed. `Seed { root,
path }` with a canonical `7:3.0.2` text form is one artifact, one flag, one JSON
field, and it still prints as a bare number in the sampling case.

**Snapshot the world at region entry and restore it per interleaving.** Cheaper
than whole-test replay: the setup preceding the region is not re-done. It is also
the world snapshot/restore builtin ADR 0005 refused as a capability with no
type-level account, and restoring un-does writes the row still reports. Whole-test
replay gets the same reduction with no new semantics, and pays in wall clock,
which is the currency this project has decided to spend.

**Interleaving at every evaluator step rather than at effect boundaries.** Finds
more interleavings, all of them redundant: §3.3 is the proof. It would also make
the state space depend on the shape of the interpreter rather than on the shape
of the program, so the reduction number would measure the machine and not the
language.

**Approximating the dependence relation statically, from the declared rows of the
spawned bodies.** Available before the run, so backtrack points could be
precomputed. Rejected because it is strictly coarser than what the tracer already
observes, and coarser means more interleavings explored for the same coverage —
the opposite of the milestone's point. Dynamic POR exists precisely because the
static relation is worse, and Ply's dynamic relation is *exact* rather than an
approximation, which is the part worth demonstrating.

**Per-task RNG streams derived from `(seed, task_id)`.** Makes draws
order-independent, which makes replay easier and shrinks the state space. It also
*hides* a real order dependence that production code with a shared generator has,
and the whole argument for the simulated handler is that it does not get to be
kinder than the real one. A user who wants per-task streams can write the handler
that splits, and their type will say they did.

**Jitter on the virtual clock, drawn from the seed.** One more dimension of
search. It costs §5.2 — a timeout that can fire while work is pending is a
timeout that can fire spuriously, and the exactness of a simulated timeout is
worth more than the extra schedules, which the scheduler's own choices already
cover.

## Not in M7

- **Real threads.** The simulated handler runs on one thread and always will.
  `rayon` stays where it is, at the test-runner level, scheduling whole tests.
- **A real network.** ROADMAP.md's M7 line says "network"; M7 delivers the
  mechanism — an effect with a simulated handler — and no network effect. A
  socket has partitions, reordering, duplication and partial writes, each of
  which is a modelling decision, and inventing them under a scheduler rewrite is
  two designs in one change. **This is the largest gap in the milestone.**

  **No longer a gap, and not closed the way this bullet imagined.** W1 shipped
  `net` as a *host* effect behind ADR 0008's boundary (`ply_host::tcp`), and W3
  built HTTP/1.1 framing on top of it in Ply. So the network arrived as a host
  binding rather than as a simulated effect, and the modelling decisions this
  bullet listed were never taken: there is still no simulated socket, and ADR
  0006 §7.3's rule — the language does not get to claim it simulated something
  it has never heard of — is why. A test that reaches the real network is
  refused inside a re-run search by `E0427`/`E0425` (ADR 0011 §7) instead.
- **Finding races in Rust code.** Nothing here inspects the interpreter's own
  threading. The races found are races between Ply tasks over Ply resources.
- **Cancellation, timeouts as a primitive, channels, mutexes.** Cells plus
  `spawn` / `join` / `yield` is the primitive set; the rest is a library, and a
  library written in Ply is one whose handlers the effect system can see.
- **Sleep sets, and any DPOR refinement beyond backtrack sets.** Optimizations
  over a search that is already sound.
- **Schedule minimization.** §6.5: the race pair is the actionable half and it is
  exact.
- **Surface syntax for polymorphic operation signatures.** §1.4.
- **Simulating a user-declared `nondet` effect.** There is no way to ask the
  language to simulate `http`, and §7.3 is why that is the safety property rather
  than a missing feature. A user simulates their own effect by writing a handler,
  which is what handlers are.
- **Reclaiming world entries.** Still unsound for the reasons ADR 0005 gives, and
  now more visible: whole-test replay means a long exploration allocates a
  region's cells once per interleaving.

  **Superseded by ADR 0017.** There are no world entries to reclaim: the world
  is gone and a cell is an arena `Slot`. ADR 0005's "every cheap rule is
  unsound" argument was about a *monotone* map with no ownership information;
  ADR 0017 §3 supplies the missing information — a region is `unique` when the
  compiler can prove no continuation is captured across it, and `shared`
  otherwise, with `shared` slots reference-counted rather than retained forever.
  Reclamation is therefore decided at a region's lexical close
  (`TaskRegions::close_region`, `Reclaim`) rather than deferred to a tracing
  collector. What ADR 0005 got right is that no rule *keyed on the region alone*
  is sound, and that is why the kind is inferred over the whole program
  (`ply_eval::region_kind`) rather than read off the syntax.
