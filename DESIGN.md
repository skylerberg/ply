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

ADR 0017 generalizes that region to allocation. `with_region[r] { .. }` opens a
lexical allocation scope whose brand `r` appears in the types of the values
allocated in it, and a `with_cell[r]` written under one allocates into it rather
than opening a region of its own — so a cell is a value allocated in `r`, and
`with_cell` written on its own is unchanged. A value branded `r` that would
outlive the region is `E0446`, reported where it would escape.

The brand is a type-level claim, so it stops where the types do. Three runtime
boundaries hand a value somewhere no type is left to look at — a host operation's
argument, the value a host handler or runtime answers with, and an argument
handed to an entry point from outside the program — and a handle into a region
reaching one of those is `E0449`, which names the handle, the route to it and the
boundary. ADR 0017 §2's one deliberately open route, a continuation parked in an
enclosing region's cell where a constructor's field type erases the brand, stays
open; what `E0449` bounds is where the value it produces can then go.

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

**One qualification, from §6.** A `det` test may carry `sim.read`, because a seed
is an input rather than a nondeterminism. Its outcome is then a function of its
definitions *and* of the search that was performed, so it is keyed on
`(test_hash, plan)` and never on `test_hash` alone — a run under one plan must
not read a pass another plan earned. A search that spends its budget is reported
green and **not** cached: it proved nothing about the interleavings it did not
reach. That is the only green `det` test in the language that re-runs, and it is
correct that it does.

## 5. Diagnostics for a machine consumer

Failure output is a structured artifact, not terminal prose. `ply test --json`
emits, per failure: the structured expected/actual diff, the footprint at the
point of failure, and — because the definition graph and the cache are both on
hand — the **suspect set**: the definitions whose hashes changed since the last
pass and that lie in the failing test's closure. Usually one or two entries.

The agent should not have to re-derive which of its twelve edits broke things.
The system already knows.

## 6. Simulation

A `nondet` atom leaves the row when a handler discharges it, and a handler is
free to be supplied by the language. `simulate { .. }` installs one for the three
effects the language can model — `task`, `clock` and `random` — and does it with
no new typing rule: it is `handle` with a fixed clause set.

```ply
test "transfers are atomic under any interleaving" {
  simulate {
    let a = task.spawn(|| transfer(alice, bob, 50));
    let b = task.spawn(|| transfer(bob, alice, 30));
    task.join(a); task.join(b);
    assert_eq(balance(alice) + balance(bob), 100)
  }
}
```

Concurrency is an effect, so the scheduler is a test double like any other: the
signature is declared once and a production handler, a sequential one written in
Ply, and the seeded one cannot drift. A task is a suspended machine state, which
is what the explicit control stack bought.

**The seed.** A region's row gains `sim.read` — the seed dependency, in the type
— and everything the seeded handler produces is a function of that seed and the
requests made so far. A simulated run is therefore a pure function of its
definition set and its seed, which is why `clock.now()` becomes usable in an
ordinary `det`, cacheable test. A failure reports its seed and `ply test --seed
<n>` replays it exactly.

**The reduction is the point.** Two tasks whose footprints do not conflict
commute, so exploring both orders is provably redundant. Partial-order reduction
algorithms spend their complexity approximating that relation; Ply computes it
exactly, at resource granularity, with the same predicate that decides which
tests may run concurrently. When the search exhausts its frontier the result is
not a sample but a proof over every interleaving.

**Over every interleaving the scheduler could have chosen.** Tasks interleave at
the operations the scheduler answers — `task`, `clock` and `random` — so a task
that reads shared state and writes it back with none of those in between runs the
two as one step, and no schedule separates them. That is a real limit and ADR 0006
§3.3 states it: put a `task.yield()` in the window, or a `clock.now()` stamp the
code was going to write anyway, or push the check into the resource so there is
nothing to separate.

**What this costs, plainly.** Row four of the table above needs a qualification:
a test that depends on time, order or randomness no longer fails to compile — it
becomes a test over a seed set, and a green run is a claim about the seeds that
were run. The risk that a seed you did not run would have failed is real. It is
also visible on every run, often zero when the search is exhaustive, and widened
with one flag — where wall-clock flakiness was none of those.

`docs/adr/0006-deterministic-simulation.md` is the specification.

## 7. Specs

