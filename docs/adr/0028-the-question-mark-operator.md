# ADR 0028 — The `?` operator

**Accepted, implemented.** Follows ADR 0023 almost exactly, and is constrained
by the same two things: a definition's identity is its normalized structure, and
the driver hashes before it infers.

## Context

The parser spike ranks "no `?`" third among the language gaps a
recursive-descent parser hits, **and it is careful to say that the *writing*
cost is not the problem.** A boolean bail flag threaded through the parser's
state is the *same type*, not an approximation, **and the guard goes on the
callee instead of the call site, which is strictly better than the shape the
JSON module is forced into.**

**The problem it measured is verifiability.** The spike deleted its guards one
at a time and asked whether anything noticed: **of 83 guards, 63 are
unverifiable** — 43 were deleted and nothing moved, not the type checker, not a
hundred in-language tests, not a differential against the reference over error
fixtures; 20 were never deleted at all. **Only 20 demonstrably matter.** The
cause is that every consuming primitive guards on the flag itself, so a
guardless parse function reads no tokens, emits no diagnostics, builds a node
its caller discards, **and is indistinguishable from the guarded one.**

**That is 63 lines of code no reviewer can justify, in a language whose thesis
is that what a human reviews is a specification. `?` does not make those guards
verifiable. It makes them unwritable**, which is strictly stronger: there is no
guard to delete, no invariant for a reader to maintain, and no phantom
diagnostic from an unguarded predicate answering false.

## The constraint that shapes everything

> **`e?` must be the same definition as the `match` it stands for.**

Otherwise converting a site moves its hash and every dependent's, splits one
value into two cache entries, **and turns "the module's behaviour is unchanged"
into an assertion rather than a measurement.**

## 1. Expansion runs inside the parser

An unexpanded node never escapes `parse_module`, and the bare-expression entry
point — which has no enclosing function and therefore no written return type —
refuses rather than leaking.

**Not as a node in the evaluators, which is the obvious design and the expensive
one.** Under expansion the downstream arms are unreachable or one-line
recursion. **Under a live node six of them would be *semantics*:** an unwind in
each engine, a typing rule threading the enclosing function's return type **and
its row**, a normalization tag, a prover lowering. **That is ADR 0001's "four
implementations, four chances to disagree", and it buys a construct that is not
equal to any longhand.**

Two consequences worth stating rather than implying. **Effect rows are untouched
by construction**: the pass introduces a match and two constructor
applications, all pure, so the row is the longhand's character for character.
**There is no row rule for `?` because there is no `?` after the parser, which
is why the constraint cannot be got wrong.** And **tail calls are preserved** —
the continuation lands in the success arm's tail, exactly where it is today.

**The pass order relative to the other sugars is a free choice, and the reason
first given for it was wrong.** It claimed a hazard through annotated bindings
that **cannot occur**, because that shape is the one refused outright. What is
true instead is that each pass walks through the other's node, **so neither
order is a hazard** — measured, by swapping them and hashing both corpora to
**zero moved entries**. **Nothing gates this order; treat it as a convention,
not an invariant.**

## 2. The canonical expansion, failure arm first

The failure arm reconstructs and returns; the success arm binds and continues.
For a `?` that is the whole value of an *unannotated* binding, the block splits
at that statement and **the binding's own pattern becomes the success arm's
binder.**

**Arm order is measured, not chosen.** The normalizer writes match arms in
source order, unlike record fields, which it sorts — **so this parameter decides
whether converting the corpus moves a hundred-odd hashes or zero.** A
pre-registered rule fixed the answer to whichever the corpus writes more often,
before counting. It writes the failure arm first by more than forty to one for
one type and by a smaller majority in the same direction for the other. **Failure
first, for both, one rule.**

**The binding case is not a convenience.** Without it the general rule would
emit a nested rebinding, **which is a different definition with a different
hash, and the corpus conversion would move every site.**

