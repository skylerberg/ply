# ADR 0006 — Deterministic simulation

**Accepted, implemented.** Builds on ADR 0005, whose threaded state and explicit
control stack are the two things this is impossible without.

## Context

**Ply had no concurrency primitive.** Tests ran concurrently at the runner
level, but a Ply program could not spawn anything. There was nothing to
interleave. So the first thing to do is introduce concurrency, and *how* decides
whether the rest is a language feature or a bolted-on debugger.

The answer the language already implies: **concurrency is an effect.** A test
double must satisfy the same declared signature as the real resource, because
that is what stops the two drifting. Apply the argument to the scheduler itself.
`task.spawn` is an operation with one declared signature; in production a
handler runs tasks on threads, in simulation a seeded handler interleaves them
deterministically, and neither can drift because the signature is written once.
**A scheduler is a test double for the operating system.**

That framing is also why the control stack is the prerequisite rather than a
coincidence of ordering. A task is a suspended machine state, and the threaded
state of ADR 0005 — a resumption observes state as of the handler's call to
`resume`, never as of the capture — is *precisely* the semantics of shared
memory. Had ADR 0005 chosen snapshot-at-capture, two tasks would not see each
other's writes and this milestone would be unimplementable.

## The rule everything else follows from

> **A simulated run is a pure function of its definition set and its seed.**
>
> Every source of nondeterminism a Ply program can reach is an effect, and
> simulation is a handler for it. Nothing else enters: no wall clock, no thread
> identity, no address, no iteration order that is not itself specified.

## Concurrency is an effect

Four effects are declared by the language rather than by a module: `task`
(spawn, join, yield), `clock` (now, sleep), `random` (next, below), and `sim`
(seed). Each is a singleton resource — one scheduler, one clock, one random
stream per simulated region — so a resource label would name a distinction that
does not exist.

The mode annotations are load-bearing. `clock.now` is a **read** because it
observes virtual time without moving it; `clock.sleep` is a **write** because it
changes when this task is next runnable, which changes what `now()` answers
elsewhere. **`random.next` being a write is the sharpest of these**: drawing
*advances the stream*, so two tasks drawing in the other order get the other
values, so two tasks that both draw conflict, so their order is a real
difference and gets explored rather than pruned. A design that declared drawing
a read would have quietly hidden a whole class of order dependence. **The effect
system asked the right question and the honest answer prunes less.**

**`task` is `nondet`, and that is the thesis restated.** Concurrency without a
specified scheduler *is* nondeterminism, so the type says so: a deterministic
test that spawns a task and installs no scheduler fails to compile. *A
deterministic test may not contain unscheduled concurrency* — the first time the
language's central claim has had something to say about the largest real source
of flakes.

## Structured, and what structure costs

**Spawn is structured, and structure is not a new construct: the handler is the
scope.** A `simulate` region delimits the scheduler exactly as `handle` delimits
any other handler. When the body returns, the scheduler drains whatever is still
runnable before delivering the value; no task, no timer and no pending draw
survives the closing brace.

What it costs, stated rather than glossed: **no daemon tasks** — a task that
never terminates deadlocks the region rather than being abandoned at the
boundary, which in a simulation is right, because an abandoned task is an
unexplored interleaving, but is a real restriction on what production code the
model can mirror; **no task escaping in a value**, since a `Task` is a key into
scheduler state that dies with the region; and **no cancellation**, deferred
rather than approximated.

What it buys, which is why it wins: the scheduler's state is region-local, so it
contributes nothing to any escaping row; the set of live tasks is finite and
known at every scheduling point, which is what makes an *enabled set* computable
and therefore what makes the search finite-branching at all; and "every task is
blocked" becomes decidable, which is what turns a hang into a diagnostic. **Free
spawn gives up all three, and gives up the last one permanently.**

