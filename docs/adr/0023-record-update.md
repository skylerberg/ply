# ADR 0023 — Record update

**Accepted, implemented.** Constrained by ADR 0001 (a definition's identity is
its normalized structure), ADR 0002 (gate 1 skips a file on its content hash
alone), and ADR 0009's precedent for expanding inside the parser and accepting
module-locality as the price.

## Context

Changing one field of a record meant writing every other field out, which cost
the lexer spike three functions that exist only to spell a field list once each,
and cost the HTTP limits helper thirteen lines to change one bound.

**The thirteen-line one is the interesting case.** All thirteen fields of the
limits record are integers, so pairing the wrong two type-checks, **and is a
silently wrong bound in an HTTP server.**

## The constraint that shapes everything

The requirement is not "record update should work". It is:

> **`{..s, a: 1}` must be the same definition as the record literal it stands
> for.**

If it were not, every definition that adopted the sugar would move, and the two
spellings of one value would become two definitions each re-running its own
tests. Asserted, not argued.

## 1. Expansion runs inside the parser

An unexpanded update never escapes `parse_module`, and the expression entry
point — which has no module around it — runs the same pass with an empty context
so that a spread there refuses rather than leaking.

**Not in normalization, which is the obvious place.** The driver hashes *before*
it infers, which ADR 0002 pins deliberately, so normalization has no types and
cannot enumerate the base's fields. And changing the normalizer's byte stream is
a cache-format change that moves every cached result everywhere.

**Not in each consumer.** A surviving node in the hasher, the checker and both
evaluators would be four implementations of one construct — **the exact shape
ADR 0001 rejected `.` for qualified references over: four implementations, four
chances to disagree.** Instead those crates carry unreachable arms, **and the
arms are safe because the escape is *checked***: a test parses every `.ply` file
in the repository plus a file that uses the syntax, on both entry points, and
asserts none survives.

The prover is the one that does not assert unreachability. **A prover's safe
answer is "I did not reason about this" and never a term it guessed**, so its
arm records a blocker — deliberately a *sugar* blocker and not a region one,
because a record update is not a perform or a handler and the label reaches a
reader. The arm is unreachable today, so no report can contain it and no test
can watch it fire; **what *is* checked is the other half** — the label function
matches the blocker enum exhaustively with no wildcard, so a variant cannot be
added without a row to print and deleting a row fails the build.

## 2. The canonical expansion

Copies of the unwritten fields, **sorted by field name**, then the written
fields **in the order written**. Both halves are load-bearing.

**Sorted, not the type's declaration order**, because reordering a record type's
fields is an invariant the suite asserts as free — the normalizer sorts them.
Expanding in declaration order would make reordering a type move the hash of
every update written against it.

**By name, and that has to be pinned by a test that can see the difference.**
Every field name in a suite written `a`/`b`/`c` orders identically under sorting
by name and under any comparator that compares length first, **so such a suite
says nothing about which one ran.** One mixed-length pair is not enough either:
it only rules out the length direction it happens to disagree with. So each
pinning test carries a pair whose longer name sorts first *and* a pair whose
shorter one does, and a length-first comparator in either direction goes red.

**Written fields last**, because a growing sub-expression in any but the last
position of its enclosing node is quadratic. Copies are pure field reads and
never grow, so emitting them first is free, **and it puts the common
single-write update on the linear spelling.**

## 3. The base is a path, not an expression

The expansion writes the base once per copied field, exactly as the longhand
does. **A base with a call or a perform in it would run that base once per
field** — thirteen times for the limits record. Restricting it to a repeatable,
pure path is what makes the sugar and the longhand the *same definition* rather
than merely the same value. The lift, when a call base is wanted, is one `let`
and needs nothing from the compiler.

## 4. The shape is read from this module only

Expansion reads this module's own type items and the annotations written in this
file. A base whose type comes from another module is refused.

**The same restriction ADR 0009 accepted for effect sets, for the same reason:**
gate 1 skips a file whose raw bytes are unchanged, so a file's hashes must be a
function of its own bytes. A shape read across a module boundary would let an
edit in the declaring module leave a stale expansion behind in a file that never
moved — **and a stale expansion is a *wrong record*, not merely a stale name.**

**Counted before the restriction was accepted, rather than assumed to be
small.** A scan for the shape a record update replaces finds most candidate
sites have the base's type declared in the same file, and most of the remainder
are the scan failing to resolve a nested path rather than genuine cross-module
bases. **So the restriction costs on the order of one site in ten, not most of
them, which is what makes it a deferral rather than the wrong shape of feature.**
Lifting it needs a two-pass resolution plus a cache key over the declaring
module's *shape* rather than over the importing file's bytes.

