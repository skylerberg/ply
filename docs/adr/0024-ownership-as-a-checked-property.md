# ADR 0024 — Ownership: the cliff, not the count

**Accepted as a direction. Its central mechanism was proposed, built against
four times, and declined on a measurement.** This merges the direction (0024)
and the design round that answered it (0025).

**What is decided.** That whether a value is uniquely owned stops being a
runtime accident the evaluator discovers and becomes **a property the compiler
reports and a test can falsify.** **What is declined** is the form that was
argued hardest for: a mode on the arrow, published in the signature. **What is
not decided**: the list's representation, which is a priced fallback behind a
gate fixed in advance.

**No measurement was taken of the direction itself, and the reason is that the
thing to measure is a language that does not exist yet.** What was measured is
that the alternative — predicting the cost rather than checking it — **was built
and refuted**, and that the design space is narrowed by an argument rather than
by a benchmark. *Narrowed*, not closed.

## The defect

A list is a flat array behind a reference count, so an append has two cases and
no third: **sole owner, push in place; anything else, copy the whole array.**
Which case a program gets is decided by whether anything else holds a reference
at that instant, **which is decided by where the call sits in its enclosing
expression**, and it **composes across function boundaries** — a correctly
written callee is made quadratic by its caller, **and the trailing
sub-expression that costs the copy can be a literal constant**, because the
carry rule never asks what the remaining sub-expression *reads*.

**It shipped.** The JSON string escaper was quadratic in the number of escapes,
**on client-influenced input.** Measured three times by three parties, **and
found by a spike rather than by the loop.**

Measured per module over each module's own suite, the in-place rate is above
98% for three modules and **under one percent for the two on the request
path. The two modules that reuse nothing are the two that serve requests.**

Two properties of this defect matter more than the defect. **Nothing in the type
says it** — not the signature, not the effect row, not the spec — so a reviewer
reading a specification, which is what a human is meant to review, cannot see
it. **And nothing a model can observe says it either. An agent writing Ply
cannot see a refcount. It can see a type.**

### Where the copies actually are

Attributed to source lines by instrumenting the copying arm with its span.
**This is the measurement the design turns on and no proposal had it.** The
largest blocks are an accumulator threaded as a **parameter**; a growing field
written **first** in a record literal; a projection out of a record parameter
still held; and a nested append.

**Two causes, and neither is the one the language would be changed for.**

1. **A parameter is never released from a block continuation's scope.** The
   lowering seeds its live set from statement binders only, so no parameter can
   ever appear in a dead set. **An accumulator threaded as a binding is reused;
   the identical accumulator threaded as a parameter is not.**
2. **The carry rule is all-or-nothing.** It takes a boolean — *is any
   sub-expression left* — and a frame with **any** left carries the **whole**
   scope, **even when the sub-expression is a literal that reads nothing.**

**And the positional rule is worse than the record says.** It is stated
everywhere as *position in the enclosing node*. That is right and it is not the
whole rule: **it compounds at every enclosing node on the path up.** Measured,
the growing field last in its record literal — **the rule as written down in
three places** — is still never in place when the record itself is not last in
the enclosing *call*. **An author who learns the documented rule and applies it
correctly still gets the quadratic. Two careful authors have written a version
of this rule down and both were corrected; this is the third correction, and it
is the argument that no rule of this kind should have to be learned at all.**

## The alternative was built, and refuted

A static lint was written for exactly this and refuted at two sizes against the
interpreter's own counters:

| shape | truth | lint |
| --- | --- | --- |
| an append in a non-final argument, into an empty list | in place every time — **no copy at all** | **fires** |
| an append in the last field of a record literal | **fully quadratic** | **silent** |
| an append whose list came through a conditional | almost never in place | silent |
| an append whose list was rebound in a block | never in place | silent |

**False positives and false negatives, the second of them on the exact shape the
lint existed for.** The pass also documented its own error set incorrectly — its
comment explained the gap as *anything that goes through a call is invisible to
it*, **and two of the misses contain no call at all.**