**The one type-system extension is a polymorphic scheme on a prelude
operation, and nothing else.** `spawn` needs type polymorphism and an effect row
*on the operation itself*, so the effects of a spawned body appear in the row of
the code that spawned it. That second half is not a convenience: without it a
test that spawns a task writing `db.write[orders]` would report an empty
footprint, and the cross-test conflict graph would run it beside a test reading
`orders`. The spawned body's effects *do* happen, inside the region, so the row
must say so. User-declared operations stay monomorphic, and surface syntax for
declaring a polymorphic operation is deliberately not added: `task` is the only
client, and a general feature designed against one example is a feature designed
twice.

**A user may still handle `task`** with no declaration at all — a sequential
scheduler is eight lines of ordinary Ply. Three handlers for one signature, and
no way for them to drift.

## Control that crosses the region's delimiter

A `handle` written *outside* a region may answer an effect performed *inside*
one, and the capture then crosses the delimiter. That is ordinary and useful —
it is how a task's own effects are discharged by a test double the test
installed. Three cases, and only the first is quiet:

- **Resumed once.** The region carries on, and its **anchor moves**: the stack
  it eventually delivers onto is the one the splice put it over, not the one it
  was entered on. Restoring the entry stack instead silently discards whatever
  the resuming clause still had pending — a wrong answer with no diagnostic, and
  an exhaustiveness claim made over a program the machine did not run correctly.
- **Never resumed.** The region is abandoned with tasks still runnable. Every
  step past the abandonment is missing from the recording, so the search's
  completeness precondition is violated and it would report `exhaustive` over
  schedules it cut short. A run that ends with a region still live is an error.
- **Resumed twice.** The second resumption re-enters a region that has already
  delivered its value; forking a live scheduler needs the state snapshot ADR
  0005 refused. An error rather than the wrong answer.

## The seed

A seed is a root plus a **choice-sequence prefix**: at scheduling point *i*, if
the prefix has an entry, resume that enabled task; otherwise draw. Canonical
text form `7` or `7:3.0.2`.

**Why a seed is not just an integer.** A systematic search needs to say "the
interleaving that is like this one but takes the other branch at point 3", and
in general no integer produces exactly that. The choice is between one artifact
whose grammar has two fields and two artifacts a consumer must carry together.
One artifact wins, and the common case — a randomly sampled failure — still
prints as a bare number.

Two independent streams are derived from the root by domain separation, in
counter-mode BLAKE3. **They must not share a counter**, or adding a
`random.next()` call would shift the interleaving: a change to the *data* would
silently become a change to the *schedule*, and a bisection over it would name
the wrong definition.

Counter-mode BLAKE3 rather than a PRNG crate, because "the same seed produces
the same result on any machine" is a cross-version promise and a dependency's
generator is not. Range reduction is rejection sampling, specified exactly,
because "unbiased" is not a specification.

**There is no clock stream.** Virtual time is not drawn; it advances by a rule
that is a function of the sleeps requested and therefore of the seed. Jitter
would buy a dimension of search at the price of the exact-timeout property,
which is worth more.

**Hygiene is normative, not advice.** No hash-based collection may be *named*
anywhere in the simulation module — not "no hash map iteration", because a rule
about how a type is used is a rule nobody enforces. No wall clock, no thread
identity, no work-stealing, no external RNG. No pointer value, address, refcount
or allocation order may be observed by any decision. Task ids come from the
region's own counter. Requirement one is enforced by a test that greps the
module's sources, which is blunt and is the kind of check that actually catches
the regression six months later.

## The virtual clock

> **Virtual time advances at exactly one moment: when no task is enabled and at
> least one is blocked on a timer. It jumps to the earliest deadline, and every
> task with that deadline becomes enabled at once.**

Three properties follow, and each is something a real test suite wants. A run's
virtual duration is a function of its sleeps, not of the machine, so a loaded CI
box and an idle laptop agree. A sleeping test costs no wall clock, so
retry-with-backoff logic becomes testable at its real timings. And tasks that
wake together race, and that race is explored — the timer-coalescing bug that is
nearly impossible to hit on a real clock.

