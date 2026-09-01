# ADR 0018 — Closing the gap for compute kernels

**Proposed. No decision here is accepted.** Every section states what is
measured, what is assumed, and the measurement that would settle it — because
ADR 0017 asserted an unmeasured premise, called it forced, and cost three
milestones.

## Context

The motivating question: could a maximally-performant Monte Carlo tree search
library be written in Ply rather than Rust? MCTS is a good probe because it is
almost pure compute — a tight loop over a mutable tree, millions of iterations,
hot RNG, parallel rollouts contending on shared nodes. **It exercises every
place Ply is currently weak and almost none of where it is strong.**

The answer today is no, by roughly an order of magnitude.

**What is assumed and not measured**, and everything below was ordered on it:
that a compute kernel is mostly inside the compilable fragment, where an HTTP
request is not. Section 1 existed to test that assumption before anything was
built.

## 0. The assumption held and the plan failed anyway

**The fraction came back high — most of the kernel's executed work is inside the
fragment, against a few percent for an HTTP request — and codegen still bought
nothing.** End to end, every compiled set from "entry only" to "everything
accepted" landed inside the method's own noise floor. The kernel ratio on the
part that is wholly inside the fragment is enormous, **and reporting *that*
would misprice the decision in exactly the way the performance verdict warned
about.**

**Why a high share and a large in-fragment ratio still bought nothing: the
interpreter could not call compiled code.** Only compiled-to-interpreter
existed. Every function the fragment accepts whose *callers* it refuses is
compiled and then never entered. **It is not the boundary cost** — the crossings
are a rounding error of the run.

So the binding constraint is **architectural — one-way calling — rather than the
size of the fragment or the cost of the boundary.** That is a different first
milestone from anything on the list below.

### The refusal census is not a work list

The ranked table of refused constructs is accurate and the reading it invites is
wrong by more than an order of magnitude. It is a **first-refusal** census — one
construct named per refused function — and two things break the arithmetic:
**a refusal names one construct, not all of them**, so admitting the top one
moves a function to the next row rather than into the fragment; and **the
fragment must be closed under calls**, so a function is admitted only when every
construct in it *and every function it reaches* is admitted.

Measured one item at a time: the top-ranked row delivered one function and
changed nothing about what executes. **The rows are not four wins; they are one
closure, and the whole of it arrives on the last item** — plus a fifth construct
that appears in no row at all, because it sat behind another and therefore never
got to be a first refusal. With all of them in, the fragment takes every
function and every node of the kernel.

**What did not move, and bounds all of it:** the map, record and list machinery
— a fifth of executed work — is still outside the fragment and untouched.
**Widening changes *which functions compile*, never what a map insert costs.**

Two hazards belong to the fragment rather than to any function, so they appear
in no census. **No nested-call bound**, since closed: every compiled function
carries a fuel prologue seeded from the budget it is handed, and a body that
would pass it declines so the machine raises its own diagnostic. **It is not
free** — the machine re-offers the same function at every interpreted depth and
each attempt burns its whole remaining fuel first, so a runaway program is
orders of magnitude slower with a backend attached. Bounded, paid only by a
program that is about to die, and recorded rather than fixed. And **no
floating-point path**: the fragment compiled addition as integer arithmetic
whatever the operands were. Closed at the boundary rather than by building a
path, and **the reason is scope, not difficulty** — a real float path widens the
agreement surface by NaN, signed zero, float equality and decimal precision, and
the workload the fragment is being widened for is field accesses and list
patterns. **What is still open, and a reader will assume it is closed:** only
the literal-shaped case is refused, so a float arriving as a *builtin's return
value* is refused by nothing and declines at run time — a slow answer rather
than a wrong one, asserted as an exact decline count so a lowering that ever
*answered* one goes red.

## 0.5. Entry was built. It beat this record's own ceiling, and the ceiling was wrong.

Pre-registered before any number existed. Agreement was taken before anything
was timed — thousands of generated cases and whole-kernel searches, zero
disagreements, against both the no-backend machine and the independent
tree-walker.

**What was built** is a seam taking a name, some scalars and a call budget and
returning at most one scalar. No arena, no stack, no handler stack, no host
binding, no mutable machine, **no route back — so a backend that cannot finish
has changed nothing observable and declining is free by construction.** The hook
is one branch on every interpreted call.

**The cost side, on the workload that ships: zero allocations per request**, by
the pre-registered rule, comparing the shipping tree against the same tree with
the call site deleted, arms alternated, byte-identical at every window size. In
the linked binary the hook is eighty bytes of machine code.

