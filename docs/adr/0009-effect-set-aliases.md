# ADR 0009 — Effect-set aliases

Status: proposed

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
effect set Web = {db, http, log, clock}

fn create_order(req: Request) -> Response / {Web, random.read} = ...
```

An alias may name whole effects or individual atoms, and may include other
aliases. Expansion is purely syntactic and happens before inference.

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

### 4. `--explain` shows the expansion

```
$ ply check --types --explain
  create_order : (Request) -> Response
    / {Web, random.read}
    = {db.read[users], db.write[orders], db.read[inventory],
       http.write[outbound], log.write, clock.read, random.read}
```

An alias that hides what a definition touches would defeat its own purpose. The
expansion has to be one flag away, and the reviewing commands should print it
rather than the alias.

## Consequences

Signatures become readable at web-application scale without weakening any
property they carry. The cost is one more layer between what is written and what
is meant, which is why decision 4 exists.

Risk worth naming: an over-broad alias used everywhere degrades signatures back
toward `IO`. Nothing prevents `effect set All = {...everything...}`, and a
codebase that does that has given up the benefit while keeping the syntax. This
is a review concern rather than a language one — but `ply check --explain`
makes it visible, and an alias whose expansion is most of the program is a
finding a reviewer can act on.

## Not in this ADR

Aliasing over row *variables* — `effect set Handler<e> = {db, log | e}`. Useful
for higher-order handler combinators, more design than the web track needs now.
