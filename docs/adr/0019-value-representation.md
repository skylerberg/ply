# ADR 0019 — What a value costs, and what to change about it

**Partly accepted.** The two invariants below bind every change here. The
argument-vector pool and the constant-value memo are landed. The record layout
is ranked and priced and **not accepted**. Narrowing the value type is
**rejected**, and needs no further measurement: the number that would justify it
was taken and it is zero.

## Provenance discipline

Every figure has a command that renders it, named where the claim is made rather
than transcribed. **Re-take rather than quote.** Allocation counts do not vary
with load; wall-clock ratios do, which is why every ratio comes from a harness
that times both sides inside one window. And **one instrument runs a pre-built
binary while the rest rebuild** — check the binary is current before believing
that row.

## Context: the milestone was requested as "unboxed primitives", and the premise was false

The case was two sentences: every integer is a heap-allocated value, and the
literal constructor allocates on a workload doing almost no arithmetic.

**The first is false.** The scalar variants are inline and building one touches
no allocator. **There is no primitive boxing in this evaluator to remove**, and
it is a test that says so rather than a reading.

**The second is the one worth keeping, because a re-take cannot catch it.** The
frame ranking was real and the count was real; the conclusion did not follow from
either. The literal constructor cannot allocate at all unless the literal is a
string or bytes — the other arms return without touching the allocator — so the
count was not integers being boxed. And the count itself was read off a **short
window**, where a machine's worth of one-time literal construction divides down
into what looks exactly like a per-request slope. Fitted over two windows the
per-request part is much smaller, and the two reconcile to the digit.

**A ranking is not a cost, and a frame is not a type.**

## What the milestone did instead

It took the attribution the premise should have rested on: allocations per
request, attributed to the **value being built** rather than to the frame that
built it, **fitted as a slope over two windows so that per-machine setup cannot
masquerade as per-request work.** Two independently written classifiers — one
ranking by frame, one by value — agree on the split.

Two routes are reported and they disagree on ranking. A lever is judged on the
served path, because it is the only one that pays for framing, the host boundary
and the response encode. The pure-call route is the one to read for the
interpreter proper. Both are printed, and the two top levers rank first and
second on both, so nothing depends on the choice.

**The baseline the shares are fractions of was fixed before any lever was built.
Do not edit it to make a measurement pass** — that is the one edit that would
stop the number meaning anything.

## Two things every change preserves

**A secret's payload stays unmatchable and unprintable.** Three moves would
break it: folding the secret into an ordinary constructor, which makes the
payload matchable; a rendering path that descends into a compound before the
redaction arm sees it; and **a pool or intern table that keeps a value after the
call that carried it returned, which leaves a credential in a buffer the next
call reads from.** That third is the risk the pool introduces, and it was armed
against the seam before the pool existed.

**The store's schema fingerprint moves if a stored type's encoding moves.** It
is digested over *encoded exemplars*, so an encoder that starts writing a field
differently moves it even when no declaration changed. **Checked, because it
decides how much of this is a cache-format change: the value type is not a
stored type.** What *is* stored is rendering's **output**, cached in a failure
message — so a change that moves a rendered byte is a *runtime* version bump,
not a schema one, and a change that moves a stored type's encoding is a
*front-end* bump.

## 1. Recycle the call-argument vector

The largest single line in the profile, on both routes. Most argument vectors
are filled, handed to the callee, emptied into its scope and freed: an
allocate/free pair per call. A thread-local free list in four capacity classes,
wired at two sites.

**Where the reasoning had a hole, and it is the finding.** The specification
said the entry point frees every non-retained buffer. It does not, because it
skipped a callee kind: a builtin call takes its argument vector **by value** and
consumes it, so that buffer is freed inside a function with no way to hand it
back, whatever the seam does. The vectors split four ways: **recycled** by the
free list; **retained** as a constructor's arguments, where there is nothing to
give back; **wider than the classes**, left to the allocator by construction;
and **freed but never given back**, overwhelmingly builtins.