**A simulated timeout never fires early.** It cannot pre-empt work that could
still run, because time does not move while anything is runnable. That is the
exact opposite of a wall-clock timeout, whose whole failure mode is firing
because the machine was busy.

**Virtual time does not advance for computation**, so a run in which every task
is CPU-bound takes zero virtual nanoseconds. A simulated test cannot detect that
an implementation got slower and must not be read as a performance test.
Conflating the two would produce a benchmark that is deterministic and
meaningless.

## The scheduler runs on the control stack

The seeded scheduler is a **native prompt**: a delimiter on the machine's stack
whose clauses are Rust. A task is a continuation. When a task performs a
scheduler-visible operation, the machine captures up to that delimiter exactly
as it would for a Ply clause, hands the continuation to the scheduler, and the
scheduler decides who runs next. **Every mechanism it needs — capture, splice,
deep handlers, one threaded state — was landed for multi-shot continuations and
none of it was designed for this.**

A **step** runs from a resumption to the next scheduler-visible perform or to
completion; a **scheduling point** is the boundary between two, and the
scheduler's choice there is the only choice a simulated run makes.

**Enabledness — not the dependence relation — is how synchronization is
represented.** An implementer who encodes "join happens after the child
finishes" as a conflict gets a search that explores impossible schedules and
then prunes them; the enabled set makes them ungenerated. The two mechanisms
answer different questions and the reduction depends on keeping them apart:
enabledness says what *could* run, dependence says whether the order *matters*.

**One choice sequence per entry point, not per region.** Nesting is refused, but
*sequence* is not and cannot be: a test may write two regions one after another,
and an ordinary function whose body is a region reaches one twice with no syntax
pointing at it. Both consequences of getting this wrong are silent. A record
covering one region gives the search a trace describing that region alone, so
the other regions' choice points are never branched on and the run is still
reported exhaustive — the worst artifact this design can produce. And with a
per-region counter, a path entry names the first choice of *every* region, so a
backtrack point aimed at one silently re-aims the others.

### What the granularity costs

**A scheduler cannot suspend a task at a perform it does not answer.** Its only
power is to decide who runs next at a point where control has already reached
it, and control reaches it exactly when a task performs one of the three
simulated effects.

> `exhaustive: true` means every interleaving **at scheduler-visible
> granularity** has been executed.

So a task that reads shared state and writes it back with no simulated perform
in between runs the two as *one step*, and the classic lost update is **not**
found unless something in the window is scheduler-visible. Three ways to make
such a window explorable, in the order to reach for them: a `yield` between the
read and the write, which is what production code's real preemption point
corresponds to; a clock read, which real code writes anyway; or **push the check
into the resource**, which is the fix rather than the test — an operation that
decides and debits at once cannot be separated by any schedule, because there is
nothing to separate.

**Why not interleave at every perform.** It is not free and it is not obviously
more honest. Every user perform becoming a scheduling point means suspending a
task at an operation the scheduler does not answer and re-issuing it on
resumption; cell operations are builtins rather than performs, so shared state
would still not be covered without making every builtin a scheduling point too;
and the state space grows by a factor of the perform count per task, which turns
`exhaustive` from the common case into the rare one. **The rule that makes the
headline true — `exhaustive` is a proof — is worth more than a wider model whose
searches never finish.**

## Footprint-guided exploration

> Two steps are **dependent** iff their access sets conflict. Two steps that are
> not dependent commute, so exploring both orders is redundant, and a scheduler
> that explores both is doing work it can prove is useless.

Partial-order reduction algorithms spend most of their complexity
*approximating* this relation from an alias analysis. **Ply computes it exactly,
at resource granularity, and has been computing it since footprints existed** —
it is the same predicate that decides which tests may run concurrently. The
claim that resource granularity is "exactly the information needed to decide
whether two tests can run concurrently" turns out to have been one instance of a
more general fact.

