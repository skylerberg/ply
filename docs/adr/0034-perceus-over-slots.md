# ADR 0034 — The append cliff is a calculus mismatch: Perceus over slots

**Proposed.** The diagnosis is confirmed on four of the five shapes its gate
holds and narrowed by the fifth; the representation change is unmeasured; reuse
and the checked promise are untouched.

Continues ADR 0024, whose findings it accepts entire and whose sequencing it
re-orders.

> **What this decides.** That the positional rule is **not a language-design
> defect and not a property to be checked, warned about or annotated.** It is
> one implementation decision — ownership tracked at *scope* granularity over a
> shared reference-counted chain — and the fix is to give the machine the
> calculus Perceus is stated over. Paired with a representation whose worst case
> is bounded, **the rule stops existing rather than becoming better
> documented.**
>
> **What it does not decide.** Whether the change is worth its size. That is the
> gate, which is armed and partly answered.

## The defect is one row of a table the pass already prints

The reference-counting module's own header maps Perceus' operations onto Ply's
and has a fourth row the calculus does not: the *carry*, *because a frame
holding a scope it will not read is an owner Perceus' calculus has no name for.*

**That row is the field-order rule.** Perceus is stated over stack slots; Ply
runs it over a persistent chain that a closure, a continuation frame and the
current evaluation share by pointer. **Because the unit of ownership is
*everything in scope* rather than *this binding*, a frame that will never read a
record still holds it, and whether one of its fields is at one owner is decided
by what syntactically follows it.**

Three consequences, all already in the record: **the rule as documented is too
weak** — a program that obeys it is quadratic anyway when the literal is not last
in the *call*; **a lint over it is a partial oracle**, built and refuted; and **a
mode on the arrow is checkable-and-useless or useful-and-uncheckable**, measured.

ADR 0024 answered the last two correctly and then treated the residue as a cost
to be *reported*. **The disagreement here is narrow and is the whole of it: the
residue is not a cost to report, it is a representation to replace**, and those
two failed because they aimed at a language-level property the defect does not
live at.

## What the surveyed languages do, and the line all of them hold

Survey, not measurement — API and literature. Four readings, and the third
decides this.

**Invisible constant factors are ubiquitous and tolerated; invisible asymptotes
are shipped by nobody.** Go's escape analysis is genuinely unpredictable and
Go's answer was a reporting flag — the precedent for a cost report. **But escape
analysis decides heap versus stack, a constant. A reporting flag is proportionate
to an invisible constant and is not proportionate to an invisible complexity
class.** That is why ADR 0024's cost report is a good instrument aimed at the
wrong tier.

**Every solution is one of two families and there is no third.** Either
ownership is visible and checked, or cost is independent of ownership. **Koka
does neither and gets away with it by making the *count* precise.**

**Ply cannot take Rust's route, and multi-shot is why.** Rust's ownership works
because there is exactly one continuation. Under a twice-resumed clause one
syntactic occurrence legitimately yields two uses — a semantic fact, and what ADR
0024 measured. **Ply chose effects; effects cost you Rust's answer.** It cannot
take Go's either: Go lets hidden aliasing corrupt *meaning* rather than cost, and
ADR 0017's governing property forbids that trade. **Koka's is what is left, and
it is the one designed for a language with handlers.**

**The in-house proof.** `Map` is a persistent tree, so an insert costs the same
whoever holds it, **and it has produced no rule, no lint, no record and no
paragraph in the guide.** `List` is a flat array behind a count and has produced
all four. **Same language, same evaluator, same effects, same multi-shot
handlers. The only difference is that one container has a cliff.**

## Koka's mechanism, in four parts

Summarised from the literature to be checked rather than trusted.

**An ownership-passing IR.** Every binding is owned by exactly one place and
consumed once along every path. Two properties, both stronger than scope-based
release: **drops land at the last use, not at scope end**, and **ownership is
per-variable — nothing owns "the scope".** The second is the row the pass has no
name for.

**Borrowed parameters** mark callees that do not consume. This is the *weak* half
of the mode dilemma — *the callee does not keep this* — the half ADR 0024 was
right to say does not buy the append. It buys reference-count traffic, a constant
factor.