§1–§6 make the verification loop cheap. This section is the other half of the
thesis: making the thing a human reads a **specification** rather than an
implementation.

A definition carries pre- and post-conditions, and a module carries standalone
laws:

```ply
fn withdraw(acct: Account, amount: Int) -> Account
  requires amount > 0
  ensures result.balance == acct.balance - amount
= ...

law "credit and debit cancel"
  forall (a: Account, n: Int) where n > 0 && n <= a.balance {
    credited(debited(a, n), n) == a
  }
```

**A spec expression is pure — an empty effect row.** A spec that can perform
effects can change what it observes, and an obligation that mutates the world it
is judging is meaningless. The one exception is a law whose body is a `simulate`
region, whose row is `{sim.read}`; that is a claim about every interleaving and
§6 is what discharges it.

**A spec is a claim *about* a definition, not part of it.** Specs are erased by
normalization, so writing one changes no definition hash and re-runs no test —
the same sentence as "renaming a function re-runs no test", and true for the same
reason. The *claim* gets its own hash, which covers the definition's, so an
obligation invalidates when the implementation moves while the implementation
does not invalidate when the claim moves. That asymmetry is exactly the asymmetry
review has.

### Tiers

Each obligation is discharged at the strongest tier the system can **demonstrate**:

| tier | what it claims |
| --- | --- |
| `proved` | a static argument covering **every** input satisfying the guard |
| `property` | randomized cases, the count reported, shrinking on failure |
| `example` | concrete cases, and no coverage claim |

`proved` is a small, exactly-stated fragment — linear arithmetic over `Int`, case
analysis over ADTs, structural equality and congruence closure, and unfolding of
*non-recursive* definitions only. Recursion over unbounded data needs induction,
which is not here, so `reverse(reverse(xs)) == xs` is `property` and should be.
An inconclusive proof attempt reports `property`, never `proved`.

> **A tier label is a truth claim.** Every prior milestone could produce a wrong
> answer; only this one can produce a wrong answer wearing a certificate. When in
> doubt, report the weaker tier.

That is enforced structurally rather than by convention: there is no `tier` field
anywhere. A tier is computed from the evidence a discharge carries, and the only
evidence that computes to `proved` is a certificate naming the inference rules it
used — which only the prover can construct.

### Frame conditions are already inferred

The classic tarpit of program verification is the frame problem: an `ensures`
says what changed, and a caller needs to know what did not, so Dafny and Why3
make the user write a `modifies` clause and then prove it. Ply has computed that
set for every definition since §1 — it is the footprint, at resource granularity
— and it is checked as an upper bound by inference rather than asserted by a
user. So an `ensures` means *this holds of the result, and every resource outside
the footprint's writes is unchanged*, and the second half is not an obligation at
all. It is what the effect system has been paying for.

Ply also needs no `old()`: it is a value language, so the pre-state of
`withdraw(acct, amount)` is `acct`, still in scope and still exactly what it was.

### Coverage is the honest number

```
   41 definitions · 18 carry an obligation · 23 do not
   26 obligations · 7 proved · 16 property · 2 example · 1 unattempted
```

The count of definitions carrying no obligation is exactly the surface where
review still costs what it costs today, so it is in the default output of `ply
prove` and `ply review`, ahead of the results, and never behind a flag. Hiding it
would turn an honest tool into a misleading one: a project with three proved
obligations and four hundred unspecified definitions would print three green
ticks and invite a reviewer to stop.

`ply review --changed` is the artifact the milestone exists to produce. It
reports, per changed definition, whether the implementation changed, whether the
spec changed, and whether the obligations still hold — and the row that matters
is *implementation changed, spec unchanged*, where the review is reading the
obligations rather than the diff.

**What this is not**: a general-purpose theorem prover, an SMT integration, or a
termination checker. `requires` is a filter on the domain of the `ensures`
clauses beside it, not a contract checked at every call site.

`docs/adr/0007-specs.md` is the specification.

## Non-goals for the vertical slice

Native codegen (the v0 evaluator is a tree-walking interpreter), multi-shot
continuations, VM-level snapshot/fork, deterministic scheduling simulation, and
specs are all deliberately deferred. See ROADMAP.md — each has a milestone, and
the M0–M4 architecture is shaped so none of them requires a rewrite. §6 is M7 and
§7 is M8; both describe what those milestones land, not what the vertical slice
ships.
