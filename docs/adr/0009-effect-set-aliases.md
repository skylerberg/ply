# ADR 0009 — Effect-set aliases

**Accepted, implemented.**

## Context

Resource-granular effects are what make footprint scheduling, isolation and
interleaving reduction work. They are also unreadable at the surface once a
definition touches more than two or three resources:

```ply
fn create_order(req: Request) -> Response
  / {db.read[users], db.write[orders], db.read[inventory],
     http.write[outbound], log.write, clock.read, random.read}
```

Multiply by a hundred endpoints and the annotation becomes noise nobody reads,
which is worse than no annotation — it trains people to skip the one part of the
signature that carries the safety properties. Inference already handles this by
omission, but a published signature is the thing a reviewer reads, and "omit it
everywhere" gives that up.

## 1. A named set expands to a union of rows

```ply
effect set Web = {db.read[users], db.write[orders], log.write, clock.read}

fn create_order(req: Request) -> Response / {Web, random.read} = ...
```

A set's members are **atoms** and other sets declared in the same module.
Expansion is purely syntactic and happens before inference.

**An alias may not name a whole effect.** `effect set Web = {db}` is an error,
and the reason is stronger than legibility: `{db}` has no fixed expansion. It
means every `db` atom in the program, so adding a table in an unrelated module
would silently widen every row that names the set — widening every footprint,
coarsening the conflict graph, and **changing the hash of every definition that
carries it**, which re-runs their tests for an edit they do not depend on. A
whole-effect member would have made "an alias never enters a hash" impossible to
keep.

## 2. The alias is annotation-only; the precise row is what exists

Inference produces the exact atom set as it does today, and the alias is checked
as an upper bound in exactly the way a written row already is. Nothing
downstream ever sees the alias — scheduling, isolation, interleaving reduction,
the determinism check and the stored footprint all operate on atoms. **An alias
buys legibility and gives up no precision, which is the only trade worth making
here.**

## 3. An alias never enters a hash, and its expansion enters exactly as the row would

Alias names are namespace metadata, exactly like module names and import
aliases. Declaring a set, renaming one, reordering its members or writing one
twice all move **no** hash anywhere; rewriting an explicit row as an equivalent
alias moves none either, which is the headline property.

**Changing which atoms a set contains moves exactly the definitions annotated
with it, and their transitive dependents — and that is not a concession.** A
`/ {..}` annotation is the *published* signature, so widening a set widens the
bound every definition annotated with it publishes, and a caller checked against
the narrower bound has to be rechecked. Gate 2 only rechecks a definition whose
own hash moved, so a set edit that moved no hash would leave that caller
accepted against a signature that no longer admits it, with a stored footprint
under-reporting what it can now reach.

## 4. The expansion is the default; the alias is what `--explain` adds

An alias that hides what a definition touches would defeat its own purpose, so
the reviewing command prints the **expansion**, with no flag and with the alias
name nowhere in the output. `--explain` adds back the provenance the expansion
alone cannot show — the set's definition, the row the body actually performs,
and the difference:

```
    written as     / {Desk}
    body performs  {...}
    declared, not performed: db.read[items], db.read[orders]
```

That last line is what makes the risk below checkable rather than merely
visible.

## 5. A set is private to the module that declares it

`pub effect set`, a qualified reference to another module's set, and naming a
set no local declaration provides are one error, because they have one fix.

**The reason is incrementality rather than encapsulation.** Gate 1 skips a file
whose raw bytes are unchanged, so expansion has to be a **function of the
file**. A set expanding across a module boundary would let an edit in the
declaring module leave a stale published row behind in a file that never moved —
a stored footprint that under-reports, which corrupts scheduling and isolation
*silently*. That is the failure mode that matters most, arriving through the
most innocuous-looking feature available.

The cost is real and is paid knowingly: a set common to twenty modules is
written twenty times. Expansion is textual and hashes identically each time, so
the duplication is in the source and nowhere else.

## 6. A set that contains itself is an error

Expansion is a fixed point and a cycle has none. Every set on the cycle is
reported once, and a cyclic set contributes no atoms rather than looping — so
one bad declaration yields one diagnostic instead of a cascade.

## Consequences

Signatures become readable at web-application scale without weakening any
property they carry. The cost is one more layer between what is written and what
is meant, which is why decision 4 exists.

**The risk worth naming: an over-broad alias used everywhere degrades signatures
back toward `IO`.** Nothing prevents a set enumerating every atom in the
program, and a codebase that does that has given up the benefit while keeping
the syntax. Two things bound it, and neither existed when the risk was first
written down. Decision 1 means such a set has to enumerate every atom by hand,
so it cannot be written once and then widen silently. And the
declared-not-performed line reports the gap per definition, **so an over-broad
alias is a countable finding rather than a judgement call.**

Two mechanical costs of a wide alias, so they can be measured rather than
worried about: the conflict graph widens, because the declared row is what
scheduling reads, so two tests reaching two endpoints that share a set contend on
every atom in it; and every frame condition weakens, since the footprint *is*
the frame condition, so a wider footprint promises less about less.

## Not done here

Aliasing over row *variables* — `effect set Handler<e> = {db.read[users] | e}`.
Useful for higher-order handler combinators, more design than the web track
needed, and a whole service shipped without wanting it.
