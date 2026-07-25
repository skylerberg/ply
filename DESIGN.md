# Ply — Design

## Thesis

Generating code is becoming free. Knowing whether it is correct is not. Ply is a
language designed so that the *verification* loop — build, select, set up, judge
— collapses toward zero, and so that what remains for a human to review is a
specification rather than an implementation.

Four costs dominate the loop today. Ply attacks each with a language-level
mechanism rather than a tooling workaround:

| Cost | Today | Ply |
| --- | --- | --- |
| Build | Recompile modules on any touch | Content-addressed definitions; a definition compiles once, ever |
| Selection | File graph over-approximation | A test re-runs iff its hash is absent from the cache — exact |
| Setup | Migrate, seed, boot, per test | Effect handlers replace real resources with in-memory ones, checked to share a signature |
| Signal | Flakes discovered at run 400 | Nondeterminism is in the type; a flaky test fails to compile |

The keystone is the effect system. Everything else is downstream of knowing,
statically and precisely, what a piece of code can do.

## 1. Effects

A function's type carries an **effect row**: the set of things it may do.

```ply
fn active_users() -> List<User> / {db.read[users]} = ...
```

### Atoms

An effect row is a set of **atoms** plus an optional row variable tail. An atom is:

```
(effect: Symbol, resource: Option<Symbol>, mode: Read | Write)
```

Resource granularity is the design contribution. Most effect systems track
`db` as one capability. Ply tracks `db.read[users]` distinctly from
`db.write[orders]`, because that is exactly the information needed to decide
whether two tests can run concurrently.

Two footprints **conflict** iff they name a common resource and at least one
access is a `Write`. Readers-writers, applied to test scheduling.

### Declaring effects

```ply
effect db {
  read  get[r](key: Int) -> Option<Row>
  write put[r](key: Int, value: Row) -> Unit
}

nondet effect clock {
  read now() -> Int
}
```

`read` / `write` are mode annotations. `[r]` marks an operation as
resource-parameterized: `db.get[users](3)` performs the atom `(db, users, Read)`.
Operations without `[r]` use a singleton resource named for the effect.

`nondet` marks an effect whose results are not a function of the program state.
This is what makes flakiness statically detectable.

### Rows and polymorphism

Rows are `{a1, a2, ... | ρ}` where ρ is a row variable or closed. Because atoms
are ground labels, unification is set unification with a tail variable — much
simpler than general row polymorphism over typed fields:

```
{A | ρ1} ~ {B | ρ2}
  ⟹  ρ1 := (B \ A) ∪ ρ3,  ρ2 := (A \ B) ∪ ρ3     (ρ3 fresh)
```

with an occurs check on row variables. Effect-polymorphic functions thread the
tail:

```ply
fn map<a, b, e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e = ...
```

If a function omits its `/ {...}` annotation the row is inferred. If it carries
one, the annotation is the published signature and inference must produce a
subset of it.

## 2. Handlers

Handlers discharge effects. They are how a real resource is swapped for an
in-memory one — with no mock library, no injection ceremony, and no drift,
because both satisfy the same declared signature.

```ply
handle body() with {
  db.get[users](k)    -> map_lookup(ref_get(cell), k)
  db.put[users](k, v) -> ref_set(cell, map_insert(ref_get(cell), k, v))
  return x            -> x
}
```

**Typing rule.** The `handle` expression's footprint is

```
(footprint(body) \ handled_atoms) ∪ ⋃ footprint(clause_i)
```

That second term is what makes this honest: a handler backed by a real socket
still reports network access. A handler backed by a test-local cell reports
nothing that escapes the test, which is precisely why the test is provably
isolated.

**v0 restriction: tail-resumptive only.** A clause body's value is returned
directly to the perform site; there is no reified `resume`, so no continuation
capture. This covers state, reader, writer, and every in-memory test double —
the cases that matter for the thesis. Multi-shot continuations are M6.

