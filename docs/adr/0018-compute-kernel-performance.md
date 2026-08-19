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
| 11.67× minimum, codegen spike on its compilable fragment | `benches/w6-spike.json` |
| 1.55× end to end from a fast execution strategy | ADR 0016 §"the second lever" |
| 1,082 allocations per `/health`, 24% in `frame::dispatch` | `w6-alloc`, `w6_alloc_sites` |

### What is assumed and not measured

That an MCTS kernel's hot loop falls mostly inside the spike's compilable
fragment. Everything below is ordered on that assumption and **§1 exists to test
it before anything is built.**

## 1. Re-price the spike against a compute kernel — do this first

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