**Zero allocations is not zero cost, and the load-independent count says so: one
request reaches the hook hundreds of times, and every one is a miss. The wall
clock of those branch tests was never taken**, on either binary, although both
existed. The currency was pre-registered as allocations and a wall-clock rule
was not; **this should not be read as if it had been.**

**The kernel ratio.** The ladder is monotone in entries, the aggregate is
filter-independent, and both pre-registered controls sat inside their band. **No
window was dropped by either pre-registered filter on any rung, which also means
neither filter was ever tested.** Pre-registered verdict: entry paid off.
Pre-entry the same rung was worth nothing, with zero entries. **The diagnosis
that the binding constraint was architectural is confirmed.**

**The figure has never been re-taken on the pinned code generator, and the
reason is a blocked instrument rather than a busy machine.** The port to an
older version was pre-registered with this section's command, statistic, filter
and controls taken over verbatim, and then could not be run: the harness
verifies agreement before it times anything and the agreement corpus is red.
**The red is not the port's** — it reproduces on the newer version from
unmodified source, and the ported build's agreement output is byte-identical to
the unported one. **Nothing here should be read as evidence that the two
versions generate equally fast code; it is evidence that they generate the same
*answers*, which is a different claim.**

A shipping-command number on this kernel now exists and **is not this one, and
must not be quoted as a re-take of it.** Four differences, each widening the
denominator: it measures a whole test invocation — front end, hashing, cache
check, harness — where this measures the kernel body alone; it pays compilation
inside the window; it runs different iteration counts; and it enters more often.

### Beating its own ceiling is a defect in the ceiling

**A result that beats its own predicted ceiling is a sign that the model was
wrong, not that the result is extra good**, and the model was wrong in a way
this can name. Interpreted, the search offers tens of thousands of calls to the
hook; with everything compiled it offers a small fraction. **The rest stop
existing, because they now happen inside a native body.**

The ceiling's denominator priced each function's **body** in isolation, which
charges the call-site machinery to an unattributed bucket rather than to any
function. **The executed-work share is a body-only accounting, and entering
compiled code deletes the call-site machinery too**: the argument vector, the
frame push, the environment binding. **So the ceiling is an artifact of a
body-only attribution, not a bound, and it is withdrawn as a bound.**

**The same fact turned up as a correctness defect from the other side.** A
compiled body pushes one call frame for a whole call; the interpreter pends a
frame per pending operand as well. That is the unattributed machinery, counted.

### What entry also bought: a divergence, with no mutation

A deeply recursive body accepted whole by the fragment raised on the machine
alone and **answered** with the backend attached. The seam passes only a call
budget, and the machine's *second* bound could not be expressed at the boundary,
so no backend could honour it.

**Fixed by deleting the second bound rather than widening the seam.** The frame
count was a resource guard on the machine's own heap phrased as a program
answer, and it was sensitive to **how a body's operands were spelled** rather
than to what the body did. It survives as an opt-in ceiling, and a machine
holding one enters no compiled body — **so nothing this seam admits can turn on
a limit it was not handed.**

Two more the reviews found. Every entry cost proportional to the *previous*
entry's peak arena — the real mechanism behind a per-function regression that
had been blamed on a filter — fixed by clearing the arena at the end of the
entry that filled it. And a definition that discharges its own effects publishes
an empty row, so the purity gate cleared it — **fixed by publishing a second
fact rather than a second row**, transitive over the call graph.

### Nothing here ships, and that is the load-bearing sentence

**No shipping command can install a backend.** The two-engine comparison cannot
attach one and therefore catches **none** of the deliberately wrong backends the
mutation harness runs. The rule that a run with a backend attached is a third
execution strategy whose results the cache must not keep is stated in the seam
and is **not enforced, because it is unreachable.** **So none of this is
available to any user of Ply.**

### Is the list below correctly ordered?

**No, and not for the reason the ceiling gave.** What replaces it is worse for
the list, not better.

1. **The first milestone is discharged as an experiment and not as a feature.**
   Entry works, it pays on this kernel, and it is unreachable from every
   shipping command. Making it reachable is a decision of its own (ADR 0026).
2. **Unboxing is spent**, corrected in place below: there is no primitive boxing
   to remove.
3. **Evidence passing is unpriced by this measurement and must not be read as
   re-ranked.** The kernel declares **no effect at all**, so no entered
   definition ever exercised a handler, the purity gate or the effect path.
4. **The map/record/list machinery is the largest lever still identified**, by
   the unchanged finding above — but that is inference from a pre-entry
   attribution, not a new measurement, and it should be re-taken on a hybrid run
   before anything is built on it.

**The honest summary: the ordering was derived from a model that undercounts
what entry removes, and every remaining item is still priced by that model. No
lever below has an end-to-end price on a kernel that can enter compiled code.**