**The lever removed everything the mechanism could reach and still landed under
its floor, and the floor was deliberately not edited.** The floor was derived as
a fraction of a share this document had wrong, and the share the lever can
actually reach is under it. **That is a documentation defect, not a weak lever.
Editing a pre-registered threshold to make a measurement pass is the one edit
that would stop the number meaning anything**, so the criteria file is
untouched. Re-deriving the floor is a decision for whoever amends this.

**"No regression detected" is weaker than "no regression".** The instrument is
paired windows running two binaries back to back, alternating which goes first
so position bias cancels, keeping or dropping windows on a load threshold fixed
before the data. Three things carry forward, and none of them is a ratio: **the
one pre-registered run is underpowered and does not resolve the criterion** —
pooling it with earlier runs clears the bar, but that applies a filter chosen
with some data already seen, so it is a supporting cut and not the result; **the
sign test leans the wrong way**, at a magnitude of one print step of the
harness, which is exactly what a bounds check and a length reset would look like
and which this instrument cannot separate from rounding — **resolving it needs a
timer with more digits, not more windows**; and **a percent-scale criterion
cannot be resolved on a loaded machine at all.**

**The buffer a builtin consumes is the next lever and it is larger than the
record layout.** Recovering it means every arm of a hundred-arm match taking a
mutable reference or draining rather than owning — a change to a function
signature, not to two function bodies.

**What it must not break.** A pooled vector may not hold a value — it would keep
a cell past the region that would reclaim it, defeat the uniqueness probe, and
park a credential in a reused buffer. A vector handed out may not be non-empty,
because the callee pushes and a residue shifts every argument. The list may not
cross a thread. A retained vector may not also be in the list. And a release
during thread-local teardown may not abort.

**The two-engine comparison cannot see the case this lever is most likely to
break.** Every continuation-binding clause is refused by the tree-walker, so the
differential harness records the *refusal* and compares no value at all for a
program that resumes a continuation — **and multi-shot resumption is exactly
where one frame becomes two, each finishing a buffer taken from the free list.**
An audit covers it instead, with a continuation captured *inside an argument
list* and resumed twice, **and it asserts the blindness itself, so a tree-walker
that later grows the capability turns that justification into a failure rather
than leaving it stale.**

## 2. Build a compile-time constant's value once

Second on both routes: values rebuilt every evaluation from something the
compiler already knows — literal string and bytes constructions, nullary
constructor mentions, constructor-closure mentions. **The same pattern as
runtime work for a static value that an earlier milestone found:** a literal is
a compile-time constant whose value could be built once at lowering, and a
refcount bump is measured at zero allocations.

Two mechanisms, no new type: literals carry their built value on the lowered
node, and constructor values come from a thread-local cache. **The literal
variant stays on the node** — not for the machine, but because the code
generator dispatches on it to choose a native type, and that crate is outside
the workspace and has already bit-rotted from exactly this kind of widening. **A
build agent that widens the node and does not build it breaks the only
instrument this project has for pricing codegen.**

**The mitigation this section specified was structurally incapable of firing,
and that is the finding.** It said the tree-walker is unchanged and the
two-engine comparison is therefore the check. For the constructor half that is
false: the memo lives inside a function **both** engines call, so comparing them
compares a value against itself. The literal half is the control, where the
tree-walker builds fresh and the machine clones, so a divergence *would* be
visible. **Measured, not reasoned**, by a test asserting pointer equality across
the two engines in both directions.

This is the failure class the contribution rules list twice: **a mitigation
named in a document that cannot fire.** What actually audits the memo is a file
whose own note says *why* it can, and which checks the properties that survive
sharing: the shared value matches the arm a fresh one matched, compares equal to
a fresh one, and holds nothing a closing region could reclaim.

**No version bump.** No stored type moves and no rendered byte moves — the same
value renders the same way whether built once or a thousand times. **If a build
agent finds it needs a runtime bump, that is a signal something *is* observable,
and it should stop rather than bump.**

