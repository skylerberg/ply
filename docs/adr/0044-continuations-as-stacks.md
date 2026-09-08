# ADR 0044 — Continuations as stacks: the runtime the fifth step leaves

**Accepted, and the first stage is built.** ADR 0042 placed deleting the
machine fifth and ADR 0043 stopped at the line "what the runtime is once the
machine is gone". This record is that line: what a suspended computation is in
the compiled tier, how `simulate` and the three effects it answers are served
there, where a `perform` nothing on the stack answers goes, and which parts of
`crates/ply-eval` the deletion keeps. Like the two records before it, it is a
design before it is code, and each stage below names the instrument that
decides whether it landed.

**Built:** the stacks and the switch between them (`crates/ply-codegen/src/stack.rs`,
with the in-place restore tested there), the scheduler made generic over what
it suspends, and `simulate` as a frame the runtime serves
(`crates/ply-codegen/src/simulate.rs`): the body runs as the root task on a
stack of its own, every `task.spawn` gets one, a `perform` of `task`, `clock`
or `random` switches to the loop that drives the machine's scheduler, and the
footprints are recorded at the three events. The emitter written in Ply emits
the node; the reference does not. The audit pairs every seed of a simulated
test the tier took with a run of the machine alone and compares the schedules
step for step, and it is green over the examples and the standard library with
the chain entered whole, every simulated test paired. The census above loses
its first three rows and the cascade, so what the tier refuses over the
shipped corpus is the host-served performs and the credential.

> **What this decides.** That a suspended computation in the compiled tier is
> **a C stack the runtime owns**, switched to and from with the C library's
> context functions, so that a task, a `resume` bound off the tail and the
> scheduler's choice of who runs next are all the same primitive and nothing in
> an emitted function changes shape for any of them. That multi-shot resumption
> is served by **restoring a snapshot of that stack in place**, which keeps every
> pointer an emitted frame holds into itself valid, at the price of one rule:
> a continuation cannot be resumed again while an earlier resumption of it is
> still suspended on the same stack. That `simulate` is a handler frame whose
> clauses the runtime serves, driving the scheduler that exists today, moved
> whole and made generic over what it suspends, and that the interleaving
> search stays where it is and drives a compiled test through the driver
> interface it already has. That a `perform` no frame answers reaches the host
> binding through the context, parking its task on the token exactly as the
> machine's host policy does. That what the deletion removes is the CEK
> machine and everything only it needs, and that the Rust suites over it are
> re-expressed as Ply tests or deleted, which is where ADR 0042's O(change)
> loop starts to exist.
>
> **What it does not decide.** The sixth step: the Rust front end stays until
> nothing reads the syntax tree. Whether the scheduler and the explorer are
> later written in Ply; they are Rust in the runtime crate and this record
> moves them, it does not translate them. The per-definition object and link
> that ADR 0037 describes; the unit is built as it is built now.

## Where the tier stands, measured

With the chain entered whole, the tier refuses these and nothing else over the
shipped corpus. The command is the measurement; the classes are the design's
input:

```sh
PLY_C_CACHE=$(mktemp -d) PLY_C_EMITTER=ply-whole:spikes/ply-parser PLY_C_REFUSALS=1 \
  ./target/release/ply test examples --backend c --audit-backend --no-cache 2>&1 \
  | grep "c tier refused" | grep -E "task|not emit|answered by|credential"
```

| refused | why | examples | std |
| --- | --- | --- | --- |
| the tests of `bank`, `pipeline`, `timeout` | a `simulate` block, the one node the port's catch-all arm refuses | 11 | 0 |
| `pipeline.drain_*`, `pipeline.merge_*`, `timeout.grind` | a `task.*` operation, which nothing but the scheduler answers | 6 | 0 |
| `bank.settle`, `bank.transfer`, `timeout.backoff`, `timeout.deadline`, `timeout.work` | `clock#now` or `clock#sleep` under a `simulate` the tier does not hold | 5 | 0 |
| `http.serve`, `http.listen_and_serve` | `std.net.net#accept` and `#listen`, which only the host binding answers | 0 | 4 |
| `timeout.claim` | its handler is a refused test: a cascade | 1 | 0 |
| `std.config.secret_step` | a credential in the value arena, refused on purpose | 1 | 0 |