**Drop-reuse.** A pattern match yields a **reusable memory token** when the count
was one, and a later constructor of the same size allocates *at* that token. So
map, filter, reverse and a tree insert over uniquely-owned data allocate nothing.

**Ply's in-place append is not this.** It is a uniqueness probe on a vector —
Swift's mechanism. **Perceus reuse recycles a *dying* value's memory into a
*newly constructed* one, which covers a class Ply does not touch at all**: a
record update expands at parse time to a full field list, so at runtime it builds
a fresh record and nothing recycles the dead one. The attribution work puts the
bulk of request-path allocation in value construction, **so this is aimed at the
measured profile and not only at the append.**

**How it survives multi-shot, which decides whether any of this is importable.**
Koka distinguishes clause forms *statically*: a tail-resumptive clause is a direct
call with no capture; a general clause captures for real and counts honestly, so
under a twice-resumed clause the count is genuinely two and **the copy is
*correct* — two futures need two lists.**

**So ADR 0024's sentence — *any perform in the enclosing dynamic extent puts a
second owner on the value* — is not a fact about effect handlers. It is a fact
about handlers that capture.**

## Decision 1 — the environment becomes slots

Replace the persistent chain looked up by name with **flat frames of slots
resolved at lowering**: every variable a slot index, every binding a computed
last use, ownership per slot. Closures capture their **free variables** rather
than the chain.

Traced against the shapes that refuted the documented rule: a projection that is
its binding's last use *moves* out of its slot, which is then empty, so a later
sibling reads a different slot and the pending frame holds nothing that reaches
the list. **A parameter is a slot like any other, so if the caller passed *its*
last use the value arrives at one owner — the chain composes across calls with no
annotation, which is why Koka needs no ownership in its surface types.**

**Measured by a probe rather than argued: four of the gate's five pairs go to a
gap of zero**, including the case that refuted the documented rule.

**The fifth does not move, and the reason narrows the claim.** Its pessimal
spelling puts the growing field first and a later field genuinely reads the
record. **No release keyed by a *name* can free the field there, because the
record is not dead — only the field is.** That is **path-granular liveness**, the
general form ADR 0024 records as declined **because a wrong answer there is a
wrong program rather than a slow one.** So one slot per binding removes position
dependence wherever the enclosing node's other sub-expressions do not read the
same binding; **where they read a different *field*, the slot has to be finer
than the binding** — which puts the flat record representation *upstream* of
finishing this rather than after it.

### Every alternative is now priced, and none is cheap

**This subsumes ADR 0024's three evaluator changes rather than competing with
them, and the size of the constant that separates them is measured.** Both
primitives that ADR offered for narrowing a carried scope **double the
allocations on the request path.**

The reason is the same for both: **the cost is inherent to building a narrowed
scope at runtime out of a persistent chain.** Releasing pays one link per binding
above the deepest it drops, which for a parameter is the whole scope. Building up
from empty pays one per live name — **and the live set that is *safe* includes
everything read after the call as well, because a name read both later and by a
remaining argument is still the frame's to hold. Narrowing it to just the
remaining arguments' reads is what turns a legal program into a lookup failure,
which is how that version was found.**

**Narrowing only where it can pay is fifteen times cheaper and still not free** —
same benefit for a fifteenth of the cost, and two attempts to sharpen it further
both bought less. **It still costs a few percent of the request path, and the
window has to be free rather than cheap.**

**And the frame's narrowed view has to be inline, which is what forces the rest
of the rewrite.** Modelled over scope depth: today's carry is *free* — one
refcount bump — so **every scheme that narrows the view is a cost on the common
path, and only the inline one is free again.** That settles the shape: the window
lives in the frame, not behind a pointer.

**It also settles that there is no cheap first step.** An inline window is only
usable if the sub-expressions reading it resolve names to *indices into it*,
**because a window that has to answer lookups by name is the chain again.**
Indexed lookup *is* the slot rewrite. Pooling does not rescue an intermediate
design either: a carried scope lives as long as its frame, so deep recursion
holds every link at once and outruns any free list. **The rewrite is the unit of
work — and if the IR is changing anyway, change it to slots and get a
constant-time answer instead of one linear in depth.**