A step's access set is finer than a footprint in exactly one place. Two cells
conflict iff they name the **same cell** and one is a write — cell granularity
rather than the resource label, because a cell is a location and the label is a
name several locations may share. And **allocation always conflicts with
allocation**, which is the case the soundness condition rules on and the type
system cannot: allocation has no location to name, so two tasks that each open a
private cell look like tasks that touch nothing, and run in the other order they
reach a *different* state because the two ids are swapped. "Not dependent ⟹
reaches the same state" is false of them. No surface construct observes a cell
id today, so this cannot yet flip an assertion — but **a relation that is right
only until someone can observe it is not a relation, it is a coincidence.**

**The correction that would otherwise be a silent unsoundness.** The test
scheduler exempts cell atoms from its conflict graph, because two tests hold two
separate states. That exemption does not survive being applied to two *tasks*:
**two tests hold two states; two tasks in one simulated run hold one.** A build
that reuses the test scheduler's footprint here prunes away every shared-memory
race in the corpus while reporting a larger reduction for having done it. **That
is the single most expensive mistake available here, because its symptom is a
*better* number.**

**Why the terminating scheduler op is excluded from the access set.** Every step
ends in a simulated perform and all of those are writes to a singleton resource;
included, every pair of steps would be dependent and the reduction would be
exactly 1×. The exclusion is sound because the scheduler is the *explorer*: its
state is a function of the choice sequence, and the search enumerates choice
sequences. **`random.write` is the exception that proves the rule and must not
be excluded**: the value a draw returns is observed by the *program*, not by the
scheduler, so a draw is a genuine read-modify-write of shared state.

### The search

Dynamic partial-order reduction in the backtrack-set formulation, with the exact
dependence relation substituted for the alias analysis the literature has to
approximate.

**A dependent pair is not yet a race.** The naive rule asks whether two steps
conflict and whether the later task could have run earlier; it does not ask
whether the two could ever have run in the other order, and on the ordinary
shape of a concurrent test — spawn, join, then assert on what the children wrote
— the answer is usually no, because the join already ordered every child step
before every assertion. Queueing those pairs is not conservative, it is wasted,
and it was measured at hundreds of interleavings where one was correct. So the
scheduler carries a **vector clock per task**, advanced per step, inherited at
spawn and merged at join, and a dependent pair whose earlier step happens-before
the later is not a backtrack point. Those are the region's only two
synchronization edges; a timer waking a task adds none, because time advancing
is the scheduler's decision rather than the program's, so two tasks that wake
together are racing. This is a filter over reachability and not a second
dependence relation: it cannot hide a race, since the reordering it refuses to
queue is one no schedule produces.

**Sleep sets are not built.** They are a further reduction on top of backtrack
sets and are where implementations get subtly wrong. Backtrack sets alone are
sound and produce the reduction claimed.

**Replay is self-checking.** Re-running a prefix must reproduce the same enabled
set at every choice point it names. A mismatch means the run was not a function
of the seed — Ply's fault, not the program's, and the same class of defect as an
engine divergence.

**The reduction is measured, not asserted.** One flag re-runs the same search
with the dependence relation forced to `true`, which degenerates the search into
exhaustive enumeration of every schedule — exactly the naive scheduler the
reduction is claimed against, from the same code, with no second implementation
to disagree with the first. It is off by default; the claim is a benchmark, not
something every run should pay double for. **The honest shape of the result: the
reduction is large exactly where the resource labels say the work is
independent, and it is 1× where they say it is not.**

**The three modes are not ordered, and measurement says so.** A systematic
search enumerates equivalence classes from one end; a sample jumps around. On a
race that lives in most of the space they are level; on one that lives in a
corner, sampling wins outright. `random` is what to reach for when the goal is
to find a bug in a space too large to exhaust; `dpor` is what to reach for when
the goal is to prove there is none.