A `resume` called off the tail refuses nothing shipped; the corpus's one
non-tail clause is the zero-shot one ADR 0043 unwinds. Every class above but
the last is this record's, and the first three are one mechanism: a task is a
computation that stops at an operation and is picked up later, which is also
what a bound `resume` is.

## The semantics being carried

The guide's §10 is the contract and nothing here changes it. The three
simulated effects are declared by the language:

```ply
nondet effect task   { write spawn<a | e>(body: () -> a / e) -> Task<a> / e
                       write join<a>(t: Task<a>) -> a
                       write yield() -> Unit }
nondet effect clock  { read  now() -> Int
                       write sleep(nanos: Int) -> Unit }
nondet effect random { write next() -> Int
                       write below(bound: Int) -> Int }
effect sim           { read  seed() -> Int }
```

Tasks interleave only at the operations the scheduler answers; virtual time
advances only when no task is enabled; a region with nothing enabled and no
timer that can fire is `E0414`; a `Task<a>` may not leave its region
(`E0413`) and a `simulate` under a `simulate` is `E0416`. The first two are
the scheduler's and move with it. `E0413` and `E0416` are the checker's and do
not move at all.

The search in `explore.rs` is a partial-order reduction over **footprints**:
each scheduling step records the set of accesses the task made since the last
one, and two steps are dependent when an access of one conflicts with an
access of the other. An access is an effect atom, a cell read or write by cell
identity, or a cell allocation, and two allocations always conflict. The
search's soundness is that the footprint is *complete*: nothing a step did is
missing from it. The machine records atoms where it performs, cell accesses
where a builtin touches a cell, and the allocation where `with_cell` opens.
The runtime does the same three things in `rt_perform`, in the cell builtins
it already routes through `builtins::call`, and in `rt_cell`, so the
footprint is recorded at the same three events and nowhere else.

A `resume` bound by a clause may be called zero, one or many times (§7.7), and
`E0426` already names the one case the language forbids: resuming twice
across an at-most-once host operation.

The host boundary is `host.rs`: a binding maps a declared operation to a Rust
function, and an unbound operation reaching it in a hermetic run is `E0424`.
Under the host policy the scheduler parks a task on a pending token and gives
control to the reactor once per scheduling decision (`sched.rs`'s
`park_on_host` and `next_host`); nothing else in the language waits.

## Why a suspended computation is a stack

Three shapes can hold a computation that has stopped at a `perform` and will
continue later.

**A state machine per function** — every function that might suspend is
compiled to a struct and a step function, and every call site of it changes.
ADR 0041 priced this and declined it: it changes the calling convention of the
whole tier for a construct that is rare in the corpus, and every effectful
call pays.

**Heap frames the runtime interprets** — the continuation is a chain of frames
the runtime allocates and walks. That is a CEK machine, which is what step
five deletes; building one under the compiled tier keeps two implementations
of the semantics exactly as ADR 0042 said an interpreter written in Ply would.

**A stack the runtime owns.** Emitted C is ordinary C. The only thing that has
to change for a suspension is *which stack* the C is running on: a task body
runs on a stack of its own, and a scheduling decision is a switch from one
stack to another. Nothing emitted knows this happened. The switch is the one
place the runtime's Rust touches a stack that is not its own, and it is two
naked functions of a dozen instructions each per architecture, saving the
callee-saved registers on the stack being left and popping them on the one
being entered; a fresh stack is laid out so that its first switch returns into
a trampoline that calls the entry. The C library's context functions would do
the same and the probe below used them, but their structure has no declared
layout on this side of the foreign-function boundary, and the assembly is the
smaller thing to own.

