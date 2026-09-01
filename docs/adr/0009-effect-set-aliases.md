# ADR 0009 — Effect-set aliases

Status: accepted — implemented in W3, **amended by W6**. Decision 1 as first
written allowed an alias to name a whole effect; the implementation refuses it,
and W6 corrected this document rather than the code, because the reason the
implementation found is better than the reason this ADR gave. The rejected form
is no longer shown except as the `E0114` example it now is; see decision 1 and
decision 5.

Amended by: `docs/adr/0013-w3-contract.md` §1.2 (a member is an atom, never a
whole effect — the argument in full, including why a wildcard atom is not the
alternative) and §1.3 (sets are module-local).

`crates/ply-syntax/src/effect_set.rs` is the implementation; decision 3 holds by
construction, because `expand` splices atoms into the row and the set's name
reaches no later pass.

## Context

Resource-granular effects are what make footprint scheduling, world isolation
and interleaving reduction work. They are also unreadable at the surface once a
definition touches more than two or three resources. A realistic route handler:

```ply
fn create_order(req: Request) -> Response
  / {db.read[users], db.write[orders], db.read[inventory],
     http.write[outbound], log.write, clock.read, random.read}
```

Multiply by a hundred endpoints and the annotation becomes noise that nobody
reads, which is worse than no annotation — it trains people to skip the one part
of the signature that carries the safety properties.

Inference already handles this: omit the row and it is computed. But a published
signature is the thing a reviewer reads, and "omit it everywhere" gives that up.

## Decisions

### 1. A named set expands to a union of rows

```ply
effect set Web = {db.read[users], db.write[orders], log.write, clock.read}

fn create_order(req: Request) -> Response / {Web, random.read} = ...
```

A set's members are **atoms** and other sets declared in the same module.
Expansion is purely syntactic and happens before inference.

An alias may **not** name a whole effect. `effect set Web = {db}` is `E0114`,
and the diagnostic gives the reason:

```
Note 1: `db` is an effect, and a member of a set is an atom:
        write `db.read[..]` or `db.write[..]`
Note 2: a whole effect is every resource label anywhere in the program, so
        naming one would let an unrelated module change this row and therefore
        this definition's hash
```

That is a stronger reason than legibility, and it is the one this ADR should
have given. `{db}` has no fixed expansion: it means every `db` atom in the
program, so adding a table in an unrelated module silently widens every row that
names the set — widening every footprint, coarsening the conflict graph, and
**changing the hash of every definition that carries it**, which re-runs their
tests for an edit they do not depend on. Decision 3 says an alias never enters a
hash; a whole-effect member would have made that impossible to keep.

### 2. The alias is annotation-only; the precise row is what exists

Inference produces the exact atom set as it does today. The alias is checked as
an upper bound in exactly the way a written row already is — the inferred row
must be a subset of the expansion.

Nothing downstream ever sees the alias. Scheduling, isolation, interleaving
reduction, the determinism check and the footprint stored in the cache all
operate on the precise row. An alias buys legibility and gives up no precision,
which is the only trade worth making here.

### 3. An alias never enters a hash

Alias names are namespace metadata, exactly like module names and import
aliases. Defining an alias, renaming one, regrouping which atoms it contains, or
rewriting a signature from an explicit row to an equivalent alias must change
**no definition hash anywhere**.

This follows directly from ADR 0001's rule that the namespace is metadata over
hashes, and it is a required test: two definitions that differ only in whether
their row is written out or named must hash identically.

### 4. The expansion is the default; the alias is what `--explain` adds

An alias that hides what a definition touches would defeat its own purpose, so
the implementation inverted this decision's emphasis and is right to have done:
`ply check --types` prints the **expansion**, with no flag and with the alias
name nowhere in the output. `--explain` adds back the provenance the expansion
alone cannot show:

```
$ ply check examples/desk.ply --types --explain      # type line elided
  recorded : (…) -> (…)
           / {std.db.db.read[items], std.db.db.write[items],
              std.db.db.read[orders], std.db.db.write[orders],
              std.db.db.write, std.trace.trace.write[items],
              std.trace.trace.write[orders]}
    written as     / {Desk}
    body performs  {std.db.db.write[items], std.db.db.write[orders],
                    std.db.db.write, std.trace.trace.write[items],
                    std.trace.trace.write[orders]}
    declared, not performed: std.db.db.read[items], std.db.db.read[orders]
```

The third line is not in this ADR's original design and is what makes the risk
named under Consequences checkable rather than merely visible. An alias wider
than the body carrying it is legal and costs two things (ADR 0013 §1.6): it
widens the conflict graph, so tests that could have run side by side are
serialised, and it weakens every frame condition, since a footprint is what an
`ensures` promises nothing outside of. `declared, not performed` is that gap,
per definition, in the default reviewing command.

### 5. A set is private to the module that declares it

Added by W6 from the implementation. `pub effect set`, the qualified form
`other::Web`, and naming a set no local `effect set` declares are one error —
`E0114` — because they have one fix: declare the set in this file.

The reason is the same one decision 1 turns on, and it is about incrementality
rather than encapsulation. Gate 1 skips a file whose raw bytes are unchanged
(ADR 0002), so expansion has to be a **function of the file**. A set expanding
across a module boundary would let an edit in the declaring module leave a stale
published row behind in a file that never moved — a stored footprint that
under-reports, which corrupts scheduling and isolation *silently*. That is the
failure mode ROADMAP's "risk that matters" names, arriving through the most
innocuous-looking feature in the web track.

The cost is real and is paid knowingly: a set common to twenty modules is
written twenty times. Expansion is textual and hashes identically each time
(decision 3), so the duplication is in the source and nowhere else.

### 6. A set that contains itself is `E0115`

Expansion is a fixed point and a cycle has none. Every set on the cycle is
reported once, with its members in the order they contain each other, and a
cyclic set contributes no atoms rather than looping — so one bad declaration
yields one diagnostic instead of a cascade.

## Consequences

Signatures become readable at web-application scale without weakening any
property they carry. The cost is one more layer between what is written and what
is meant, which is why decision 4 exists.

Risk worth naming: an over-broad alias used everywhere degrades signatures back
toward `IO`. Nothing prevents `effect set All = {...every atom...}`, and a
codebase that does that has given up the benefit while keeping the syntax. Two
things bound it in practice, and neither existed when this ADR was written.
Decision 1 means such a set has to enumerate every atom by hand, so it cannot be
written once and then widen silently. And decision 4's `declared, not performed`
line reports the gap per definition, so an over-broad alias is a countable
finding rather than a judgement call, and on the shipped example it fires for a
handful of definitions out of hundreds.

## Not in this ADR

Aliasing over row *variables* — `effect set Handler<e> = {db.read[users],
log.write | e}`. Useful for higher-order handler combinators, more design than
the web track needed, and W3 through W6 shipped a service without wanting it.