**The trade, counted rather than assumed.** A persistent chain makes *capture*
cheap; a slot stack makes *carrying* cheap. The second is only worth having if
programs carry far more often than they capture, **and that is a property of real
programs**: over the corpus, twelve carries per capture, with the average capture
a few frames deep. **A test counts it and fails if the ratio ever inverts, which
is the condition under which this stops being worth its size.**

**The shape the measurements leave standing.** With a slot stack, "the frame
carries less" is not a construction at all: **the slots stay where they are and
the carry *clears* the dead ones.** That is writes and no allocation, which is
the only option that matches today's free clone.

**The cost is owed for a second reason.** A name-keyed walk over a shared chain
is not a runtime a bootstrapped compiler can keep, **so the representation is on
the bootstrap critical path for reasons independent of the append.**

**What does not change.** The uniqueness walk and the runtime probe both stay in
release builds; the analysis may be wrong, and when it is the program is slow and
never incorrect. **The ownership hint is not promoted to a permission.**

### What the machine has to become

Nine measurements narrow this to one design, and **it is not the "frame carries a
narrowed scope" that every attempt kept reaching for. That framing is what made
each of them cost something. A window is an allocation however rarely it is
built.**

> **The frame should not narrow anything. A last use should move the value out
> of its slot.**

That is Perceus' rule, and the existing take-unique walk is already the attempt
at it — **it fails today for a reason that is now precisely stated: it refuses at
the first shared link, and a pending frame has cloned the chain's head, so every
link is shared and it refuses every time. The chain is what makes sharing
all-or-nothing.**

With slots the same rule is a write. **The machine owns one slot stack** and an
activation is a window into it; nothing per-scope is allocated. **A frame records
a base index, not a scope**, so carrying is free and *narrowing disappears as an
operation* rather than becoming cheap. **A read marked owned takes the value out
of its slot, leaving it empty** — and the pending frame sees the empty slot too,
which is correct, because *owned* is exactly the claim that nothing after this
point reads the binding. **A closure copies its free variables** out of the
window. **A capture copies the windows it took**, and each resumption restores
them — the cost this design pays, and the carry-to-capture census is what says it
is affordable.

**A flat per-barrier index is not yet a runtime address, and this is the first
thing to fix.** A handler clause's body is a barrier to the lowering, so only its
own binders are ownable inside it — but at runtime the clause's scope is the
prompt's environment *extended* with the clause's parameters. **So the two passes
agree with each other and disagree with the machine.** Two ways out: give every
occurrence a depth and an index, or **make a clause body a real frame that copies
in the free variables it needs, exactly as a closure does.** The second is built,
behind the probe, and keeps addressing flat; **it is behind the probe rather than
on because the allocation gate is a gate and it is an increase.** What it removes
is the structural obstacle.

**What makes the move work is the machine owning the stack, and only that**: a
value can be moved out of a slot exactly when the array has one owner, and the
array has one owner exactly when it is the machine's rather than each frame's.
**That is why the pieces do not land separately.**

**Run the resumption-scope audit first, and read its note before writing any of
this.** It was written before the change for that purpose, **and it exists
because the suite had a hole exactly here: every resumption test asserts what a
*cell* holds, and none asserted that a resumption re-enters with the *bindings*
it captured — because a persistent chain gives that for free and there was
nothing to get wrong.** Its four probes are the four ways a slot machine takes it
away: a binding whose last use *follows* the capture and is resumed twice, where
**owned means "no later *code* reads this", which is not "no later *execution*
reads this" once a continuation resumes more than once**; a binding read across
many intervening activations, where a machine-owned stack reuses the indices the
captured frames named; a binding and a cell in one expression, where **restoring
too much reddens one assertion and too little the other, so only the threaded
reading passes**; and nested captures over overlapping windows.

**One caveat the file states about itself**: the first probe's own assertion
cannot fail on the chain, because a continuation holds an immutable copy nothing
can empty. **What arms it there is a pin on the move counter — the mechanism
rather than the answer — and it is a tripwire.**