The probe that decided it, and the cost:

```sh
# a coroutine on its own stack, suspended, snapshotted, resumed, restored, resumed again
cc -w -o /tmp/uctx spikes/ucontext/uctx.c && /tmp/uctx
# a switch there and back, and the memcpy a snapshot is
cc -O2 -w -o /tmp/uctx_bench spikes/ucontext/uctx_bench.c && /tmp/uctx_bench
```

On macOS arm64 the library's functions are deprecated and present, the probe
prints its second shot with the local the first shot saw, and a switch there
and back through them costs about a microsecond because Darwin's `swapcontext`
saves the signal mask with a system call; the CI runner is glibc, which does
the same. That is the ceiling the runtime's own switcher sits well under, with
no system call, and it is also where the decision stops mattering: a scheduling
step under the machine costs several microseconds by the same command
(`ply test examples/bank.ply`, nine interleavings in under a millisecond), and
the search's cost is in running interleavings, not in switching between tasks
inside one. The probe is kept as a spike CI runs, because the in-place restore
it shows is the claim the third stage rests on.

### The stack, and what the context keeps per stack

A stack is mapped by the runtime with a guard page below it. `Ctx.stack_floor`
is the one field that is per stack rather than per context: the prologue every
emitted function runs compares its frame against the floor before it spends
fuel, so a switch saves and restores the floor with the stack, and a runaway
recursion inside a task hits `rt_no_stack` exactly as it does at the top.
Fuel stays one budget per entry, as the machine's call bound is.