That is not a bad implementation to be improved. **A lint is a partial oracle
over an undecidable-in-practice dynamic property, and this is what a partial
oracle looks like when someone finally measures it.**

## The design space is closed by an argument, not quite

> Value semantics + a flat array + two owners + append ⇒ **somebody must copy.**

**A theorem, not an implementation detail, so it cannot be benchmarked away.**
Three doors: **give up value semantics**, which is Go and Java and removes the
trap by removing the operation, **at the price of aliasing bugs**; **give up the
flat array**, which turns the cliff into a slope with no annotation and no
analysis, **at the price of a permanent logarithmic index, which is the
operation every real program does most**; or **give up the second owner**, which
is Rust, **and is the only door that keeps value semantics, constant-time
indexing and native speed together.**

**The space is not as closed as that presents it, and at least a fourth door
exists: make the copy visible instead of the ownership.** An append that is
constant-time only in a linear position with the copying case spelled
differently, or a narrow array type whose only operations are append and freeze,
**puts the property in the type or the error without a whole-value ownership
check.** Smaller in scope, worse ergonomically, and it moves the problem to
conversion sites. **This should not claim a closed space while a reviewer can
find an unrefuted door in a paragraph.**

**And the claim the third door actually earns is predictability, not speed. The
two-owner case does not disappear under any door; what changes is whether you
can see it coming.**

**Swift is the control that shows the third door is required rather than merely
available.** It has value semantics over a flat copy-on-write buffer — this
design exactly — and has this trap exactly, as a well-known footgun in a
mainstream, compiled, heavily-resourced language. **Reaching for a better data
structure is not what saved anyone.**

## The instrument error this exists to correct

The first version of this argument recommended the persistent vector, and its
reasoning is withdrawn here rather than deleted, **because the shape of the
error is the point.**

> **Withdrawn: "the index cost is nearly invisible, so a persistent vector is
> close to free."** The evidence was a profile — dispatch dominating, refcount
> traffic next, every builtin body together at a fraction of a percent of leaf
> samples. Indexing happens inside a builtin body, so making it three pointer
> chases instead of one moves a few percent of a few percent.

**The measurement is sound and the argument is not.** It says the interpreter's
dispatch overhead is large enough to hide a data-structure regression. **That is
true, and it is an argument for remaining an interpreter.** If Ply intends to
compile, the target deletes the dispatch, at which point the index cost is fully
exposed and permanent. **A decision that is correct about today's implementation
and silently assumes today's implementation is the destination is the error —
not the number.**

**And the same number has a second trap in it.** "Every builtin body at a
fraction of a percent" is a share measured *underneath* the dispatch and
refcount traffic. Delete both — which is what compiling does — and **the builtin
share rises by construction.** The append's copy is inside that fraction. **So
read carelessly, the profile this cites says the shipped defect it was written
about is negligible.**

The general form is worth asserting, because the record contains more of it:
**an interpreter ratio can settle a question about this evaluator and cannot
settle a question about the language.** The deferral of a code generator, the
verdict on compute kernels and the rejection of a narrower value type are all
ordered by such ratios, **and each should be read asking which question it
actually answered.**

## Declined: a parameter mode, and the reason is measured

All four proposals put a three-point mode on each parameter of an arrow,
inferred over components the way an effect row is, carried in the function type,
and published in the signature. **Declined.** No keyword, no mode in the
function type, no change to unification, no hash movement, no version bump, no
signature churn.

### A parameter's uniqueness is not a property the caller can promise

A parameter with **exactly one occurrence**, at its last use, free in no
closure, cell or record — **every rule stated by every proposal infers `own`
here** — measures **two owners** under a multi-shot handler. **The one-shot
control is what decides it**: under a single *tail* resumption it is two as
well, and with no handler at all it is one, in place. **So it is capture, not
multiplicity.** Any perform in the enclosing dynamic extent puts a second owner
on the value, and the corresponding region analysis measures **every region in
this repository as shared and none as unique**, every one because of a
tail-resumptive clause — **because the canonical Ply region is a cell backing a
handler.**