**The mixed-length trick ADR 0023 needed has no analogue here, and imitating it
would have been cargo cult.** Record-update copies are *sorted*, so a suite of
same-length names cannot tell sorting by name from a comparator that ties on
length. Match arms are not sorted. **What has to be pinned instead is that the
order is the corpus's *and that reversing it is visible***, so the hash test
carries a reversed longhand and asserts inequality against it. Binder names are
erased by de Bruijn levelling, which is why a synthesized binder can equal a
longhand's ordinary name at all — **and the longhands use ordinary names, so a
levelling that leaked one fails the test.**

**Synthesized binders are unwritable on purpose**, spelled with a character no
identifier can contain. **A binder with an ordinary name would *capture* one the
author wrote in the expression it wraps, and the capture would type-check
wherever the two happened to agree.**

## 3. The mode is the enclosing function's *written* return type

The parser has no types — the driver hashes before it infers, **which is
precisely why ADR 0023 put its expansion here and not in normalization** — so
`?` cannot be resolved from the scrutinee's type. What *is* available is the
same thing record update reads: **text written in this file**, on the line
above.

**Cross-module aliases are refused**, for ADR 0023's reason: gate 1 skips a file
whose raw bytes are unchanged, so a meaning read across a boundary would leave a
stale expansion behind in a file that never moved.

**Serving the optional type as well as the fallible one is corpus-backed, not
speculative**: a pre-registered rule said serve it only if the corpus wants it,
and it does, across five modules. **Two operators for the two types was
rejected: one operator whose meaning is fixed by the signature the reviewer is
already reading beats two a reviewer has to keep apart.**

## 4. Where `?` may stand, and why the rule is the purity predicate

A `?` is admitted when, from its region root down to it, every step is
**unconditional** and does not leave a **nested block**, and everything
evaluated **before** it is **pure**.

The two conditions have different reasons. The nested-block rule is about scope:
lifting a `?` out of a block would take it out of the scope of that block's own
binders. The conditional rule is about what runs.

**The purity predicate is the one normalization already uses** to license
reordering a run of pure bindings, and it was **moved** rather than duplicated.
**The lift is a reordering, the commutation is a reordering, and the licence is
the same one: *a failure that happens in one order happens in every order*, and
a call or a perform breaks that.** Two implementations could drift, **and a
drift would mean normalization reordering something `?` refused to, or the
reverse.** The move was free — the pinned normalization digest is unmoved.

**Refusal rather than hoisting.** Normalizing an impure prefix into synthetic
bindings is complete and order-preserving, **and it is rejected for ADR 0023's
reason, verbatim in shape: restricting the sugar to a case where it *is* the
longhand is what makes it one definition. Hoisting introduces a second expansion
shape nobody wrote, which is a second thing to review.**

**One rule turned out to be partly redundant, and saying so is cheaper than
discovering it later.** Conditional branches are always blocks here, and the
scan does not enter a nested block for the unrelated scope reason. **So the
conditional ban does the work that is uniquely its own at a match arm and at the
right operand of a short-circuit, which are not blocks.**

## 5. Two codes, and no third

One for *this file gives `?` no meaning here* — no written return type, a head
that is neither of the two types, a type parameter or a cross-module name, or a
position with no readable return type at all (a lambda, a handler clause, a
region, a test, a law, a spec clause) — with a reason enum so the note names the
rule the writer hit. And one for *the early exit would change what runs, or
would discard something written*: an annotated binding whose whole value is a
`?` is refused, because the expansion has no binding left to carry the
annotation. **Measured cost of that refusal is zero** — the corpus's three
annotated bindings are all one shape, and a `?` *inside* an annotated binding's
value is fine, because the binding survives the split.

**There is deliberately no third code and no typing rule.** By the time
inference runs, it *is* the match, so a mismatched error type is an ordinary
type error. **`?` does no error conversion**: there is no dispatch mechanism in
Ply and this does not invent one, and the corpus's eight error-mapping sites are
left alone.