Handler frames are per stack too. A `handle` inside a task pushes onto that
task's frames; a `perform` searches the task's frames and then the frames
below the `simulate`, which are shared and are where the region's own `clock`
and `random` clauses sit. An unwind (ADR 0043's zero-shot `resume`) that
targets a frame below the region leaves the task: its C frames return through
the checks every call site already has, the task's trampoline finds the
unwind's depth is not its own, and the scheduler ends the region with the
unwind still pending, so the `simulate` site's landing continues it. That is
the machine's semantics for a `db.rollback` inside a task, and it falls out of
the failure path rather than needing one.

The trampoline is the one new piece of C-facing runtime: a task body is a
closure word, and the runtime calls it through `rt_call` on the new stack,
records its answer or its failure for `join`, and switches back to the
scheduler.

### Multi-shot by in-place restore

Capturing a continuation off the tail — a clause that binds `resume` and does
not call it last — snapshots the live range of the performer's stack, from
its saved stack pointer to the top, together with the saved context. The
first `k(v)` switches in without copying. Every later one copies the snapshot
back over the same range first. The probe above is that sequence, and the
second shot sees the first shot's stack.

**Why in place and not into a fresh stack.** An emitted frame holds pointers
into itself: the argument array a call passes to `rt_perform_p`, `rt_call_p`
or `rt_builtin_p` is a local array whose address crosses into the runtime
(`spikes/ply-parser/emit.ply`, the `(Word)(intptr_t)arr` forms). A snapshot
restored at a different address would leave every such pointer dangling in
frames that are about to run; restored where it was taken, every one is
valid. The alternative is an emitter that never passes stack addresses, which
is a change to every call site for the benefit of the one program shape the
rule below excludes.

**The rule.** A continuation cannot be resumed again while an earlier
resumption of it is suspended on the same stack, because the restore would
overwrite the frames that resumption is waiting to return into. The runtime
reports it as a failure with a code of its own rather than restoring; the
guide's `k(true) + k(false)` is sequential and is served, as is every
resumption in the language's own suites (`resumption_*_audit.rs`,
`simulation.rs`'s "resumed twice across a region"), which this record checked
by reading and stage three checks by running. The machine serves the excluded
shape today, so this is the one place the fifth step narrows what runs. It is
recorded as a rule and not as a refusal because after the fifth step there is
nothing to fall back to.

Cells are not snapshotted; they are shared state across resumptions, which is
the language's semantics (`region_wiring_audit.rs`, "two resumptions over one
cell") and the machine's. A region opened inside the snapshot and closed by
each resumption releases what that resumption allocated, because the mark it
closes to was in the snapshot.

## `simulate` as a frame the runtime serves

`simulate { body }` compiles to one call, `rt_simulate_p(ctx, closure)`, and
the emitter treats it as a `handle` whose clauses have no C bodies: the frame
it pushes answers `task`, `clock`, `random` and `sim`, and `rt_perform` routes
an operation of those effects that reaches such a frame to the scheduler
rather than to a clause closure. That is the same frame type with one more
clause kind, not a second dispatch.

The scheduler moves from `crates/ply-eval/src/sched.rs` into the runtime crate
with its logic intact and its one dependency on the machine removed: where it
holds a `Continuation` it holds the runtime's suspended stack, and where it
starts a task from a `Value` it starts a trampoline over a closure word.
`sim.rs`'s seed, plan, dependence relation and seeded `clock`/`random`
handlers move with it unchanged; `explore.rs` does not move and does not
change, because the `Simulation` trait it drives is already just "run this
seed and hand me the interleaving", and a compiled test is a closure that sets
the context's plan, enters the test root, and hands back the steps the
scheduler recorded. The harness's one call to the search
(`crates/ply-test/src/lib.rs`, the `explore(plan, ..)` site) chooses the
compiled driver when the tier holds the test, which is the same choice
`eval_test_in` makes now for an unsimulated one.

**The oracle that makes this checkable** is the count the harness already
prints. Under `--audit-backend` a simulated test runs on both sides, and the
number of interleavings each explores must be equal, not just both green: a
footprint the runtime under-records makes the search unsound and shows as a
smaller count, and one it over-records shows as a larger one. That comparison
is added to the audit at stage one and is the instrument the whole step rests
on.

The fixpoint's per-operation rule from ADR 0043 extends without a new case: a
`perform` of `task#spawn` or `clock#sleep` is answered by the nearest
`simulate` when there is one in the unit, and by the host binding when the
performer runs outside any, which is the host route below.

## The host route

A `perform` that reaches the bottom of the frames goes to the host binding the
context carries, which is the same `HostBinding` the machine hands its
handlers today. Outside a scheduler it is a call: the binding's function runs
on the performer's stack and its answer is the operation's value, and an
unbound operation is `E0424` exactly as `host.rs` raises it now. Inside the
host policy's scheduler — a production `simulate`, which is what
`Scheduler::production` builds for a served program — the binding hands back a
pending token, and the task parks on it and is picked up when the reactor
resolves it, which is `park_on_host` and `next_host` running unchanged over
stacks instead of continuations. `E0426`'s check is where it is, in the host
boundary.

This retires the rule ADR 0043 installed in the fixpoint — that a `perform`
the host would answer is refused before it is compiled — and with it the
`host_served` set the CLI computes for the producer.

## What the deletion keeps and removes

`crates/ply-eval` today is the machine, the runtime the machine and the
compiled tier share, and the seam between them. After this step it is the
runtime, and the crate keeps its name.

| stays, moved or not | goes |
| --- | --- |
| `value.rs`, `arena.rs`, `region*.rs`: the representation and the regions | `machine.rs`, `cont.rs`, `window.rs`: the CEK machine, its continuations and its slot stack |
| `builtins.rs`: the builtins the compiled tier already calls through | `code.rs`: the lowering to the machine's code |
| `sched.rs`, `sim.rs`, `explore.rs`: the scheduler and the search | `compiled.rs`, `backend.rs`: the seam, `Mutant` and the differential oracle |
| `host.rs`, `escape.rs`, `trace.rs`: the host boundary, the escape checks, the observed row | `differential.rs`, `costs.rs`, `limit.rs`'s frame bound: what only the machine measured |
| `limit.rs`'s call budget, as `Ctx.fuel` | `tests.rs` and the machine's own unit tests |

The CLI loses `--audit-backend` and the `--backend` choice with the machine,
and §17 of the guide moves in the same change; the corpus is the oracle from
then on, as it is for every language.

**The Rust suites over the machine are the other half of the deletion.**
Every file in `crates/ply-eval-tests/tests/suite` is one of two things. A statement about the language — "a string argument survives two
resumptions", "the guarded transfer never overdraws" — becomes a Ply test in
the corpus, run by the tier, keyed by its own hash. A statement about the
machine's mechanics — slot resolution, position invariance, the snapshot
audit's frame counts — is deleted with the mechanism it audits. The sort is
done file by file at stage five and the record of it is the commit that
moves each one, not a table here. This is where ADR 0042's loop begins: a
language test written in Ply re-runs only when an edit reaches it.

## Staging, and the oracle at each stage

1. **Stacks, and `simulate` with its three effects.** Built. The switcher, the
   stack allocator, the trampoline, the frame kind the runtime serves, the
   scheduler made generic and driven from the runtime, footprints at the three
   events, the seed and the record crossing the seam, and the audit pairing
   every seed the tier took with the machine's run of it. The scheduler and
   the simulation did not move: they stay in `crates/ply-eval` and the runtime
   instantiates them, which is what the deletion needs and less than a move.
   Oracle, met: the audit in whole mode over the examples takes `bank`,
   `pipeline` and `timeout`'s tests, every seed paired and every schedule the
   machine's, and the census above lost its first three rows and the cascade.
2. **`resume` off the tail, once.** The capture is a switch out and the call a
   switch in, on the primitive stage one built. Oracle: the producer test that
   asserts "cannot carry" for a non-tail `resume` asserts the body runs, and
   the audit agrees with the machine on it.
3. **Multi-shot by restore.** The snapshot, the in-place restore, the rule and
   its code. Oracle: the language's multi-shot tests moved to Ply and run by
   both sides under the audit, `k(true) + k(false)` among them.
4. **The host route.** The binding on the context, parking under the host
   policy, `E0424` and `E0426` from the runtime. Oracle: the served examples
   under `--host` (`crates/ply-cli/tests/suite`'s `w5_shutdown.rs`,
   `tls_cli.rs`, `artifact.rs`) pass with the tier holding `http.serve`, and
   the census above is the credential row alone.
5. **Delete the machine**, by the table above, and sort the suites. Oracle:
   `c tier took N of N` over both corpora with nothing to fall back to, the
   workspace builds without the removed modules, and CI's wall clock measured
   before and after with the command in `benches/`.

Stages one to four each land behind the flags that exist, with the machine
still the fallback and `--audit-backend` still the oracle. Stage five removes
both, and is the first change in this programme that cannot be audited
against the machine, which is why it is last.

## What would make this wrong

- **If the footprints cannot be made equal.** The machine records at three
  events and the runtime at the same three, but the machine's cell identity
  is a slot and the runtime's is whatever `TaskRegions` hands `rt_cell`; if
  those disagree the counts disagree and stage one does not land. The count
  oracle is what says so, before any interleaving is missed silently.
- **If a shipped program resumes a continuation while its earlier resumption
  is suspended.** Nothing shipped does; if one appears, the alternative is the
  emitter that passes no stack addresses, priced above, and the record is
  re-read.
- **If a task's stack is the wrong size.** Stacks are mapped lazily, so a
  large reservation costs address space and not memory, and the guard page
  turns an overrun into `rt_no_stack`; if the corpus's servers spawn enough
  tasks that address space matters, the reservation is a knob, not a design.
- **If a third architecture is needed.** The switcher exists for aarch64 and
  x86_64, which are the two the tree builds on; another is a third copy of two
  short functions behind the same interface, and the stack module's tests say
  whether it is right.
- **If the search should be in Ply.** Stage five leaves `explore.rs` in Rust
  in the runtime, and step six of ADR 0042 deletes the front end, not the
  runtime. Translating the search is a later record, if the loop wants it.