**A mode is a contract between a caller and a callee, and neither party can
discharge it**: the callee cannot, because the second owner is created outside
it; the caller cannot, because the handler that captures the continuation may be
installed by *its* caller, arbitrarily far up. **The only sound side condition
is "no perform anywhere in the dynamic extent", which over this corpus is zero
regions.**

**The dilemma, as sharply as it can be stated.** A mode can mean *"the caller
does not read this again"*, which is syntactic, local, genuinely checkable — and
buys **nothing**, because not reading a value again is not the same as being its
only owner. Or it can mean *"one owner when the callee runs"*, which is what
licenses the append **and is measured false at a site every proposal accepts,
with no perform written anywhere in the callee.**

**So the mode is either checkable and useless, or useful and uncheckable.** All
four wrote the first meaning into their surface and reasoned about the second in
their worked examples, **which is why all four report a burden of zero and none
reports a benefit that is theirs rather than the carry rule's.**

The answer stays correct today only because the runtime probe refuses. **That is
the right behaviour, and it is why the copy under a multi-shot handler is not a
pessimization but *the semantics*: the second resumption must not observe the
first's append.** A design promising constant time there would be promising a
wrong answer, **and one that promises it *and* keeps the guard is promising
something it silently does not deliver — a green result over unexplored space,
inside the mechanism meant to cure it.**

### The mode cannot be cached under a hash

The region-kind analysis settles this against itself: **the sound key is the
whole program.** Two of its inputs are whole-program and neither is inside the
hashed dependency closure — whether *any* capture is written *anywhere*, and
whether a name denotes a definition or a local. **Adding a handler to a module a
definition neither names nor reaches flips the answer, and moves no hash.**
Every proposal put the mode in the function type, hence in the scheme, hence in
the content-addressed store, arguing from the effect row's precedent. **A
published row is sound to cache because a footprint is a function of the
definition's own hashed closure. Uniqueness is not. The precedent does not
transfer.**

### The surface cannot describe the file the milestone exists for

The lexer spike has **zero** list-typed parameters: all three of its list
occurrences are record *fields*. The call ceiling means a lexer threads its state
through a fold as a record — **the shape the whole spike is about, and the shape
the two largest measured copy blocks take.** A mode on a *parameter* has nothing
to say about any of them. **Every proposal's worked examples are accumulators
passed as parameters, which is the one shape a last-use analysis already handles
at zero cost.**

## What replaces it: report the expectation, and make it falsifiable

The ownership hint stays a hint — *an optimization hint and never a permission*.
**Ply cannot prove a value has one owner. It can say where it expects one, print
that expectation, and fail a test when the expectation was wrong.**

**A cost report**, per definition, naming each append and whether the lowering
marked its list argument owned. The presentation is grafted from how a region
brand is inferred and *shown* in a printed signature without being *asked for* —
**and the correction that matters is that what belongs in that slot is a
*count*, not a type, because the honest form of this guarantee is a cost.**

**The falsifier, and this is what stops the result being vacuous:** every append
the lowering marked owned **must be counted in place, or the test fails, naming
the site.** It costs an afternoon, needs no language change, and **converts a
checker that asserts a property it has not established into one that fails
loudly on the existing corpus.** The statistics it reads are diagnostics-only,
so nothing about it can move a program's meaning.

**Registered before it is built: the assertion will fail on the tree as it
stands. That is the point.** Sites that cannot be reused are to be
**reclassified** — by making the liveness analysis refuse to mark them — **not
exempted.**

**An explicit copy builtin**, semantically the identity, promoted from a silent
fallback to a written word. **Identity-ness is the property that makes an escape
hatch safe for code a model writes at speed: every diagnostic has a mechanical
fix that provably cannot change what the program means.** **And its own weakness
is adopted with it**: because inserting it cannot change meaning, **it is
exactly how a model discharges a cost signal without understanding it.** So
there is deliberately **no automatic fix**, and it is never the fix a diagnostic
*recommends*.