### What a reader still does not know

The wall-clock cost of the hook on the request path. What a backend costs in
allocations — **no allocation figure was taken with a backend attached
anywhere.** Whether the ratio holds anywhere else: one kernel, one program, one
box, one pre-registered run whose pre-registration forbade re-running. What
compile time costs — it is in no ratio. **Anything about the seam under the rest
of the language**: effects, handlers, continuations captured across an entry,
simulation, secrets, non-integer types, closures, the store, the test cache, the
prover and the host path were not exercised with a backend. Whether the
fragment's own lowering is correct — instruction selection was read and never
mutated, and every wrong backend built corrupts *answers*, not lowering. **How
strong the agreement result really is**: most compiled functions are offered
zero times during whole-kernel searches, and a uniform off-by-one in the scoring
function changes no move in any of them. And **whether the oracle is right**: if
the two engines were wrong in the same way, every comparison here agrees.

## The levers, and what is spent

**Unboxed primitives — spent. Both motivating sentences were false and were
measured so.** "Every integer is a heap-allocated value" is false: the scalar
variants are inline and building one touches no allocator, **so there is no
primitive boxing in this evaluator to remove.** And the frame count reasoned
from attributed to the wrong thing — a literal cannot allocate unless it is a
string or bytes, and the count was read off a window short enough that one-time
work divided down into what looks exactly like a per-request cost. What survives
is the interaction list for whoever revisits a representation change:
normalization, the store codec and its fingerprint, the differential oracle, and
rendering.

**Handler specialization (evidence passing).** Effects are the language's
central abstraction and they are dynamically dispatched; where the handler for
an operation is statically known at a perform site, pass the evidence directly
and inline the clause, so the effect disappears. **The effect-system analogue of
monomorphization, and what would make Ply's defining abstraction free rather
than merely cheap.** *Unmeasured:* what fraction of performs in a real kernel
have a statically known handler.

**Unboxed mutable arrays.** MCTS wants a flat node array with index-based
children, not a pointer-chasing tree of ADTs. **In-place update requires unique
ownership, which the region work does not establish** — this item booked that
prerequisite as already paid, **and it is the kind of error that travels:
regions establish non-escape, and non-escape is not non-aliasing.** A
region-allocated value can be aliased freely *inside* its region, and that is
exactly the case that makes an update copy. **So this item's real cost includes
establishing uniqueness — statically, or by a rule an author can check locally —
and neither exists.** Until it does, **this is a larger item than it is priced
as, not a smaller one.**

**Monomorphization.** Content addressing makes the bookkeeping natural — a
specialization is a definition with its own hash. **The cost to watch is
definition-count inflation**, which derivation already flagged and the
incremental front end absorbed; monomorphization multiplies harder. **Measure
the definition count and cold-check time, not just the speedup.**

**Parallel rollouts and shared mutable state — a design gap rather than an
optimization.** MCTS wants many workers mutating shared tree nodes, and the
region brand refuses exactly that. **This needs its own record and should not be
improvised.** The shape worth exploring: an explicitly declared shared region
where the isolation guarantee is *deliberately* relaxed and the relaxation is
visible in the type. **What must not happen is a silent relaxation** — the
pooled-connection defect coupling two tests the footprint graph believed
disjoint is the same hazard with a different name. **Consolation for the
interim:** seeded rollouts replay exactly and footprint-guided reduction finds a
race in two interleavings with the seed as the repro. Debugging a
nondeterministic parallel tree search in Rust has no equivalent.

**SIMD, layout control, and an escape hatch — mostly don't.** An unsafe escape
hatch punctures the effect system's central claim that the runtime knows what
code can do, and ADR 0008 already establishes the honest alternative: a **host
handler** with a declared footprint, a determinism flag, a linearity obligation
and a line in the listing. The boundary cost is nothing at kernel granularity.
**That is a better answer than a language-level escape hatch because it is
*listable*.**

## The middle path, available today

Write the kernel in Rust behind a host handler, and the strategy, evaluation and
experiment harness in Ply. **That is precisely what the boundary was built
for**: simulation, specs, exact test selection and bisection over the part of
the system where correctness is hard, and native speed over the part where it is
not. **This is not a concession — it is the design working — but it should be
stated so nobody discovers it by disappointment.**

## What would make this wrong

If a compute kernel had turned out to be mostly *outside* the fragment, the
ordering would be wrong and codegen would not be the endpoint. **It ran, the
fragment was not the problem, and this record was rewritten around that rather
than executed.**

If unboxing had landed and allocations had not moved, the same reasoning that
failed for regions would have failed again, and the next step would be another
attribution run rather than the next item.