## 3. A record's fields in one allocation — ranked, not accepted

A sorted-slice record costs one allocation where a tree costs two for a
one-field record. It waits on the two levers above landing and on a **record
width histogram, which does not exist** — the layout is a linear scan, and it is
wrong at fifty fields.

**The sort is load-bearing at three places and is the whole risk of this
change**: comparison iterates in order, equality compares key sequences first,
and rendering renders in that order — **and that output is stored.** All three
would silently answer wrongly over an unsorted slice rather than fail. The sort
is lexicographic over interned symbols and therefore **does not depend on intern
order**, which was checked, because a field order that varied run to run would
break four things at once and none of them loudly.

## 4. Rejected: narrowing the value type

The milestone's name points here, so the refusal is stated rather than left to
omission. Boxing the widest variant reaches a smaller value, and what that buys,
measured, is a few kilobytes off a byte column that is **not comparable across
windows** — and **zero allocations.** What it costs is an allocation on every
applied constructor. **So the change trades an incomparable byte count for a
comparable allocation count, in the wrong direction.**

The other direction is worse: a slice for the constructor's arguments removes
one indirection and *widens* the value, and prices construction at two
allocations against one on the path where it matters.

**Rejected, and this needs no further measurement: the figure that would justify
it was taken and it is zero.**

## 5. What the kernel re-pricing changed

The compute-kernel record asked for one measurement before anything was built,
and it landed. **The premise held on shape** — most of the kernel's functions,
lowered nodes and executed work are inside the fragment.

**What the fragment refuses is the roadmap — but the refusal census does not
read as a work list, and that is the trap.** It is a *first-refusal* census, so
the rows are **not additive and not independent**. Admitting the top construct
moved the census by almost nothing, because the rows are a single closure that
arrives on the last item — and a fifth construct that appears in no row at all
had to be lowered too.

**The conclusion drawn does not hold, and the reason is structural.** End to end
the hybrid was worth nothing, **because the interpreter could not call compiled
code.** The boundary was never the problem.

So an amendment owes three things. **A new blocker above everything else: a
backend the interpreter cannot enter buys nothing whatever the representation
is.** The float item priced or withdrawn — the fragment has no float path and
fails at run time, **and a census counting such a function as compiled counts
one that cannot run.** And **a lever the list does not have, which outranks most
of the ones it does**: there is no square root and no logarithm for any numeric
type, so the kernel's hot function computes its own square root by Newton's
method over an integer logarithm — and that function dominates the kernel. **Two
prelude builtins and no compiler work.**

## 6. Found while integrating, and deliberately not priced

One site: a reserved-prefix test builds a string on every call, once per host
operation resolution. The removal is three lines with no semantic content, and
is almost certainly worth taking. Not taken here, and the second reason is the
real one: nothing places a share under it or sets a floor for it, so the
judgement rule cannot answer; **and landing it would move the freshly re-taken
figure and break the per-lever decomposition beside it, which was measured one
change at a time.** When a lever lands under its floor the next step is *another
attribution* rather than the next lever, **and that applies to a lever found by
accident as much as to a planned one.**

**How it surfaced is worth more than the site, and it is a trap in the
instrument.** The attribution rule table keys on the deepest matching frame over
a small window. In release this allocation is **inlined** into its caller and
lands in the host-boundary bucket; in debug it is a frame of its own whose
callers symbolize to a bare crate name, **so the same tree is attributed two
ways by profile.** The residue passed in the profile the harness was developed
in and failed in the one the test suite runs. **Anything added to that table
should be checked in both profiles before it is believed.**

## 7. Found while auditing, and fixed: a map key was a function of insertion history

Not a lever and not a regression — it predates this work. It is here because it
is a *value representation* defect, this is the value-representation record, and
**the consequence was recorded nowhere while three comments in the tree asserted
its opposite.**