**State in handlers** uses region-scoped cells, a builtin rather than a
user-level effect so that its atoms are discharged at the region boundary and
cannot escape (the ST-monad trick):

```ply
with_cell[users](initial) { cell -> handle body() with { ... } }
```

## 3. Content addressing

The unit of compilation is the **definition**, not the file or module.

```
normalize(def):
  locals            → de Bruijn levels        (renaming a local changes nothing)
  free references   → the referent's hash     (not its name)
  names, spans, comments, formatting → erased
  structure, literals, declared signatures    → kept
  serialize postorder, length-prefixed

hash(def) = BLAKE3(normalize(def))
```

Mutually recursive definitions are hashed as a strongly-connected component:
self-references become component-local indices, the component is hashed as a
whole, and each member's hash is `BLAKE3(component_hash ‖ index)`.

Effects are the one nominal thing in the language — `db` and `audit` may declare
byte-identical operations and are still different capabilities — so a reference
to one carries its **slot** in the enumeration of the effects that definition's
component can reach, alongside the declaration's hash. The slot is a de Bruijn
level for effects: it says which of the effects in view is meant without naming
it and without consulting anything the definition cannot reach. Two definitions
that differ only by a consistent renaming of their effects therefore hash alike
— they are one computation — while any context that can tell the two apart
reaches both and records which one it picked. A rank over the program's effect
names would separate them, at the price of making a definition's identity depend
on modules it does not import; that is not a price content addressing can pay.

A **codebase** is a content-addressed store `Hash → (Definition, Type, Footprint)`
plus a **namespace** mapping names to hashes. Renaming edits only the namespace.
Because the referent's *hash* is substituted for its name, renaming a function
changes no hash anywhere in the program — so nothing rebuilds and nothing
re-tests. That is the sharpest demonstration that selection is exact rather than
conservative.

## 4. Tests

`test` is a language construct, not a discovered function with a decorator.

```ply
test "active_users excludes inactive" {
  with_cell[users](seed) { cell ->
    handle {
      assert_eq(len(active_users()), 2)
    } with { db.get[users](k) -> map_lookup(ref_get(cell), k) }
  }
}
```

Tests are hashed like any other definition, so their hash already transitively
covers every definition they reach.

**Cache.** `(runtime_version, test_hash) → Pass | Fail(report)`. A pass is valid
forever unless something in the closure changes. Not "probably skip" — provably
unnecessary.

**Selection.** Run iff the hash is absent from the cache. No file graph, no
heuristics.

**Scheduling.** Build a conflict graph over the selected tests' footprints and
greedily colour it. Tests with disjoint footprints, or that only read a shared
resource, run concurrently by construction rather than by convention.

**Determinism.** A `test` is `det` by default. If its footprint retains any atom
from a `nondet` effect after handling, that is a compile error:

```
error[E0412]: nondeterministic effect in a deterministic test
  ┌─ src/user.ply:42:13
42│     let now = clock.now()
  │               ^^^^^^^^^^^ performs `clock`, declared `nondet`
  = handle `clock`, or declare the test `test/nondet`
```

`test/nondet` opts out and is never cached.

## 5. Diagnostics for a machine consumer

Failure output is a structured artifact, not terminal prose. `ply test --json`
emits, per failure: the structured expected/actual diff, the footprint at the
point of failure, and — because the definition graph and the cache are both on
hand — the **suspect set**: the definitions whose hashes changed since the last
pass and that lie in the failing test's closure. Usually one or two entries.

The agent should not have to re-derive which of its twelve edits broke things.
The system already knows.

## Non-goals for the vertical slice

Native codegen (the v0 evaluator is a tree-walking interpreter), multi-shot
continuations, VM-level snapshot/fork, deterministic scheduling simulation, and
spec-derived property tests are all deliberately deferred. See ROADMAP.md — each
has a milestone, and the M0–M4 architecture is shaped so none of them requires a
rewrite.