## Decision 2 — the worst case becomes bounded

Even with a perfect count, **the count is honestly two sometimes**: a capturing
resumption, the memo table, a real alias. Today that costs a whole-array copy and
**the penalty against the good case grows without bound.**

The list becomes a **chunked persistent vector with an in-place fast path when
unique**. Unique appends stay a push into a small array; shared appends become
logarithmic.

ADR 0024 priced this and it failed two of four pre-registered criteria. **Two
corrections to how that result should be read, neither of which moves the bar it
failed.** The measurement was re-taken against both candidate families, and the
one that allocates once per append is refused. And **the gate's shape is wrong**,
which is a disagreement rather than a re-measurement: it read *take the
representation change if the analysis fails*. **They are not alternatives — the
analysis makes the common case free and the representation makes the uncommon
case bounded, and no amount of the first removes the need for the second under
multi-shot.** Re-posed as a property of the language: **no core operation may
have a cost ratio that grows with *n* on a property the source does not show.**

### The instrument is the larger half of this change

The in-place counter answers *did this append copy the whole list*, **which is
the right question only while a copy is all-or-nothing.** A chunked append copies
a *path*, so the boolean would read false for something costing a logarithm and
**the rate would look uniformly bad while the program got faster.** So a counter
of what was actually copied lands first — the question that survives the
representation.

**And that is not sufficient, which is worth stating precisely because it looked
sufficient.** The volume counter is computable today because the copying arm
knows the length it copied. **The candidate vector exposes no such thing**: its
only sharing-related method compares two vectors rather than asking whether one
is uniquely owned. **So the assertions reading those counters are not made
*vacuous*, they are made *unmeasurable* — and the difference matters: a vacuous
assertion can be re-pointed at the same quantity, an unmeasurable one has to be
re-pointed at a different one.**

What survives the swap is what the allocator saw, **and in *bytes*, not in
allocation count.** A whole-list copy is one allocation, **so a quadratic
accumulator makes the same number of allocations a linear one does while moving
quadratically many bytes**: counting allocations does not separate them, and
counting bytes gives the four-against-two signature per doubling that names the
shape.

**And the template does not carry to the standard library, which is where most
of the sites are.** The synthetic pair works because both sides pay a near-zero
fixed cost. A probe that imports a real module charges megabytes of module-level
and memoised work that does not scale with it, **and two runs are not independent
— the second is measured against a warm memo and a warm interner.** Doubling the
subject read essentially flat, and the byte count *fell* between two sizes,
**which is not a quantity a ratio can be taken of.**

**That is one site of forty-five, and counting them changes what this is.** The
copy counters are read forty-five times across eight files, **and the whole
ownership-checking edifice is built on *did this append copy the whole list* —
including the checker's own oracle, which judges its verdicts against exactly
that counter.**

**So this is not a representation change with an instrument problem attached. It
is an instrument change with a representation change attached**, and the
instrument half is the larger one: **the checker's notion of a correct verdict
has to be restated in a quantity that survives, before the representation it
describes can move.** This record has called that step "ready" or "mechanical"
more than once and it has not been either time.

## Decision 3 — reuse, and the representation it needs first

Adopt drop-reuse for constructors and record literals. **The prerequisite is a
flat record representation** — reuse recycles cells of known size and shape, and
recycling a tree is neither easy nor worth much. Record types are structural and
already printed sorted, so the layout is statically known wherever the type is,
**and this is independently a win.** The fifth gate pair lands here too:
field-granular liveness needs a field to be addressable.

## Decision 4 — a checked promise, callee-side

ADR 0024's surviving requirement — that the absence of reuse become visible where
an author cannot miss it — is met by a **callee-side obligation, not a promise
about callers**: a function so marked fails to compile if it could allocate.

**This escapes the mode dilemma structurally: it never states anything about the
caller, so the multi-shot counterexample does not reach it.** Opt-in and scoped
to the standard library's hot paths.

## Decision 5 — a tail-resumptive clause is not a capture that outlives a region