*The design said the mismatch lands at the `?`'s span, and as first built it did
not* — the synthesized match took the *operand's* span. The pass now carries the
`?`'s own span into the match and both arms. The block-shaped case is **not** a
defect and is not fixed: what disagrees there is the *function body* against its
declared return type, so the span is the body, **and a hand-written match in the
same place gets exactly the same diagnostic.** The test asserts both halves
rather than the stronger claim the design made.

**The refusal for a module that declares its own constructors of the two types
is not hypothetical.** Constructor names are not reserved, so it is writable
today. No corpus module does it; every `?` in one that does is refused rather
than captured.

## Consequences — the measurement

**139 sites converted. No definition hash moved**, over both corpora, taken
twice and byte-identical both times, with every hash run on a binary verified
fresh by a check that covers Ply sources as well as Rust ones — **the standard
library's modules are compiled in, so an `.rs`-only check is blind to a stale
one, and ADR 0023's own corrected paragraph is what happens when that is
missed.**

**Zero moved is a claim about the gate as much as about the change.** With the
two arms emitted in the other order, the same conversion moves nearly a third of
one corpus. **Zero against that is what makes "the module's behaviour is
unchanged" a measurement**: identical hashes are identical definitions, one
cache entry, nothing re-run.

**No version bump**, for three independently checkable reasons: no Ply file in
the tree contained a `?` outside a string or comment before this, so the new
token changes no existing file's token stream; the normalizer gained one
unreachable arm and lost the moved predicate, and no byte of the encoding moved;
and **the deriver emits no `?`**, so the pinned generated-form tests keep their
text verbatim. **Putting `?` into generated code *would* be a version bump** —
gate 1 keys on raw file content, so a file whose bytes did not change would
reuse a stale generated definition — **and it is deliberately not in this
change.**

**One site refused, and it was not predicted.** A shipped example writes the
match **inside a fold's lambda**; `?` refuses a lambda, the compiler said so,
and the site was reverted. *The design phase recorded, as an auxiliary check,
that none of the convertible sites sits under a lambda. That is **wrong, and
wrong in the reassuring direction**: it is one, and the lambda restriction now
has a measured cost of exactly one shipped site — **a better number than "zero"
because it is true.***

## What this does *not* do

**It does not help the JSON module much, and the brief expected it to.** Counted
on the file: of its two-arm fallible matches, well under a third convert. Two
are the combinators the deriver depends on; four **map their error**, which `?`
cannot express; and the remaining eleven are tests and predicates that *inspect*
a result rather than bind one, **so there is nothing for `?` to do in them.** On
top of that, most of the codec half lives inside lambdas.

*The brief, and this record's first draft after it, gave a denominator that was
too small with a breakdown that leaves three unaccounted. Both were re-derived
with a brace-balanced scanner. **The figure that matters was right; the
denominator was not, and a breakdown that does not add up is the shape of a
number nobody re-derived.***

**It will never convert an error.** As pure sugar it cannot call a conversion
that does not exist. **If the project later wants error mapping at `?`, this
sugar cannot grow into it**: that needs a dispatch mechanism there is none of,
by decision, or a typed node. **Anyone reading `?` as Rust's will hit this.**

**A lambda is the restriction most likely to be regretted.** A closure returning
a fallible value is idiomatic in every language that has `?`. **The line is
defensible — a lambda has no written return type, so there is nothing to read
the mode off — and it costs one shipped site.** The lift path exists and is
stated: give lambdas a written return type, and `?` inside one becomes legal
with **no change to the expansion.**

**This is the wrong design outright if Ply grows a real early return or
exceptions.** Then the node exists anyway and the parser expansion is throwaway.
**Related: the number-chain finding is evidence that the *actual* missing feature
there is early return, and `?` is the smaller half of that problem.**

**The residual risk is a float, and it is silent.** Everything else either works
or produces a diagnostic. **If the position rules are wrong, `?` moves a perform
across a branch and the program *runs*, differently, with the same types and the
same row — a row is a set and does not see order.** What closes it in practice is
that the pass is small, its refusals are total, and the conditional rule has
been **seen to fail on a running program** — not that anything checks the
transformation against an independent account of what should have run. **The
same honest statement ADR 0023 makes about too-narrow record updates.**