**The value a region delivers does not depend on the budget.** The value and the
state a region delivers are those of the interleaving its seed names; every
other interleaving explored is a search and its state is discarded. Without that
rule, raising the budget would change what a program *means*. Exploration is
therefore whole-test replay: the test is re-run per interleaving from a fresh
state. Re-running only the *region* would need state restored per interleaving,
which is the snapshot/restore capability ADR 0005 refused; whole-test replay
needs no such capability, because a test is re-run so its writes are re-done
rather than un-done. It costs re-doing whatever setup precedes the region.
**That is the price of not putting an un-do into a language whose rows report
every do.**

**When the search flips a passing interleaving to a failing one it knows which
backtrack point did it**, and therefore exactly which two steps had to be
reordered. That pair is a better answer than a shorter schedule, it is exact,
and it is free. It is reported only when the search actually observed the flip —
never inferred, never guessed. Schedule minimization is not built: truncating a
choice path changes what the suffix means, and deleting a choice renumbers every
later one; the race pair is the actionable half.

## `nondet` under simulation

> **`nondet` is discharged by handling, and by nothing else.**

A `simulate` region is a handler. It installs clauses for exactly the operations
of exactly the effects it can simulate, and the `handle` rule removes those atoms
as it would for any hand-written handler. There is no rule that says "a nondet
effect is fine if it happens to be simulated", and no analysis that asks whether
a region is *sufficiently* simulated. **Every temptation to write a special case
here should be read as a signal that the handler is in the wrong place.**

What changed is that the handler is now *supplied by the language*, so a clock
read becomes testable without a user writing a stub returning a constant — and
without the drift a constant introduces, since the simulated clock has real
ordering semantics that a constant does not.

**`simulate` discharges the three effects it can simulate and nothing else.** A
user's own nondet effect inside a region still fails. **The language does not
get to claim it simulated an effect it has never heard of**, and that is the
safety property that survives.

The seed dependency is *in the type*, as an ordinary atom that propagates
through calls by the ordinary row rules — so a test whose closure reaches a
region carries it with no new analysis and no new field anywhere. `sim` is not
`nondet`, because a seed is an input rather than a nondeterminism, so a
deterministic test may carry it. **That is the entire type-level content of this
section.** A handler answering the seed with a constant closes the atom out of
the row, which pins one known-interesting seed as an ordinary regression test.
The mechanism explains itself, which is the sign that the atom is in the right
place.

## What is honestly weakened

Before this, a deterministic test could not depend on time, order or randomness
at all, and *nondeterminism is in the type; a flaky test fails to compile* was
true without qualification. After it, that needs one:

> A test that depends on time, order or randomness no longer fails to compile.
> It becomes a test **over a seed set**, and a green run is a claim about the
> seeds that were actually run.

**The residual risk is real: a run can now go green on a program that a
different seed would have failed.** Four things make it a trade worth making
rather than a hole. The risk is **visible** — the counts are printed on every
run, where wall-clock flakiness was never visible until run 400. It is
**countable and often zero** — an exhaustive search is a proof, and small
concurrent tests are exhaustive in a few dozen interleavings. It is
**addressable in one flag**. And **the alternative is not safety**: the
alternative is that concurrent and time-dependent code is written anyway, tested
by a never-cached nondet test, or not tested at all. *The property being
weakened protected the language from a class of program rather than protecting
the user from a class of bug.*

The honest summary: this does not eliminate flakiness in time-dependent code. It
converts it from an unbounded, invisible risk into a bounded, reported,
reproducible one, and it makes the bound a number a project can raise.

## Caching

A simulated test's outcome depends on the definition set **and** on the search
that was performed. Getting it wrong one way re-runs everything forever; the
other way caches a pass a different seed would have failed.

- A test whose footprint does not carry the seed atom is keyed as today.
- A test that does carry it is keyed by hash **and plan** — mode, roots, budget,
  steps — and is **never written under its bare hash**. That is the rule that
  stops a run with one plan reading a pass another plan earned.