**Done.** Region inference called every region holding a tail-resumptive clause
shared. **Two rules from ADR 0005 are why it need not**: a general clause binds
the continuation as a *value* the body can store, close over or return; a
tail-resumptive clause binds nothing and the continuation goes into the stack as
a frame above the region's close, **consumed before it, reachable from no
binder.**

**The case analysis is the condition on landing it**, since freeing memory a
continuation still reaches is a wrong program rather than a slow one. The
continuation reaches no binder, because the tail-resumptive rule is the only one
that builds that frame and nothing in the body can name it. The frame runs before
the close, because it sits on the stack below the handler's prompt and the region
closes below that. **A clause body that escapes is a different cause and is still
counted** — which is why the tail-clause cause is its own slot rather than a
filter, since the first-cause tally would otherwise *hide* it. And every other
route out is already its own cause.

**It is worth zero regions on this corpus, and that is the finding.** Every
region has another, independent reason to be shared. **The estimate that said
otherwise came from reading a first-cause tally as a lower bound** — the tally
keeps only the first cause in source order, **so a row of it is an *upper* bound
on what relaxing that one rule would move, which is the kind of error a re-take
cannot catch because every figure in it stands.**

**This was sequenced first because it is cheap, and being cheap is what let its
estimate be refuted before anything expensive was built on it.**

### Why releasing a parameter releases nothing still read

ADR 0024 made writing this down the condition for landing the parameter fix. The
filters are unchanged and a parameter has to clear all of them. Five routes are
not direct reads, and each keeps the name live: **captured by a closure, a
handler clause or a simulated body** — free variables are replayed into the
enclosing set as reads *at the construct that captured them*, never last ones;
**stored in a cell**, where the value is then the arena's to release rather than
this binding's; **read in a later match arm**, walked before any statement to its
left; **read in the tail**, lowered first; and **shadowed by an inner binder**,
where the filter names the shadowing binder instead.

Only parameters are seeded, because a name bound in a sibling block is not in
scope here at all. **The failure mode is a released slot reaching an internal
error on a legal program — loud, but a new way to reach a diagnostic whose point
is being unreachable.** All five routes are run as a test.

## What this does not do

**No parameter mode, ownership row, or surface annotation** — settled on a
measurement, and the callee-side obligation is not a counterexample because it is
an obligation on a body. **No linear or uniqueness types**: they conflict with
multi-shot handlers, since a linear value captured by a twice-resumed
continuation is used twice, **and Koka is the existence proof that the
performance does not require it.** **No second lint.** **The hint is not promoted
to a permission.** **The cost report is not made unnecessary** — but afterwards
it reports a *residue* rather than a rule, which is the tier a reporting flag
fits, and it prints *unknown* rather than rounding, **because four shapes cannot
be decided from one body and rounding them is the one thing that would make the
checker worse than not having it.** **And no byte quadratic is touched.**

## The gate, registered before the measurement

**Position invariance** is the central claim and the only one that can kill this
early: five paired programs, each pair computing the same value with the growing
sub-expression last and not-last, **counted rather than timed**, with both bars
in code. **Red on the shipped evaluator and armed by having been seen red.** A
second test pins today's numbers and a third **stops the corpus being narrowed —
it exists because a member was once written into the file and never referenced,
leaving four shapes reported as five.** *Four of five meet it under the probe.
What that has already done is separate a confirmed claim from an over-broad one
before a milestone-sized rewrite was started.*

**The corpus gate has already fired once.** Per-module in-place rates above a
bar, no module regressing, **and the request-path allocation report not
increasing** — ADR 0017's lesson is that this is the number a milestone of this
shape moves the wrong way, **so it is a gate rather than a report.**

**The representation gate is a property, not a ratio**: the shared-to-unique cost
ratio flat in *n* across at least two doublings, and the index cost within a bound
**measured through the backend**, per the warning that dispatch would otherwise
hide the term.

### The representation gate is answered, and the corpus gate is what decides it

Both candidates were priced before anything was built. **The property holds for
both and fails for what ships**: today's ratio grows without bound and both
candidates are flat. The index term is negligible where it was warned to measure
it.