**One warning, no new error.** Raised where the lowering can show the argument
**cannot** be owned. **A warning because the measured false-positive rate of any
analysis of this shape is not zero and the direction is wrong**: a shipped
function captures its accumulator in a lambda, which the analysis must call
unownable forever, **and it is fully in place at runtime.** A hard error there
forces an explicit copy and **makes a linear loop quadratic** — which one
proposal's ergonomics judge identified as that proposal's disqualifying defect.
**The trap is avoided by not building the error.**

## Four evaluator changes, each gated on the counter

No syntax, no type change, no hash movement, no annotation. **This is where
every measured win is.**

**A parameter may appear in a dead set. Built and measured** — roughly ten
lines, seeding the lowering's live set from the enclosing barrier's bindings
rather than from statement binders. **It takes the two request-path modules from
under one percent in place to about two-thirds**, removing thousands of copies.
The whole evaluator suite passes; eight adversarial programs answer correctly.
The safety argument is that a closure's free variables *become reads at the
construct that captured them — never last ones*. **That argument is not yet
written as a case analysis and that is the condition on landing it.**

**The carry rule takes a dead set, not a boolean.** All four judging lenses on
all four proposals named this as the graft to take regardless of which design
won. **Its ceiling is measured rather than argued**: the same programs,
reordered so the append already sits last at every enclosing node, are fully in
place. **This is what makes that hold without the reordering.**

**Two conditions, from checking the mechanism against the environment
implementation.** Releasing clones every binding above the deepest released one,
**so a release at every sub-expression replaces a refcount bump on the hottest
path with an operation linear in scope depth, and can newly refuse a take that
succeeds today.** So the released shape must be precomputed at lowering with the
common empty case staying the existing clone; **and it does not land until the
allocation report has been re-taken, because this milestone's premise moved that
number the wrong way once already.**

**A projection may move a field out of a record that is dying.** Two lexer
functions write the growing field **last**, exactly as the documented rule asks,
and still copy. **A negative result, measured, that should save the next
implementer a day**: the five-line half — take the field out when the frame owns
the record — was built and **changes nothing**, alone or with the parameter fix,
**because the record base is still carried by the enclosing literal's own frame,
so it is never at one owner when the projection runs.** Gated behind the carry
change and worth nothing before it. The general form — path-granular liveness —
is declined on the grounds that **a wrong answer there is a wrong program rather
than a slow one.**

**Fused cell and map update builtins.** An append onto a cell's contents is
**over a third of the tree's append sites**, measured at **zero percent in
place, unconditionally**: the arena still holds the value while the append runs,
**so no ownership design fixes this** — and every proposal that claimed
otherwise did so via a syntactic peephole one judge showed to be unsound, since
a perform between the take and the set exposes the emptied slot. A fused builtin
takes the value out *inside one builtin*, **which establishes soleness at
runtime rather than proving it statically — which is why it is sound under
multi-shot, and why it is the right shape.**

Those sites are a **library migration, not a compile error.** The old idiom
keeps working and keeps its quadratic. **Making it an error, as two proposals
did, turns every shipped example program into a build failure and prices the
pessimization as the fix.** The implementation cost is understated by a line
item: the cell operations are special call forms because of region-brand
inference, **so a closure-taking version needs a third call form with row
joining, a new machine frame, a tree-walker twin, and an entry in the
callback-builtin list or every region containing one silently becomes shared.**

## The burden, as a number

**Zero, and that number is worthless on its own** — this design has no
annotation, so its burden is zero by construction, **and reporting it as a
result would be exactly the vacuous green being warned about.** What is worth
something is **zero forced source edits** on the motivating functions, the
copies each has today, and the copies after the measured change. **Read the copy
counts before the burden**: one function the brief named as the demonstration
**contains no append at all**, so this design does nothing for it, **and any
proposal reporting a win on it is reporting one it did not get.**

**Tree-wide the honest number is a few dozen forced edits, all of one shape**,
all mechanical, all optional in the sense that the old form keeps compiling —
**but the shape is the one a production handler would use.**