Also refused, each with its own note: a generic alias, which has no field list
until it is applied; a sum type; an alias chain past a bound (a bound rather
than a cycle check, because this runs before anything has rejected a cyclic
alias); and a binder with no written type — **including one that *shadows* an
annotated binder**, because expanding against the outer binder's shape would be
a record of some other type's width, and the reader would be looking at a
diagnostic about a literal they did not write.

## 5. Two codes, and no third

One for a base with no record shape this file can name, one for a field the base
does not have — **an update replaces; it does not widen, because the result's
type is the base's type.**

**There is deliberately no typing rule for a record update and no third code.**
By the time inference runs it *is* the literal, so it meets the same exact
key-set unification every record literal meets. Too wide surfaces as an unknown
field; too narrow surfaces wherever the result meets a known record type.

**The second is not total, and this says so rather than dressing it up**: an
update whose result is never constrained by any annotation would go unnoticed.
What closes it in practice is that the shape is computed from the *same written
annotation* inference uses to type the binder, so there is no independent "real"
field set to disagree with — **the residual risk is a bug in this pass, not a
program a user can write.** A per-node assertion is **not enforced.**

## Consequences

The thirteen-line helper is one line. Hashes moved once, **and the moved set is
the transitive dependent set exactly** — nothing moved that is not a dependent,
and no dependent failed to move. **No version bump**: nothing about the
normalizer's byte stream changed and no stored type moved, measured rather than
assumed by hashing the base corpus with the new compiler and getting zero moved
entries.

**Two figures here were wrong, and the first was wrong in the reassuring
direction.**

**A zero is what a stale binary reports, and this is the trap.** The standard
library's Ply modules are compiled *into* the binary, so an import resolves to
the copy compiled in and never to the file that was edited: **re-running the
hash command after editing a stdlib module without rebuilding compares a binary
against itself.** The prescribed instrument check could not see it either,
**because the stale input is a `.ply` and that check looked only at `.rs`.** Now
closed by a check that reads the compiler's dependency info.

**And a second reading of "how many moved" disagreed with the first for a reason
that is entirely about the key.** Hashing the whole stdlib directory lists the
edited file **twice** — once as its own module and once under the `std` prefix,
reached through another module's import — and keying the comparison on the
*bare* name **deduplicates one half and not the other**: a definition's name
already carries its module, so two rows survive, while a test's name is its
title, so titles collapse. **Definitions doubled, tests not.** Keyed on
`(module, name)` the same corpus reports a third figure. **All of them are
artefacts of the corpus and the key. One file, one honest denominator: hash the
file itself.**

## What this does *not* do

**The positional trap is narrowed, not removed.** The expansion emits copies
first and written fields last, and a copy never grows, so a single-write update
whose value grows is *always* on the linear spelling — the expansion has nothing
it can put after it.

What remains is the several-writes case, and it is worth stating in full rather
than as a caveat: a growing field written before another written field is
**still quadratic**; putting it last is linear and computes the same record;
**and the two are not the same definition**, because written fields are emitted
in the order written and field order is part of a record's hash — so reordering
moves the hash and re-runs the tests that reach it. **The linear spelling is
available and it is not free.**

So the rule a writer must keep is narrower than before — *a growing field must
be written last among the fields you write* — but not empty. **It is, however,
syntactic**: which written field grows and where it sits are both readable at
the update site with no types, **which is what makes it lintable.** This does
not implement that lint and does not claim the trap is gone.

**It does not fix the constructor that builds the record from nothing**, and it
cannot: there is no base to update, so it must go on naming every field. **It is
the one irreducible spelling and the residual place a wrong *constant* can hide
— and it is now the only such site.**

Three helpers were held back so the hash-movement criterion could be read
against one function alone. **That criterion is established, so the deferral was
spent** — all three now take the lift, and the moved set is still exactly the
transitive dependent set. **Deferring them was the right order to do the work in
and the wrong place to stop**: a site left hand-written is a site that can be
mispaired, and one helper's twelve unvaried bounds were mispairable in silence —
crossing two of them left the checker reporting a full green count and every
targeted suite passing. **What is asserted now is the property, not the
spelling.** One mispairing survives conversion and is named rather than implied:
a helper writing seven bounds from seven integer parameters can still cross two,
and the guard left is a naming convention, **which is asserted rather than
trusted.**