**The gate as registered selects one candidate, and that candidate is
disqualified — by the other gate.** It allocates **once per append**, which is
the spine-node cost ADR 0024's fallback predicted and set against itself.
Allocations per request is what the corpus gate bounds, **so it cannot land
whatever its time ratio is.** The other candidate costs an order of magnitude
more allocations than a flat vector in the relative reading and about a hundred
in the absolute one, **while using fewer bytes, because doubling over-allocates
where a chunked spine does not.**

**The gate is not being moved to fit this.** The property gate asked for a
*ratio* and got a true answer to that question; **what it never bounded is the
absolute cost of the good case, and that is not a new bar — it is the corpus
gate, already registered, already the thing that caught the parameter fix.** Read
together they admit one candidate and refuse the other, **and the lesson for the
next gate written here is that a ratio criterion needs a level criterion beside
it or it will select whatever makes the common case uniformly bad.**

**Still unmeasured, and it is what the representation change turns on**: whether
the extra allocations per large list show up on the request path at all.

## Sequence

Done: arming the gate; the tail-resumptive refinement; wiring the cost report;
the carry probes; pricing both fallback primitives; flat closure conversion; slot
resolution verified against the names, **so the assignment the rewrite switches
to is wrong-checked first**; clause bodies copying in their free variables; and
the volume counter.

**The parameter fix fails the corpus gate, and what it buys is larger than the
record first said.** Landed by default it costs the request path a few percent.
What that buys, on the corpus, is in-place appends going from two-thirds to
nine-tenths — **ten thousand list copies that do not happen. The cost and the
benefit are measured on *different corpora*, and that is the whole tension**: the
request path pays the release without pushing the lists that would repay it. The
cause is the one ADR 0024 predicted: **releasing rebuilds every link above the
binding, and a parameter is the deepest binding in its barrier's chain, so
releasing one rebuilds the whole scope.** Whether that trade is worth taking is
not something the gate alone answers, **and it is recorded with both numbers
rather than only the one that fails.**

Next: the flat record representation, **ahead of the slot rewrite rather than
behind it**, because the fifth gate pair needs it. Then slot frames, gated on
both gates. Then **the other forty-four references to the copy counters, which is
the larger half** of the representation change. Then the chunked vector, then
reuse, then the checked promise.

**The guide's statement of the positional rule is deleted, not corrected, when
the gate is green. That is the test of whether the rule is gone: a rule that
still needs stating is still there.**

## What would make this wrong

1. **If the four confirmed pairs regress under real slot frames**, or if the
   fifth proves unreachable without the path-granular analysis ADR 0024 declined
   on soundness grounds. Then the diagnosis does not carry the whole rule and the
   representation change is what survives.
2. **If slot frames cost more on the hot path than they save.** The allocation
   report is the instrument, it is a gate, **and ADR 0017 is the precedent for
   this exact failure.**
3. **If flat closure conversion is unsound against multi-shot resumption.** A
   closure capturing free variables rather than the chain changes what a resumed
   continuation can reach, **and none of the three suites that would show it is
   currently written against a flat closure.**
4. **If the tail-resumptive refinement is unsound.** The failure mode is freeing
   memory a continuation still reaches — **a wrong program, not a slow one, and
   the only item here that is not cost-only.**
5. **If a second evaluator returns.** There is one engine, so nothing here can
   diverge between two — **which also removes the differential that would have
   caught a lowering change altering an answer.**
6. **If the request path is bytes-bound rather than list-bound.** ADR 0024
   counted the concatenation sites and two documented quadratics. **Not measured,
   and it should be measured before the rewrite.**
7. **If the size estimate is wrong by the margin this record's estimates usually
   are.**

## Provenance

Every figure attributed to ADR 0024 is quoted and was **not** re-taken. What was
measured here: the gate's five pairs with and without the probe; module in-place
rates before and after the parameter fix; and the region split before and after
the tail-resumptive refinement. **All of it is in the tests named above rather
than only here, which is the point of arming it.**

**The survey is literature and API, not measurement**, and the mechanism summary
is three published papers, summarised to be checked rather than trusted. The
later decisions carry no measurement of their own.
