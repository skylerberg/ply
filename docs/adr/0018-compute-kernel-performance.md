# ADR 0018 — Closing the gap for compute kernels

Status: proposed. **No decision here is accepted.** Every section states what is
measured, what is assumed, and the measurement that would settle it — because
ADR 0017 asserted an unmeasured premise, called it forced, and cost three
milestones (see `CONTRIBUTING.md` §"Measure an ADR's motivating claim before
accepting the ADR").

## Context

The motivating question: could a maximally-performant Monte Carlo tree search
library be written in Ply rather than Rust? MCTS is a good probe because it is
almost pure compute — a tight loop over a mutable tree, millions of iterations,
hot RNG, and parallel rollouts contending on shared nodes. It exercises every
place Ply is currently weak and almost none of where it is strong.

The answer today is no, by roughly an order of magnitude. What follows is a plan
per gap, ordered by what the existing measurements actually support.

### What is measured

| Figure | Source |
| --- | --- |
| 41.6× the Rust floor, served request | `benches/w6-ladder-r3.json` |
| 2.51× tree-walker, control-stack machine on the pure path | ADR 0016 |
| 11.67× minimum, codegen spike on its compilable fragment | **computed from** `benches/w6-spike.json` — interpreter-best ÷ spike-worst, not a field in the file |
| 1.55× end to end from a fast execution strategy | ADR 0016 §"the second lever" |
| 1,082 allocations per `/health`, 24% in `frame::dispatch` | `w6-alloc`, `w6_alloc_sites` |

### What is assumed and not measured

That an MCTS kernel's hot loop falls mostly inside the spike's compilable
fragment. Everything below is ordered on that assumption and **§1 exists to test
it before anything is built.**

## 0. §1 RAN. The assumption held and the plan failed anyway.

> **Measured 2026-08-21.** `benches/adr0018-mcts.json`, one run, cranelift
> 0.134.3, load 3.54 falling to 3.05. Kernel: `benches/kernel/mcts.ply`,
> three-heap Nim, tree as `Map<Int, Node>`, UCB1 in integer fixed point. Agreement
> was taken **before anything was timed** — 1,344 generated cases across 21
> functions plus 24 whole-kernel searches, against both engines, 0 disagreements.

§1 said everything below it was ordered on one untested assumption: that an MCTS
kernel's hot loop falls mostly inside the spike's compilable fragment. It said to
rewrite this ADR if the fraction came back low.

**The fraction came back high — 81.0% of executed work, against 2–5% for an HTTP
request — and codegen still bought nothing.**

| compiled set | crossings | end to end |
| --- | ---: | ---: |
| floor, no JIT | 0 | 1.000× |
| entry only | 1 | 0.999× |
| outer loop | 102 | 0.996× |
| + playouts | 102 | 0.999× |
| **everything accepted** | 102 | **0.998×** |

0.998× is inside the method's own noise floor. The kernel ratio on the part that
is wholly inside the fragment and crosses zero times, `mcts.playouts`, is
**52.58×** — and reporting *that* would misprice the decision in exactly the way
ADR 0016 warned about.

**Why 81% inside and 52× there still buys nothing: the interpreter cannot call
compiled code.** Only compiled→interpreter exists (`rt_call_machine`). Every
function the fragment accepts whose *callers* it refuses is compiled and then
never entered. Compiling the twenty arithmetic functions between rungs 2 and 4
moved the whole program from 57,700 µs to 57,582 µs. In the hybrid, compiled code
is reached by exactly three functions: `mcts.iterate` (100 entries), `mcts.root`
(1), `mcts.best_action` (1).

It is **not** the boundary cost. 102 crossings total 9.9 µs, 0.017% of the run.

**The ceiling, by Amdahl over the two measured numbers and nothing else:**

| | |
| --- | ---: |
| a backend that could be *entered* from interpreted code | **4.86×** |
| an infinitely fast fragment | **5.26×** |

Not 11.67×, and not 52×. This ADR opens by saying Ply is off the Rust floor "by
roughly an order of magnitude" on MCTS. **The fragment as specified caps the
recovery at 5.26×**, so §2 onward cannot reach the goal §"Context" states, and
the binding constraint is architectural — one-way calling — rather than the size
of the fragment or the cost of the boundary.

### What is actually outside, ranked — this is the roadmap §1 promised

By lowered nodes removed from the fragment:

| nodes | fns | what |
| ---: | ---: | --- |
| 253 | 7 | a field access |
| 71 | 2 | a list pattern `[x, ..rest]` |
| 25 | 2 | unary `-` |
| 10 | 1 | a list literal `[]` |

And by *executed* work, **19.0% of the run is the `Map`/record/list machinery
itself**, which is outside the fragment no matter which functions compile.

Two hazards that belong to the fragment rather than to any function, so they
appear in no census: there is **no `Float` path** — accepted silently and raising
at run time — and **no nested-call bound**, where the machine answers
`recursion limit of 10000 nested calls exceeded` and compiled code SIGABRTs.

> **Corrected (R5, 2026-08-21): the nested-call half is closed; the `Float` half
> is closed at the boundary and still open inside the fragment.**
>
> The sentence above is quoted verbatim from before R5 and both halves of it were
> true when it was written. `benches/adr0018-mcts.json`'s
> `recursion_bound_compiled` holds the crash it describes:
> `the process died: signal: 6 (SIGABRT) — ... stack overflow`.
>
> *Nested calls.* Every compiled function now carries a four-instruction fuel
> prologue seeded from the budget `ply_eval::Compiled::enter` is handed — the
> machine's own `max_calls` minus its current depth. A body that would pass it
> fails, the entry declines, and the machine evaluates the definition and raises
> **its own** diagnostic. Both probes now answer
> `recursion limit of 10000 nested calls exceeded`
> (`mcts --dir benches/kernel --probe machine|compiled`), and
> `crates/ply-codegen-spike/tests/mcts_kernel.rs::a_runaway_recursion_is_the_machines_diagnostic_and_not_a_crash`
> asserts it as a subprocess, because an in-process assertion cannot observe its
> own `SIGABRT`. It is not free: that program takes **7.9 s** with a backend
> attached against **0.11 s** without, because the machine re-offers the same
> function at all ten thousand interpreted depths and each attempt burns its
> whole remaining fuel first — 19,992 entries and 10,000 fuel declines. Bounded,
> paid only by a program that is about to die, and recorded rather than fixed;
> `crate::entry::Declines::out_of_fuel` carries the reasoning.
>
> *`Float`.* The fragment still compiles `a + b` as `Int` arithmetic whatever the
> operands are, and
> `mcts_kernel.rs::the_fragment_accepts_float_arithmetic_and_then_fails_on_it_at_run_time`
> still passes. What changed is that it can no longer be *reached* from a
> program: `ply_eval::Compiled` carries `Int` and `Bool` and nothing else in
> either direction, and the spike will not register a definition whose declared
> signature is not `Int`/`Bool` throughout. Two independent refusals, and the
> same test now asserts that a `Float` call is never offered to the backend at
> all. Inside the fragment, called directly, the gap is exactly as described.
>
> > **Narrowed again (R5 audit pass, 2026-08-21): "it can no longer be reached
> > from a program" is one word too strong.** Neither refusal reads a definition's
> > *body*, so a `Float`, `Decimal` or `String` **literal inside an `Int` -> `Int`
> > body** passes both: `numerics.float_inside(n) = if 1.5 + 1.5 > 2.0 { n } else
> > { n * 2 }` is compiled, is registered as enterable, and is offered. What
> > happens then is the part worth writing down — the native body runs, meets the
> > constant at `rt_unbox_int`, fails, and the entry **declines**, so the program
> > gets the interpreter's answer and a slow call rather than a wrong one.
> > `crates/ply-codegen-spike/tests/hazards.rs::a_float_or_decimal_literal_inside_an_int_body_is_never_a_wrong_answer`
> > asserts the decline as an exact count, so a lowering that ever *answered* one
> > of these goes red.
> >
> > The census consequence in ADR 0019 §5 item 4 stands and is now demonstrated:
> > those three definitions are counted as compiled and cannot run. The audit's
> > third case for this hazard, ordering on `String`, is not reachable at all —
> > `E0201` refuses `a < b` on `String`, so the run-time support in `interp.rs` is
> > unreachable from a well-typed program (`tests/fixtures/string_ordering/`).

### What this changes

§2 through §7 below were ordered on the assumption §1 tested. The assumption held
and the conclusion did not follow from it, so the ordering is not rescued by
compiling more: **make the interpreter able to enter compiled code, or the
ceiling is 5.26× however much of the fragment you accept.** That is a different
first milestone from any listed below, and nothing below should start until it is
decided.

## 1. Re-price the spike against a compute kernel — do this first

> **Discharged (R4, 2026-08-21), and it changed the ordering of everything
> below.** The measurement this section asked for is
> `benches/adr0018-mcts.json`, written by the command in `benches/README.md`
> §"What `mcts` adds"; the kernel is `benches/kernel/`, in Ply, and it passes
> `ply test benches/kernel/ --engine both`. The assumption held on **shape** —
> 22 of 34 kernel functions, 386 of 745 lowered nodes, and **81.0% of the
> kernel's executed work** are inside the fragment, against the 2–5% ADR 0016
> measured for an HTTP request. The conclusion it was supposed to license did
> **not** hold: end to end the hybrid is **0.998× [0.979–1.007]** against a
> harness floor of 1.000× [0.994–1.009], because **the interpreter cannot call
> compiled code** — a function the fragment accepts whose callers it refuses is
> compiled and never entered. The Amdahl ceiling over the two measured numbers
> is **4.86×**, not the 11.67× this ADR carries for the spike and not the
> 52.58× the fragment shows where it does run. `docs/adr/0019-value-
> representation.md` §5 is the write-up and lists the six things an amendment
> to this document owes; it does not make them, and neither does this block.


The spike measured **11.67× minimum** on arithmetic, comparisons, `if`, `let`,
`block`, and `match` on literal patterns. ADR 0016 concluded 1.02–1.05× end to
end because that fragment is 2–5% of an HTTP request.

An MCTS inner loop — select by UCB, increment a visit count, accumulate a score,
compare against a bound — is close to *entirely* that fragment. If it is, the
end-to-end number is not 1.05× and the whole ordering below changes.

**The work:** write an MCTS kernel in Ply, point `crates/ply-codegen-spike` at
it, and report the same conservative interpreter-best ÷ spike-worst ratio the W6
spike reports, plus the fraction of the kernel the spike refuses by name.

**Cost:** small. The spike exists, is standalone, and `rm -r` removes it.

**Why first:** it is the cheapest measurement with the highest information, and
it tests the one assumption everything else rests on. If the compilable fraction
is low, §2–§4 are the wrong plan and this ADR should be rewritten rather than
executed.

## 2. Unboxed primitives

> **Corrected in place (R4, 2026-08-21). Both sentences of "The gap" below are
> wrong, and they are left standing verbatim underneath this block because
> `CONTRIBUTING.md` §"Correct, do not delete" wants the withdrawn claim beside
> the measurement.** This ADR opened §2 by reasoning from them, and a milestone
> was requested on their strength.
>
> **"Every `Int` is a heap-allocated `Value`" is false.** `Int`, `Bool`,
> `Float`, `Unit`, `Decimal`, `Cell` and `Task` are inline variants of the
> `Value` enum (`crates/ply-eval/src/value.rs:50-104`) and building one touches
> no allocator. `size_of::<Value>()` is 32 bytes. Printed by name, by a test
> that runs:
>
> ```
> cargo test -p ply-corpus --release --test r4_value_construction -- --nocapture
> ```
>
> > ```
> > -- what one Value costs to build --
> >   Value::Int       0 allocations
> >   Value::Bool      0 allocations
> >   Value::Float     0 allocations
> >   Value::Unit      0 allocations
> >   Value::Decimal   0 allocations
> > ```
>
> **There is no primitive boxing in this evaluator to remove**, so the plan
> below — "a tagged representation where `Int`, `Bool` and `Float` live inline
> in the value word rather than behind a pointer" — describes what the tree
> already does.
>
> **"`interp::literal` allocates 111 times per request" attributes to the wrong
> thing.** The count was real; the conclusion drawn from it did not follow.
> `interp::literal` cannot allocate at all unless the literal is a `Str` or a
> `Bytes` (`crates/ply-eval/src/interp.rs:1000-1010`), and the 111 was read off
> a **20-request window**: it fits to 65.0 per request plus 925 once per
> `Machine`, and 65.0 + 925/20 = 111.25. One-time work divided by twenty looks
> exactly like a per-request cost, which is the failure `CONTRIBUTING.md`
> §"Measure an ADR's motivating claim before accepting the ADR" closes with.
>
> **What the request actually spends its allocations on** was measured instead,
> attributed to the value being built rather than to the frame that built it and
> fitted over two windows: the largest line was the **call-argument vector**, at
> 372.4 per request, 40.9%. R4 landed a free list for it and built a
> compile-time constant's `Value` once, and `/health` went from **1,082 to 773**
> allocations per request (`./target/release/w6-alloc --repo . --requests 200`).
> `docs/adr/0019-value-representation.md` is that ADR; its §4 **rejects**
> narrowing `Value`, with the number that would have justified it, which is
> zero allocations.
>
> **§2's success criterion is also spent.** It reads "a `w6_alloc_sites` re-run
> showing `interp::literal` gone from the top sites". `interp::literal` is now
> 0.0 allocations per request on both routes — and the profile's top line is
> unmoved, because it never was `interp::literal`.
>
> **This ADR is not otherwise amended, and §5 below is untouched.** ADR 0019 §5
> lists what an amendment owes, including the item that outranks everything in
> this section: a backend the interpreter cannot *enter* buys nothing whatever
> the representation is, measured at 0.998× end to end.

**The gap.** Every `Int` is a heap-allocated `Value`. `interp::literal` allocates
111 times per request on a workload doing almost no arithmetic. MCTS is visit
counts and score accumulation in the innermost loop.

**The plan.** A tagged representation where `Int`, `Bool` and `Float` live inline
in the value word rather than behind a pointer, with heap allocation only for
compound values. This is the change ADR 0017 was *supposed* to enable and did
not, because the allocations were never in the world.

**What it interacts with.** `ply-hash` normalization, the store codec and its
schema fingerprint, `--engine both` (both engines must agree on the new
representation), and `Value::render`. The schema fingerprint will need bumping;
W2's `Map` landing is the precedent for how.

**How to know it worked.** Allocations per `/health` against 1,082, and a
`w6_alloc_sites` re-run showing `interp::literal` gone from the top sites.

**Unmeasured:** whether tagging costs more in branch overhead than it saves in
allocation. Measure on the kernel from §1, not on the HTTP path.

## 3. Handler specialization (evidence passing)

**The gap.** `random.gen()` walks the handler stack. MCTS calls it in the
innermost loop, millions of times. Effects are the language's central
abstraction and they are dynamically dispatched.

**The plan.** Where the handler for an operation is statically known at a perform
site, pass the evidence directly and inline the clause, so the effect disappears.
This is the effect-system analogue of monomorphization, and it is what would make
Ply's defining abstraction free rather than merely cheap.

**Precedent.** Koka does this. ADR 0010 already named evidence passing as the
mechanism that would let a resolution layer elaborate away before hashing.

**How to know it worked.** Time a `random.gen()`-dominated loop with the handler
statically known versus installed dynamically. The ratio is the win.

**Unmeasured:** what fraction of performs in a real kernel have a statically
known handler. If most handlers are installed at an entry point and the kernel
performs against them, the fraction should be high — but that is a guess.

## 4. Unboxed mutable arrays

**The gap.** MCTS wants a flat node array with index-based children, not a
pointer-chasing tree of ADTs. Ply has `List`, `Map` and records — all persistent
or boxed. There is no primitive array with in-place update. Regions give scoped
lifetime, not a contiguous unboxed buffer.

**The plan.** A primitive `Array<a>` with in-place update, region-allocated, with
its brand preventing escape exactly as a cell's does. In-place update requires
unique ownership, which is what R1/R2's region work already establishes — this is
the first feature where that machinery pays for itself on performance rather than
on safety.

**What it interacts with.** The escape check must run on resolved types (W2 found
the analogous hole reachable through a type alias). Derivation must refuse or
handle arrays deliberately. `--engine both` must agree.

**How to know it worked.** A node-array MCTS kernel against the same kernel built
from ADTs, on allocations and wall clock.

## 5. Monomorphization

**The gap.** A generic `Node<S, A>` stays boxed and dynamically shaped where Rust
emits flat structs.

**The plan.** Specialize generic definitions at their concrete instantiations.
Content addressing makes the bookkeeping natural — a specialization is a
definition with its own hash, cached and invalidated like any other.

**The cost to watch.** Definition-count inflation, which ADR 0010 already flagged
for derivation and which the incremental front end absorbed (10,000 definitions
check from scratch in 0.45 s). Monomorphization multiplies harder than derivation
does. Measure the definition count and cold-check time, not just the speedup.

**Sequencing:** after §2, because monomorphizing over boxed values buys much less.

## 6. Parallel rollouts and shared mutable state

**The gap, and it is a design gap rather than an optimization.** Tasks are
suspended machine states with their own region stacks — heavier than a rollout
worker needs. Worse, MCTS wants many workers mutating shared tree nodes, and the
region brand refuses exactly that. There is no vocabulary in the language for an
explicitly shared, interior-mutable arena.

**The plan.** This needs its own ADR and should not be improvised. The shape worth
exploring: an explicitly declared shared region whose values are reachable from
multiple tasks, where the isolation guarantee is *deliberately* relaxed and the
relaxation is visible in the type — so a reader can see which state is shared and
the scheduler can stop pretending those tasks are independent.

**What must not happen.** A silent relaxation. W4's worst defect was a pooled
connection coupling two tests the footprint graph believed disjoint; a shared
arena is the same hazard with a different name, and it must be declared rather
than inferred.

**Consolation for the interim:** the deterministic simulation is a genuine
advantage here. Seeded rollouts replay exactly, and footprint-guided reduction
found the `bank.ply` race in 2 interleavings with the seed as the repro.
Debugging a nondeterministic parallel tree search in Rust has no equivalent.

## 7. SIMD, layout control, and an escape hatch

**The gap.** Rust MCTS implementations lean on all three. Ply has none.

**The plan: mostly don't.** An `unsafe` escape hatch punctures the effect
system's central claim that the runtime knows what code can do, and ADR 0008 §2
already establishes the honest alternative — a **host handler** with a declared
footprint, a determinism flag, a linearity obligation, and a line in `ply hosts`.
The boundary costs 0.5 µs per crossing, measured, which is nothing at kernel
granularity.

So: SIMD kernels and layout-sensitive inner loops go behind host handlers, in the
trusted computing base, enumerable in one command. That is a better answer than a
language-level escape hatch because it is *listable*.

## The middle path, available today

Write the MCTS kernel in Rust behind a host handler, and the strategy, evaluation
and experiment harness in Ply. That is precisely what W1's boundary was built
for. You get simulation, specs, exact test selection and bisection over the part
of the system where correctness is hard, and native speed over the part where it
is not.

This is not a concession — it is the design working. But it should be stated as
the current answer so nobody discovers it by disappointment.

## Sequencing

1. **§1, re-price the spike on a kernel.** Cheap, and it tests the assumption
   everything else rests on.
2. **§2, unboxed primitives.** Largest single gap; nothing else matters until it
   closes.
3. **§3, handler specialization.** Makes the defining abstraction free.
4. **§4, unboxed arrays.** First feature where the region work pays on
   performance.
5. **Re-price codegen again.** ADR 0016's verdict was about the old
   representation; §2–§4 change exactly what made its ceiling low.
6. **§5 monomorphization**, then **§6 shared state** as its own ADR.

## What would make this ADR wrong

If §1 shows an MCTS kernel is mostly *outside* the spike's compilable fragment,
the ordering above is wrong and codegen is not the endpoint. Rewrite rather than
proceed.

If §2 lands and allocations do not move, the same reasoning that failed for
regions has failed again, and the next step is another attribution run rather
than the next item on this list.
