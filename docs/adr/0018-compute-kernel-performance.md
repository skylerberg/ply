# ADR 0018 — Closing the gap for compute kernels

Status: proposed. **No decision here is accepted.** Every section states what is
measured, what is assumed, and the measurement that would settle it — because
ADR 0017 asserted an unmeasured premise, called it forced, and cost three
milestones (`CONTRIBUTING.md` §"Measure an ADR's motivating claim before
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
| the served request against the Rust floor | `benches/w6-ladder-r3.json` |
| the tree-walker against the control-stack machine on the pure path | ADR 0016 |
| the codegen spike's minimum on its compilable fragment | **computed from** `benches/w6-spike.json` — interpreter-best ÷ spike-worst, not a field in the file |
| what a fast execution strategy is worth end to end | ADR 0016 |
| allocations per `/health` and where they land | `w6-alloc`, `w6_alloc_sites` |

**The served rung runs a pre-built binary.** `mcts` itself is built by cargo and
is current by construction, but its `--served` denominator starts
`target/release/ply` as a subprocess and checks only that the file **exists**.
The kernel corpus imports no `std` module, so the kernel numbers are exposed to a
stale interpreter and never to a stale stdlib; the served rows are exposed to
both. `CONTRIBUTING.md` §"The binary is an instrument too" has the check.

### What is assumed and not measured

That an MCTS kernel's hot loop falls mostly inside the spike's compilable
fragment. Everything below is ordered on that assumption and **§1 exists to test
it before anything is built.**

## 0. §1 RAN. The assumption held and the plan failed anyway.

§1 said everything below it was ordered on one untested assumption, and to
rewrite this ADR if the fraction came back low. **The fraction came back high —
most of the kernel's executed work is inside the fragment, against a few percent
for an HTTP request — and codegen still bought nothing.** End to end, every
compiled set from "entry only" to "everything accepted" lands inside the method's
own noise floor. The kernel ratio on the part that is wholly inside the fragment
and crosses zero times is enormous — **and reporting *that* would misprice the
decision in exactly the way ADR 0016 warned about.**

**Why a high share and a large in-fragment ratio still bought nothing: the
interpreter cannot call compiled code.** Only compiled→interpreter existed. Every
function the fragment accepts whose *callers* it refuses is compiled and then
never entered; in the hybrid, compiled code was reached by three functions.

It is **not** the boundary cost. The crossings are a rounding error of the run.

**The ceiling, by Amdahl over the two measured numbers and nothing else**, came
out far below the in-fragment ratio, and barely higher at an infinitely fast
fragment. This ADR opens by saying Ply is off the Rust floor by roughly an order
of magnitude on MCTS. On that model **the fragment as specified caps the recovery
below the goal §"Context" states**, so §2 onward could not reach it, and the
binding constraint is architectural — one-way calling — rather than the size of
the fragment or the cost of the boundary. **R5 built entry, beat that ceiling,
and §0.5 withdraws it as a bound.** Read §0.5 before §1.

### What is actually outside, ranked — this is the roadmap §1 promised

`refusals_ranked` in `benches/adr0018-mcts.json` ranks the refused constructs by
the lowered nodes they take with them: a field access, then a list pattern, then
unary `-`, then a list literal.

**It is a FIRST-REFUSAL census, and read as a work list it is wrong by more than
an order of magnitude.** The table is accurate and this section's title is not:
it invites the reading "remove the top row, seven functions come inside", and
that is not what happens. Two things break the arithmetic:

- **A refusal names one construct, not all of them.** A function listed under
  unary `-` may also contain `[]`. Admitting unary `-` moves it to the
  list-literal row rather than into the fragment.
- **The fragment must be closed under calls** — `jit::Denotes::Uncompiled`
  refuses the caller of anything it did not compile — so a function is admitted
  only when every construct in it *and every function it reaches* is admitted.

Measured one item at a time with `mcts --dir benches/kernel --only agreement`:
**the top-ranked row delivered one function and a handful of nodes, and changed
nothing about what executes.** The rows are not four wins; they are one closure,
and the whole of it arrives on the last item. A fifth construct that appears in
no row — a constructor pattern, which sits behind the field access and therefore
never got to be a first refusal — had to be lowered as well. With all of them in,
the fragment takes every function and every node of the kernel.

`crates/ply-codegen-spike/tests/spike.rs`'s stdlib row for `std.http.parse_head`
is the standing illustration: it used to be refused for a field access and is now
refused for its call to `read_line`. **Removing the named construct moved the
name.**

**What did not move, and bounds all of it:** the `Map`, record and list machinery
itself — a fifth of executed work — is still outside the fragment and untouched
by any of this. `rt_field`, `rt_record` and `rt_list` call `ply_eval`'s own
representations; widening changes *which functions compile*, never what a `Map`
insert costs.

Two hazards belong to the fragment rather than to any function, so they appear in
no census:

- **No nested-call bound.** *Closed by R5.* Every compiled function carries a
  four-instruction fuel prologue seeded from the budget
  `ply_eval::Compiled::enter` is handed; a body that would pass it fails, the
  entry declines, and the machine raises its own diagnostic. **It is not free:**
  the machine re-offers the same function at every interpreted depth and each
  attempt burns its whole remaining fuel first, so a runaway program is orders of
  magnitude slower with a backend attached. Bounded, paid only by a program that
  is about to die, and recorded rather than fixed —
  `crate::entry::Declines::out_of_fuel` carries the reasoning.
- **No `Float` path.** The fragment compiles `a + b` as `Int` arithmetic whatever
  the operands are. Closed at the boundary — `ply_eval::Compiled` carries `Int`
  and `Bool` and nothing else, and `jit::Fx::literal` refuses a `Float` or
  `Decimal` literal — so the fragment's claimed coverage and its behaviour now
  agree. **Refuse rather than build a path was the choice, and the reason is
  scope, not difficulty:** a real `Float` path widens the agreement surface by
  `NaN`, by `-0.0`, by float equality and by `Decimal` precision, and the
  workload the fragment is being widened for is field accesses and list patterns.
  §2 still asks for `Float` unboxed; this closes the *dishonesty*, not the item.

  **What is still open, and a reader will assume it is closed:** only the
  literal-shaped case is refused. A `Float` arriving in an `Int` -> `Int` body as
  a **builtin's return value** is refused by nothing — it meets `rt_unbox_int`,
  fails, and declines. That is a slow answer rather than a wrong one, which is
  the property `crates/ply-codegen-spike/tests/hazards.rs` asserts as an exact
  decline count, so a lowering that ever *answered* one of these goes red.

### What this changes

§2 through §7 were ordered on the assumption §1 tested. The assumption held and
the conclusion did not follow from it, so the ordering is not rescued by
compiling more: **make the interpreter able to enter compiled code, or the
ceiling holds however much of the fragment you accept.** That is a different
first milestone from any listed below.

## 0.5. Entry was built. It beat this ADR's ceiling, and the ceiling was wrong.

Pre-registered before any number existed: `benches/r5-timing/PRE-REGISTERED.md`.
Result: `benches/r5-timing/RESULTS.md`. Raw report:
`benches/r5-timing/mcts-r5.json`, cut only by `benches/r5-timing/analyze.py`.
Agreement was taken before anything was timed — thousands of generated cases and
whole-kernel searches, zero disagreements, against both the no-backend machine
and the independent tree-walker.

### What was built

`crates/ply-eval/src/compiled.rs` — a `Compiled` trait taking a name, some
scalars and a call budget and returning at most one scalar. No arena, no stack,
no handler stack, no host binding, no `&mut Machine`, no route back in, **so a
backend that cannot finish has changed nothing observable and declining is free
by construction.** The hook is one branch in `Machine::enter_code`.

### The cost side, on the workload that ships

**Zero allocations per `/health` request**, by the pre-registered rule. `HOOK`
(the tree as it ships, no backend) against `NOHOOK` (the same tree with the
`enter_code` call site deleted), two binaries from one frozen tree, arms
alternated: byte-identical at every window size, so the delta is flat in the
window rather than absorbed by it. In the linked binary the hook is eighty bytes
of machine code.

**Zero allocations is not zero cost, and the load-independent count that says so:
one `/health` request reaches the hook hundreds of times, and every one is a
miss.** The wall-clock cost of those branch tests **was never taken**, on either
binary, although both existed. The currency was pre-registered as allocations and
a wall-clock rule was not; this ADR should not be read as if it had been.

### The kernel ratio

The ladder is monotone in entries, the aggregate is filter-independent, and both
pre-registered controls sat inside their band — a harness floor and a
nothing-enterable rung. **No window was dropped by either pre-registered filter
on any rung**, which also means neither filter was ever tested; on macOS the
1-minute load average updates far more slowly than a window lasts, so the load
filter can only drop a whole rung or none. Pre-registered verdict
**`entry-paid-off`**. Two reviewers replicated the top rung independently on
busier machines, both formally void under the pre-registration's own load rule
and both reported as direction and magnitude rather than as a second result.

Pre-R5 the same rung was worth nothing, with zero entries. **§0's diagnosis —
that the binding constraint was architectural, one-way calling, and not the size
of the fragment or the cost of the boundary — is confirmed.**

**The figure has never been re-taken on the cranelift the repository pins, and
the reason is a blocked instrument rather than a busy machine.** The spike moved
from cranelift 0.134.3 to 0.132.3 so it builds on the pinned toolchain. The
obvious question — does the older cranelift generate materially worse code — was
pre-registered before the port with this section's own command, statistic, window
filter, load gate and controls taken over verbatim, and then **could not be
run**: `mcts` verifies agreement before it times anything and `bail!`s on the
first disagreement, and the agreement corpus is red. `CONTRIBUTING.md` §"Things
known to be broken" item 18 has the defect.

**The red is not the port's.** It reproduces on the newer cranelift from
unmodified source, and the ported build's agreement output is byte-identical to
the unported one — same digest, same disagreements, same per-function entry
counts. So the port is a null result on everything this instrument can still see.
**Nothing here should be read as evidence that the two versions generate equally
fast code; it is evidence that they generate the same *answers*, which is a
different claim.**

**A cranelift-0.132.3 number on this kernel now exists, and it is not this one.**
`ply test benches/kernel --backend cranelift` is measured against no backend in
ADR 0030's series. **It does not replace this section's ratio and must not be
quoted as a re-take of it.** Four differences, each widening the denominator: it
measures a whole `ply test` invocation — front end, hashing, cache check, test
harness — where this section measures the kernel body alone; it pays JIT
compilation inside the window; it runs the corpus's tests rather than this
section's iteration counts; and it enters more often. The two numbers are not in
tension and neither is evidence about the other. What it does settle is a smaller
thing this ADR could not: **the fragment over this kernel is intact under the
pinned cranelift and under a fresh implementation of the seam** —
`crates/ply-codegen/tests/suite/kernel.rs` compiles every definition of the kernel as
one closed unit with zero refusals.

### 6.199× is above this ADR's own ceiling, and that is a defect in the ceiling

**The measurement is above the ceiling §0 derives**, taken with barely half the
kernel's functions accepted — nowhere near "however much of the fragment you
accept". A result that beats its own predicted ceiling is a sign that the model
was wrong, not that the result is extra good, and the model was wrong in a way
this ADR can name: interpreted, the search offers tens of thousands of calls to
the hook; with everything compiled it offers a small fraction of that. **The rest
stop existing, because they now happen inside a native body.**

The ceiling's denominator was built by pricing each function's **body** in
isolation — `per_call` subtracts the machine's own entry cost — which charges the
call-site machinery to the unattributed bucket rather than to any function. The
executed-work share is a **body-only accounting**, and entering compiled code
deletes the call-site machinery too: the argument vector, the frame push, the
`Env` binding. At the measured machine entry cost alone those vanished calls are
several percent of the search, **and arrival is the cheapest part of an
interpreted call.** **§0's ceiling is an artifact of a body-only attribution, not
a bound, and this ADR withdraws it as a bound.**

**The same fact turned up as a correctness defect, from the other side.** A
compiled body pushes **one** `Frame::Call` for a whole call; the interpreter pends
a frame per pending operand as well. That is the unattributed machinery, counted.

### What entry also bought: a divergence, with the real backend and no mutation

A deeply recursive body with a long chain of `+ 1` terms, accepted whole by the
fragment, raised on the machine alone and **answered** with the backend attached
— `ply_eval::compare_answers` calling it a divergence. The seam passes only a
call budget, and the machine's *second* bound could not be expressed at the
boundary, so no backend could honour it.

**Fixed, and the fix was to delete the second bound rather than to widen the
seam.** There is one bound now and the budget expresses all of it. The frame
count was a resource guard on the machine's own heap that had been phrased as a
program answer, and it was sensitive to **how a body's operands were spelled**
rather than to what the body did — the same recursion raised or answered
depending on whether a sum was written out or folded. `Machine::with_max_frames`
keeps it as an opt-in ceiling, and a machine holding one now enters no compiled
body at all, so nothing this seam admits can turn on a limit it was not handed.

Two more the reviews found, both since closed. Every entry into the spike's
backend cost O(the previous entry's peak arena) — the real mechanism behind a
per-function regression `RESULTS.md` §3 blamed on its own filter — fixed by
`Ctx::end` clearing the arena at the end of the entry that filled it. And a
definition that discharges its own effects publishes an empty row, so the seam's
purity gate cleared it and offered it — fixed by publishing a second fact rather
than a second row: `ply_core::DefInfo::internally_effectful`, transitive over the
call graph, refused at the seam by `Gate::InternalEffects`.

### Nothing here ships, and that is the load-bearing sentence

**No shipping command can install a backend.** `Compiled` and `set_compiled`
appear nowhere in `ply-cli`; outside `crates/ply-eval`'s own tests and the
deletable spike, `set_compiled` has no caller. `ply test --engine both` cannot
attach one and therefore catches **none** of the deliberately wrong backends the
spike's mutation harness runs. The rule that a run with a backend attached is a
third execution strategy whose results the result cache must not keep is stated
in `Machine::set_compiled` and is **not enforced, because it is unreachable** —
`cache_bypassed` has no `Machine` in scope. So **none of this is available to any
user of Ply.** It is a measurement at a seam only the spike's harness and
`ply-eval`'s differential corpus can reach.

### So are §2 through §7 correctly ordered?

**No, and not for the reason §0 gave.** §0 said the ordering was not rescued by
compiling more because of the ceiling. That ceiling is withdrawn above. What
replaces it is worse for the list, not better:

1. **§0's own first milestone is discharged as an experiment and not as a
   feature.** "Nothing below should start until it is decided" — it is decided:
   entry works, it pays on this kernel, and it is unreachable from every shipping
   command. Making it reachable is M9, and ADR 0016 §3.5 requires the spike be
   deleted rather than promoted, so the next decision this ADR owes is not a
   lever at all.
2. **§2 is spent**, and was corrected in place by R4: there is no primitive
   boxing to remove.
3. **§3 is unpriced by this measurement and must not be read as re-ranked.**
   `benches/kernel` declares **no effect at all**, so no entered definition ever
   exercised a handler, the purity gate or the effect path. R5 says nothing
   whatever about evidence passing.
4. **§4 is the largest lever this ADR still identifies for this kernel**, by
   §0's own unchanged finding: the `Map`/record/list machinery is outside the
   fragment no matter which functions compile, and is therefore what caps a
   kernel that *can* enter compiled code. That is inference from a pre-R5
   attribution, not a new measurement, and it should be re-taken on a hybrid run
   before anything is built on it.
5. **§5's sequencing note is void**: it reads "after §2", and §2 does not exist.
6. **§6 and §7 are untouched.** Nothing in R5 bears on shared mutable state or on
   the host boundary.

The honest summary: the ordering below was derived from a model that undercounts
what entry removes, and every remaining item is still priced by that model. **No
lever in §2–§7 has an end-to-end price on a kernel that can enter compiled code.**
That is what an amendment owes, and this section does not pay it.

### What a reader still does not know

- **The wall-clock cost of the hook on the request path.** Only allocations were
  taken. Zero allocations must not be read as zero cost.
- **What a backend costs in allocations.** No allocation figure was taken with a
  backend attached anywhere, and `compiled.rs` warns that a `w6-alloc` figure
  from a run without one may not be quoted for a run with one.
- **Whether the ratio holds anywhere else.** One kernel, one program, one box,
  one pre-registered run; the pre-registration forbade re-running.
- **What JIT compile time costs.** It is in no ratio; every window times warm
  code.
- **Anything about the seam under the rest of the language.** Effects, handlers,
  continuations captured across an entry, `simulate`, secrets,
  `Float`/`Str`/`Decimal`/`Bytes`, higher-order closures, the store, the test
  cache, `ply prove` and the host path were **not exercised with a backend**.
- **Whether the fragment's own lowering is correct.** `jit.rs`'s instruction
  selection was read and never mutated; every wrong backend R5 built corrupts
  *answers*, not lowering.
- **How strong the agreement result really is.** Most of the compiled functions
  are offered to the backend **zero** times during the whole-kernel searches, and
  a uniform off-by-one in the scoring function changes no move in any of them.
  **Half the entered functions are caught by their own generated cases and by
  nothing else.**
- **Whether the oracle is right.** If the tree-walker and the machine were wrong
  in the same way, every comparison here agrees.

### One correction to §1's discharge block, while this is open

**`benches/adr0018-mcts.json` holds the pre-R5 numbers** — including an
end-to-end figure measured with zero entries. R5 wrote to
`benches/r5-timing/mcts-r5.json` and did not overwrite the artifact §1 quotes.
The two disagree on the fragment's shape and **the newer file is right**:
removing the trampoline made a compiled set closed under calls, so the kernel's
top-level driver functions are now refused by name and the census shrank. The
executed-work share is essentially unmoved.

## 1. Re-price the spike against a compute kernel — do this first

**Discharged by R4, and it changed the ordering of everything below.** The
measurement is `benches/adr0018-mcts.json`, written by the command in
`benches/README.md` §"What `mcts` adds"; the kernel is `benches/kernel/`, in Ply,
and it passes `ply test benches/kernel/ --engine both`. The assumption held on
**shape** and the conclusion it was supposed to license did not hold — see §0,
and ADR 0019 §5 for the write-up and the list of what an amendment to this
document owes.

The reasoning that made it first is still worth having. The spike's fragment is
arithmetic, comparisons, `if`, `let`, `block` and `match` on literal patterns,
and ADR 0016 concluded almost nothing end to end because that fragment is a few
percent of an HTTP request. An MCTS inner loop — select by UCB, increment a visit
count, accumulate a score, compare against a bound — is close to *entirely* that
fragment, so if the ADR 0016 result carried over, the whole ordering below
changed. **It was the cheapest measurement with the highest information, and it
tested the one assumption everything else rested on.**

## 2. Unboxed primitives

**Spent. Both motivating sentences were false and R4 measured them so.** "Every
`Int` is a heap-allocated `Value`" is false — the scalar variants are inline and
building one touches no allocator, so **there is no primitive boxing in this
evaluator to remove**. And the frame count this section reasoned from attributed
to the wrong thing: `interp::literal` cannot allocate unless the literal is a
`Str` or a `Bytes`, and the count was read off a window short enough that
one-time work divided down into what looks exactly like a per-request cost.
ADR 0019 is the ADR that measured it and §4 of that document **rejects** narrowing
`Value`, with the number that would have justified it, which is zero.

**This section's success criterion is spent too.** It read "a `w6_alloc_sites`
re-run showing `interp::literal` gone from the top sites". It is gone, and the
profile's top line is unmoved, because it never was `interp::literal`.

What survives is the interaction list, for whoever revisits a representation
change: `ply-hash` normalization, the store codec and its schema fingerprint,
`--engine both`, and `Value::render`. W2's `Map` landing is the precedent for a
fingerprint bump.

## 3. Handler specialization (evidence passing)

**The gap.** `random.gen()` walks the handler stack. MCTS calls it in the
innermost loop, millions of times. Effects are the language's central abstraction
and they are dynamically dispatched.

**The plan.** Where the handler for an operation is statically known at a perform
site, pass the evidence directly and inline the clause, so the effect disappears.
This is the effect-system analogue of monomorphization, and it is what would make
Ply's defining abstraction free rather than merely cheap.

**Precedent.** Koka does this. ADR 0010 already named evidence passing as the
mechanism that would let a resolution layer elaborate away before hashing.

**How to know it worked.** Time a `random.gen()`-dominated loop with the handler
statically known versus installed dynamically. The ratio is the win.

**Unmeasured:** what fraction of performs in a real kernel have a statically known
handler. If most handlers are installed at an entry point and the kernel performs
against them, the fraction should be high — but that is a guess.

## 4. Unboxed mutable arrays

**The gap.** MCTS wants a flat node array with index-based children, not a
pointer-chasing tree of ADTs. Ply has `List`, `Map` and records — all persistent
or boxed. There is no primitive array with in-place update. Regions give scoped
lifetime, not a contiguous unboxed buffer.

**The plan.** A primitive `Array<a>` with in-place update, region-allocated, with
its brand preventing escape exactly as a cell's does.

**In-place update requires unique ownership, which R1/R2's region work does not
establish.** This section booked that prerequisite as already paid, and it is the
kind of error that travels: **regions establish non-escape, and non-escape is not
non-aliasing.** A region-allocated value can be aliased freely *inside* its
region, and that is exactly the case that makes an update copy — `push` probes
with `Arc::get_mut` at the moment of the update and copies the whole value when
anything else can still see it, and the commonest way a second owner gets there is
`ply_eval::rc::carry`, which hands a pending frame a live clone of the scope
whenever any sub-expression of the enclosing node remains. **That is a positional
fact about the call site and its caller, entirely within one region, and no brand
touches it.** ADR 0017 §4 now says the same thing about itself: it makes in-place
update *available*, not guaranteed.

**It bears on the ranking.** §0.5 calls this the largest lever this ADR still
identifies. An `Array<a>` built on "the regions already did it" would ship the
same dynamic probe under a mutable-looking type, so this item's real cost includes
establishing uniqueness — statically, or by a rule an author can check locally —
and neither exists today. **Until it does, §4 is a larger item than it is priced
as, not a smaller one.**

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
for derivation and which the incremental front end absorbed. Monomorphization
multiplies harder than derivation does. **Measure the definition count and
cold-check time, not just the speedup.**

**Sequencing:** this said "after §2", and §2 does not exist.

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
found the `bank.ply` race in two interleavings with the seed as the repro.
Debugging a nondeterministic parallel tree search in Rust has no equivalent.

## 7. SIMD, layout control, and an escape hatch

**The gap.** Rust MCTS implementations lean on all three. Ply has none.

**The plan: mostly don't.** An `unsafe` escape hatch punctures the effect
system's central claim that the runtime knows what code can do, and ADR 0008 §2
already establishes the honest alternative — a **host handler** with a declared
footprint, a determinism flag, a linearity obligation, and a line in `ply hosts`.
The boundary cost is nothing at kernel granularity.

So: SIMD kernels and layout-sensitive inner loops go behind host handlers, in the
trusted computing base, enumerable in one command. **That is a better answer than
a language-level escape hatch because it is *listable*.**

## The middle path, available today

Write the MCTS kernel in Rust behind a host handler, and the strategy, evaluation
and experiment harness in Ply. That is precisely what W1's boundary was built
for. You get simulation, specs, exact test selection and bisection over the part
of the system where correctness is hard, and native speed over the part where it
is not.

This is not a concession — it is the design working. But it should be stated as
the current answer so nobody discovers it by disappointment.

## Sequencing

**The original list is void in three places and §0.5 is why.** §1 is discharged
twice, by R4 and by R5. §2 was refuted by R4, which also voids the "after §2" in
§5. And "re-price codegen again" was done and produced a number **above** this
ADR's own ceiling.

What the list did not contain, and what §0.5 says is now owed first, is **a
decision about whether a backend is ever reachable from a shipping command.**
That is ADR 0026. After it: §3 handler specialization, §4 unboxed arrays with its
uniqueness prerequisite priced in, then §5 monomorphization and §6 shared state as
its own ADR.

## What would make this ADR wrong

If §1 shows an MCTS kernel is mostly *outside* the spike's compilable fragment,
the ordering above is wrong and codegen is not the endpoint. Rewrite rather than
proceed. **It ran, the fragment was not the problem, and this ADR was rewritten
around §0 and §0.5 rather than executed.**

If §2 lands and allocations do not move, the same reasoning that failed for
regions has failed again, and the next step is another attribution run rather than
the next item on this list.