**The defect is a conjunction of two deliberate decisions.** Comparison and
equality compare a decimal **by numeric value**, so two spellings of one number
are one map key. Rendering prints **the scale as stored**, because the scale is
a digit count the value carries. Together they made a map's *keys* a function of
insertion history: an insert replaced an equal key's key as well as its value,
so whichever spelling was written last is the one iteration answered with.

Three written claims said that could not happen — *a hash-ordered map makes
iteration a function of insertion history, and four guarantees rest on a value
having one canonical form*; *iteration is a function of the values and of nothing
else*; *a fold over a map is a function of its contents rather than of how it was
built* — **and the failure each names was present anyway, through the key rather
than through the order.**

**What it cost, end to end.** Two maps that compared equal as one value served
two different response bodies, because the JSON writer goes through the
decimal's stored form. **The two-engine comparison reported no divergence,
because this was never an engine disagreement. It was the language.** The blast
radius was wider than one map type: a record-keyed map carried it into a
compound key.

**The fix goes in the representation, not in either deliberate decision.** A key
is reduced to the one representative of its equivalence class on the way in,
through a single site every insert passes — **adding a second site re-opens the
defect.** The canonical member is the minimal-scale form, which is unique per
numeric value and therefore a canonical form rather than merely a smaller one.
Every position comparison descends into is walked, because a decimal anywhere
under a key is a distinction the order cannot see. **A secret is not descended
into**: it is refused as a key before this runs, and a path that rebuilt a
credential's payload is what the secret work exists to prevent.

Two spellings still compare equal, and a decimal that is *not* a map key still
renders every digit it was written with — **both asserted, so a "fix" that
rounded the scale away everywhere would fail.**

**Versioning, judged here explicitly rather than by omission.** A stored
artifact's contents do move for a program that renders such a map, but only from
one of two spellings to the canonical one, and only for a value that had no
single spelling before. No stored *type* moves. **A runtime bump is not taken**,
because the values whose rendering moves are exactly the ones whose cached
message described a run that was not reproducible in the first place. **A reader
who disagrees should bump it; the argument is here to be disagreed with.**

## The criteria, in code

The window pair a slope may be fitted from, the baseline, the share placed under
each lever, each floor, the wall-clock regression none may exceed, and the
divergence count that reverts one whatever it saved — all fixed before any of
this was built, with a test that a floor cannot be quietly raised above what was
ever counted. **The same pattern as the performance verdict's criteria, and for
the same reason: a threshold a measurement supplies is a threshold the
measurement cannot fail.**

## Sequencing

The argument vector first — largest line on both routes, touches no stored
artifact. **Then re-take the attribution**, not "run the next lever": if a lever
lands under its floor, the answer is another attribution, **because a mechanism
that fired on something other than what was counted invalidates the ranking
under it too.** Then the constant memo, sequenced after only so each lever's
number is attributable to it. Then the width histogram, then decide the record
layout.

## What would make this wrong

- **If the vector pool lands and the allocation count does not move by its
  floor.** Then the vectors are not transient in the sense a pool needs, the
  attribution's largest line is not a lever, and the right response is another
  attribution — not the next lever, and not a wider pool.
- **If it saves allocations and loses wall clock.** Then every allocation-count
  target here **is measuring a proxy that has stopped tracking the thing anyone
  cares about.**
- **If the constant memo requires a runtime bump.** Then interning a
  compile-time constant *is* observable, and the builtin-closure sharing that
  has been doing exactly this for milestones is a latent defect rather than a
  precedent.
- **If a build agent has to widen the value type to land any of this.** Every
  argument in the rejection was made at the current width, and most of a
  request's value-wide slots live in argument vectors alone.
- **If the seam turns out not to be where the vectors are made.** It is wired on
  the strength of one controlled experiment. **If a pool there moves fewer
  allocations than the arity table predicts, the rule table is what to doubt
  first, not the pool.**
- **If the two routes' rankings diverge after a lever lands.** They agree today.
  If they stop, one harness is measuring something the other is not, **and no
  verdict may be read off either until that is explained.**