**And the number nobody should take from this document:** the bytes type has no
spare capacity and no in-place append, so byte concatenation cannot reuse
anything whatever the analysis says. Two shipped functions are quadratic on
bytes and their own comments say so. **Nothing here touches any of them. If a
reader takes "ownership fixes the quadratics" away from this document, that
reader has been misled.**

## Soundness

**There is no soundness obligation to discharge, and that is the design.** The
hint is not promoted; the uniqueness walk and the runtime probe both stay in
release builds; a wrong answer costs a copy. The hazards are therefore all
*precision* questions.

**Aliases the liveness of a *name* cannot see** is the hole that would have sunk
a mode system, worth stating even though this design does not fall in it: a
binding aliased to another is genuinely dead at the append — its last use, on
every path — **and there are two owners. Name-liveness is not
value-uniqueness.**

**Regions and cells**: the arena is a genuine second owner for the whole of the
append, so only a fused builtin fixes it. The enumerated escape boundaries — a
host operation's argument, a handler's or the runtime's answer, an entry point's
argument — are where a value acquires an owner no analysis can see, and the new
dead sets must not release across them. **That list is already enumerated and
tested, and reusing it verbatim is the one piece of region machinery ownership
should not re-derive.**

**The two engines.** The tree-walker contains **zero** occurrences of the
ownership machinery, receives none of these changes, and **the differential
oracle compares answers rather than costs, so it cannot see a cost divergence in
either direction. A real gap, not closed here.** What makes it survivable is
that none of the first three changes can change an answer, and the release is
functional, **so a wrong release is a loud internal error naming the binding and
never a different value.**

**One thing this design cannot see.** The constant memo retains a value for
every nullary pure definition and hands out clones, **so a memoized constant has
a permanent second owner** — and its key is the *published row*, which is
whole-program information again. **A fourth route to a second owner that no local
rule models, harmless here because the guard catches it, and unsound under a
mode system for the same reason as the caching argument.**

## Regions do not build ownership; the dependency runs the other way

**Regions are already downstream of the ownership analysis** — ADR 0017's own
correction says the reference-counting pass *is* its implementation, so building
the ownership check on the region brand would be **a cycle in the argument**.

**They ask converse questions with opposite quantifiers, and only one is
flow-sensitive.** A region asks whether a value can reach *outside* a scope —
existential, decided on a **type**, flow-insensitive by construction and right
to be. Ownership asks whether anything else can reach a value *here* —
universal, decided on an **occurrence**. The measurement that settles it is two
lines: **two record literals with the same fields in the other order have
identical types, identical values and identical semantics, and measure fully in
place and never in place. No predicate over a resolved type can separate
those.**

**And confinement is not a count**: a region containing an aliased list escapes
nothing and has two owners inside it. **Make region-brandedness license an
in-place append and the program answers differently — a change of meaning,
forbidden by the same record that defines the regions.**

**What regions genuinely contribute** is the enumerated boundary list and the
presentation graft. **Regions are where ownership is lost, not where it is
established.**

## What this costs, and what a mode would have

Zero files in the syntax, hash and store crates; a handful in the evaluator; two
at the CLI; a few dozen optional call-site edits. **No hash moves. No version
bump. No re-run. No review baseline is invalidated** — reviews are keyed by
hash, **which writing a mode into a signature would not have allowed.**

**For contrast, and because a reader should see what is being declined:** the
mode designs were priced by three independent implementation reviews at dozens
of files across half the crates, thousands of lines, a mandatory version bump
and a full re-run — **before one quadratic is fixed. The parameter fix alone is
ten lines and removes thousands of copies.**

## The persistent-vector fallback, priced — and a result not claimed

Nobody in the round measured it. Criteria were pre-registered before running,
against a vector that is **already a workspace dependency**, so the dependency
cost is zero.

**The shape is the whole finding.** Today's behaviour whenever anything else
holds the list is quadratic **and its penalty against the good case grows
without bound.** The shared vector is linear and **its penalty is flat at a
constant factor. A structurally-shared vector does not make the bad case fast;
it converts an unbounded penalty into a constant one, and it does so for *every*
case this design cannot otherwise reach** — the cell sites, the multi-shot site,
the memo, every alias.