**Two modes, two claims, and only one of them decomposes.** Under sampling,
interleavings are independent, so a per-seed key is a true standalone claim and
a widened plan runs only the roots it has not seen. Under a systematic search a
root's exploration is *not* decomposable — the interleavings it visits depend on
what earlier ones observed — so "seed *s* passed" is not a fact that survives
being lifted out of its search, and widening re-runs the root. Two rules rather
than one because they are two different claims, and conflating them is precisely
the failure this must not ship.

**Never a pass for a plan that spent its budget without emptying its frontier**,
under either mode: an exhausted search proved nothing about the interleavings it
did not reach. Such a run is reported green and **not cached**, and the summary
says so. This is the one place where a green run re-runs next time, and it is
correct that it does.

**Bisection runs at the failing seed.** A hybrid that explores its own
interleavings is answering a different question from the one the search asked,
and a bisection over it names whichever definition the *other* interleaving
happened to run through — a defect that produces confidently wrong culprits
rather than obvious breakage.

## Validating it

The property is that a run is a function of its inputs, and the way that
property breaks is never loudly. Same seed twice in one process: identical
outcome, identical step sequence, identical final state — comparing the outcome
alone would pass on a run whose interleaving differed and whose assertions
happened not to notice. One worker against sixteen: byte-identical output, which
is what catches a scheduler decision reading anything that varies with thread
count. Two different orders of the corpus: byte-identical for that test. Plus
the hygiene grep, and the rule that budget does not change a value.

**The failing half of the demo is a fixture, not an example.** Two files, the
same program with two different assertions: one asserts conservation, which
holds under every schedule and is the *proof* half; the other asserts
non-negativity, which does not, and is the failure that reports a seed and a
race pair. A cold `ply test examples/` passing is a headline invariant, so a
deliberately red file in `examples/` would break it.

## Alternatives rejected

**`spawn` as syntax rather than an effect operation.** It makes the scheduler
unswappable: there is no signature to write a second handler against, so the
production scheduler and the simulated one are two implementations of a shape
that exists only in prose. It would also put concurrency outside the effect row,
so a function that spawns would not say so and the conflict graph would not see
the spawned body's effects.

**Free spawn with an explicit nursery for those who want structure.** More
expressive and what most languages ship. Every one of structured's three
benefits is lost, and the last permanently: with tasks outliving their creator
there is no point at which the absence of progress is observable. Structured is
also the easier thing to un-restrict later; free is not restrictable later.

**Snapshot state at region entry and restore it per interleaving.** Cheaper than
whole-test replay, and it is the un-do capability ADR 0005 refused. Whole-test
replay gets the same reduction with no new semantics and pays in wall clock,
which is the currency this project has decided to spend.

**Interleaving at every evaluator step.** Finds more interleavings, all of them
redundant, and would make the state space depend on the shape of the interpreter
rather than of the program — so the reduction number would measure the machine
and not the language.

**Approximating the dependence relation statically.** Available before the run,
and strictly coarser than what the tracer already observes. Dynamic POR exists
precisely because the static relation is worse, and Ply's dynamic relation is
*exact*, which is the part worth demonstrating.

**Per-task RNG streams.** Makes draws order-independent and shrinks the state
space. It also *hides* a real order dependence that production code with a
shared generator has, and the whole argument for the simulated handler is that
it does not get to be kinder than the real one. A user who wants per-task
streams writes the handler that splits, and their type says they did.

**Jitter on the virtual clock.** One more dimension of search, at the cost of
the exact timeout — and a timeout that can fire while work is pending is a
timeout that can fire spuriously.

## Not done here

Real threads; a simulated network (the network arrived later as a *host* effect
instead, and the modelling decisions a socket needs — partitions, reordering,
duplication, partial writes — were never taken, which is why a test that reaches
the real network is refused inside a re-run search); finding races in the
interpreter's own threading; cancellation, channels and mutexes, which are a
library and a library written in Ply is one whose handlers the effect system can
see; sleep sets; schedule minimization; and simulating a user-declared nondet
effect, which is the safety property rather than a missing feature.