## What was seen to fail

*A gate nobody has watched fail is not a gate.* Every corruption was applied,
the named test watched red, and the corruption removed: dropping the failure
arm; the failure arm losing its constructor; swapping arm order — **which also
moves hundreds of corpus entries**; widening the region to the whole block;
dropping the binding case; deleting the hygiene check; making the pass a no-op;
deleting the conditional check — **which reddens a test *and a running
fixture***; swapping the two modes; deleting the cross-module check; taking the
operand's span; making the failure arm's pattern unmatchable; making the bare
entry point a no-op; and making the purity predicate call an application impure.

**All 33 tests this change adds have been watched red at least once, and that is
a count taken by re-running every corruption and unioning the failures, rather
than a recollection.** The first pass of that union came back one short: the
missing test's subject is the *commutation* rather than `?` — **and the
corruption it wants is the one that argues for sharing the predicate at all.**
Make the predicate call an application pure and **both callers' gates fire at
once**, which is the property the move was for.

**Two deserve their own paragraph.**

**Replacing the purity predicate with a constant *true* left every test green.**
The gate as first written used a call and a perform prefix — **and the scan
answers "impure" for those *structurally*, without consulting the predicate.**
The predicate is only consulted in four positions, so the mutant was invisible.
Four cases in those positions were added and it is now red. **Reported rather
than quietly repaired because it is exactly the defect this project ships
repeatedly: a green result over unexplored space.**

**The conditional mutant was shown non-equivalent, not merely red.** A `?` whose
operand *performs*, in a match arm: under the shipping compiler it is refused;
under the mutant it compiles and the test fails with a wrong answer, **because
the operand ran on the path that does not reach it.** A pure operand would have
made the mutant equivalent, **and an equivalent mutant is not a hole.**

**The escape guard is armed.** It parses every Ply file in the repository
**plus an appended file that actually writes `?`** — including three written
where they are refused, so the refusal path is covered too. **No file in the
tree wrote a `?` before this change, so without the appended file the guard
would pass whether or not expansion ran at all.** That is the record-update
guard's own comment, and the same trap.

## What the adversarial review found

Four gates were green over unexplored space, and one behaviour was wrong.

- **The call-argument scan walks left to right and nothing tested the
  direction.** Reversing it reddened **zero** of 354 tests, and made the exact
  shape the guide spells out as refused compile and **drop an argument's
  effect.** Now gated by a test asserting the refusal **and** the lift, **because
  only the pair pins an order.**
- **The bare statement form's canonical shape.** Deleting its arm reddened
  **zero**; no corpus file writes one, so the hash gate is silent, **and the
  compiler still ran the program correctly while emitting a different definition
  with a different hash.**
- **Three of the six no-meaning barriers.** Removing any one reddened **zero**;
  a `?` there answered the *other* code, against the guide's own claim that
  every one of them is the first.
- **A selective import of a constructor captured the expansion.** The hygiene
  check inspected only type declarations, **but a selective import binds a
  constructor unqualified in the same namespace** — so the author got errors on
  a match they never wrote, pointed at their own `?`. **The only behaviour change
  here**, turning a confusing cascade into the right refusal with a note saying
  to qualify the import. It moves no hash, **and the corpus was re-hashed after
  it to say so rather than assume it.**

**What the review tried and could not break.** Hashing a module written both
ways, on five shapes none of this change's tests use, gives one digest per pair,
and a doubled `?` agrees with its nested longhand. The corpus conversion was
re-measured independently from the pre-conversion files: **zero moved entries in
both corpora, with the instrument shown live by swapping one longhand's arms and
watching its digest move.** Two thousand sequential and sixty nested `?` in one
module compile and run on both engines. Every runnable example in the guide was
executed under the differential, and every refusal example produces exactly the
code the guide names for it.