**Against the pre-registered rule, two of four criteria failed, and this does
not get to claim the result.** The shared-case constant factor missed its bar,
and so did the index ratio — **and the index criterion was ill-posed when it was
written, which was found out after measuring: there was no list index builtin at
all.** No Ply program could pay the cost, **so the criterion was meaningless
rather than met, and it is recorded as a miss rather than quietly dropped,
because a criterion rewritten after seeing the number is not a criterion.** *An
index exists now (ADR 0027), so it is well-posed for the next taking — with one
caution: a peek is measured as almost entirely interpreter dispatch, so the index
arm will price a small term unless the backend has landed.*

**So the representation change is not taken now.** The gate is fixed here,
**before the measurement, so a number cannot set the bar it clears**: after the
four evaluator changes land and the allocation report has been re-taken, the
representation changes if either request-path module is still below a high
in-place rate over its own suite, or the falsifier is still failing at more than
a small fraction of marked sites. **Below that bar the analysis has not
delivered and the cliff should be removed by representation instead. Above it,
the remaining copies are the ones the semantics require.**

One caveat in the fallback's favour that the measurement understates: the
elements measured were integers, whose clone is trivial, **while the shipped
corpus's lists hold bytes and records.** One against it, **and it is the one ADR
0017's correction exists to warn about:** the shared vector allocates spine
nodes where an in-place append allocates nothing amortized, **so the change
would very likely move allocations per request *up*, which is the number this
milestone's three predecessors are judged on.**

## What would make this wrong

- **If releasing on the hot path costs more than the carry change saves.** The
  number that could sink it, and it is not in this document. **ADR 0017's record
  shows a premise of exactly this shape moving allocations the wrong way once.**
- **If releasing a parameter can outrun a capture.** Eight adversarial programs
  are consistent with the argument, **but the case analysis is not written.**
  The failure mode is a loud internal error on a legal program — the right
  direction, **but a new way to reach a diagnostic whose whole point is that it
  is unreachable. Write the case analysis or do not land it.**
- **If the falsifier, once armed, fails at a rate that cannot be brought down.**
  **The right response is the representation fallback, not a weaker assertion.**
- **If ownership inference cannot be made to work without annotation burden**,
  and Ply acquires a borrow checker's learning curve **in a language whose
  premise is that a model writes the code and a human reads specifications.**
  The central risk, and not small.
- **If the compiled target does not arrive.** Then the interpreter's dispatch
  overhead really does hide the representation cost, **the withdrawn argument
  becomes correct again**, and the fallback is the cheaper answer. **This is a
  bet on the same world the bootstrap record bets on, and it fails in the same
  world.**
- **If indexing turns out not to matter.** *This once offered the absence of a
  list index as evidence for that, and the shape of that mistake outlived its
  content: **it read the absence as evidence about programs, and it was evidence
  about the surface.***
- **If bytes turn out to dominate.** Dozens of concatenation sites, two
  documented quadratics, and this fixes none of them.
- **If a fourth door exists.** One is recorded above. **An argument that finds
  another is the most valuable refutation of this record.**

## Provenance

Every number was taken on this tree by the author, not quoted from the four
proposals, **and several of theirs did not reproduce.** Pre-registration written
before any count or benchmark; the built changes were behind a flag and **all of
it has been reverted.**

What did not reproduce, recorded because the disagreements are informative: a
reordered append inside a recursive loop measured **zero** in place, not fully —
**it is fast only when the reordering also holds at the *enclosing* call**; a
memoized constant's append did **not** reproduce as a copy, because the harness
did not arm the memo, **so the claim is untested rather than refuted**; and one
grep figure was contaminated by a shell fallthrough.

Four proposals were prepared and judged, and this is not any of them. **The
falsifier is one proposal's soundness judge's idea and is the most valuable
single idea produced in the round.**
