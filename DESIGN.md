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

Two atoms **conflict** iff they name the same resource *of the same effect* and
at least one access is a `Write`. Readers-writers, applied to test scheduling.
The effect is part of the test because effects are nominal (§3): `db.read[users]`
and `audit.read[users]` name a resource with the same label and do not contend.
Two footprints conflict iff any of their atoms do
(`EffectAtom::conflicts_with`, `crates/ply-core/src/ty.rs:52`).

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
fn map<a, b | e>(xs: List<a>, f: (a) -> b / e) -> List<b> / e = ...
```

The `|` in the generic list separates type parameters from effect variables. A
bare `<a, b, e>` puts `e` in the type namespace and the row annotation is then
`E0301 unknown effect variable`, which is what this sample said until it was run.

If a function omits its `/ {...}` annotation the row is inferred. If it carries
one, the annotation is the published signature and inference must produce a
subset of it.

## 2. Handlers

Handlers discharge effects. They are how a real resource is swapped for an
in-memory one — with no mock library, no injection ceremony, and no drift,
because both satisfy the same declared signature.

```ply
handle body() with {
  db.get[users](k)    -> map_get(cell_get(cell), k)
  db.put[users](k, v) -> cell_set(cell, map_insert(cell_get(cell), k, v))
  return x            -> x
}
```

(The cell builtins are `cell_get` / `cell_set` and the map lookup is `map_get`.
This sample read `ref_get` / `ref_set` / `map_lookup` until it was run; none of
those three names has ever existed, and each is `E0101 unknown name`.)

**Typing rule.** The `handle` expression's footprint is

```
(footprint(body) \ handled_atoms) ∪ ⋃ footprint(clause_i)
```

That second term is what makes this honest: a handler backed by a real socket
still reports network access. A handler backed by a test-local cell reports
nothing that escapes the test, which is precisely why the test is provably
isolated.

**Resumption.** A clause written `op(x) -> body` is *tail-resumptive*: the body's
value goes straight back to the perform site, with no continuation captured. That
covers state, reader, writer, and every in-memory test double — the cases that
matter for the thesis, and still the shape nearly every shipped handler uses.
Across `examples/` and `crates/ply-std/ply/` exactly **one** clause of some 294
binds a continuation: `db.rollback(reason) resume k -> ...` in
`std.db.transaction`, which is how a rollback declines to resume its body.

> **Corrected.** This section read "**v0 restriction: tail-resumptive only** …
> there is no reified `resume` … Multi-shot continuations are M6" long after M6
> landed them. There *is* a reified `resume`: a clause may be written
> `op(x) resume k -> ...`, which binds the delimited continuation as `k` and may
> invoke it zero, one or many times — `amb.flip[coin]() resume k -> k(true) +
> k(false)` is the multi-shot case, and it parses and runs today
> (`crates/ply-syntax/src/ast.rs:1019`, `crates/ply-eval/src/handler.rs`).
> `resume` is contextual, a keyword only between a clause's `)` and its `->`, so
> it stays an ordinary identifier everywhere else.

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
boundary. ADR 0017's one deliberately open route, a continuation parked in an
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
    } with { db.get[users](k) -> map_get(cell_get(cell), k) }
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
[E0412] Error: nondeterministic effect in a deterministic test
   ╭─[ user.ply:8:13 ]
   │
 8 │   assert_eq(stamp(), stamp())
   │             ───┬───
   │                ╰───── reaches `user.clock.read`, and `user.clock` is declared `nondet`
   │
   ├─[ user.ply:8:13 ]
   │
 7 │ test "it stamps" {
   │      ─────┬─────
   │           ╰─────── test `it stamps` is deterministic
   │
   │ Note 1: `user.clock.read` is performed inside something this expression calls
   │
   │ Note 2: handle it here, e.g. `handle <body> with { clock.now() -> <value> }`
   │
   │ Note 3: or declare this `test/nondet`, which opts out of the cache and re-runs every time
───╯
```

(Transcribed from a run rather than sketched: the previous rendering here was in
a `error[E0412]: … ┌─ … ^^^` style this compiler has never emitted. Note that the
atom and the effect are printed **module-qualified** — `user.clock.read`, not
`clock.read` — because the module is what the effect is nominal in, per §3.)

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
tests may run concurrently — literally the same function: the search's
`Access::conflicts_with` delegates to `EffectAtom::conflicts_with` for atoms.
When the search exhausts its frontier the result is not a sample but a proof over
every interleaving.

A step's footprint is wider than its atoms, though, and the two extra kinds both
*add* dependence rather than remove it. Two cell accesses contend iff they are
the same location and one writes — the same readers-writers rule, at a location
instead of a resource. **Two allocations always contend**, because they draw from
one bump pointer: run in the other order, two tasks that each open a private
`with_cell` reach a *different arena*, so treating them as independent would be
unsound. `Access::Alloc` is recorded on a cell allocation and on a region open or
close, and only while a `simulate` is running. The practical consequence is that
a search can explore more than an atoms-only reading of this section predicts —
two tasks that allocate are ordered even when nothing in their rows conflicts.

**Over every interleaving the scheduler could have chosen.** Tasks interleave at
the operations the scheduler answers — `task`, `clock` and `random` — so a task
that reads shared state and writes it back with none of those in between runs the
two as one step, and no schedule separates them. That is a real limit and ADR 0006 states it: put a `task.yield()` in the window, or a `clock.now()` stamp the
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

Run that sample and the law comes back `proved` — *propositional · congruence ·
linear arithmetic · 2 unfoldings* — but `withdraw`'s `ensures` comes back
**`unattempted`**, not `proved`, with *"raised: integer overflow in subtraction,
shrunk to acct = {balance: -9223372036854775808}, amount = 9047"*. That is the
system behaving correctly and it is worth seeing here rather than being
surprised by it: `Int` is `i64` and `-` is checked, so at the bottom of the range
the expression *raises* and there is no result for the postcondition to be true
of. A proof covering "every input satisfying the guard" has to cover those too.
`requires amount > 0` does not bound the domain enough; `examples/bank.ply`'s
specs carry explicit `> -1000000000 && < 1000000000` guards for exactly this
reason. A spec that looks obviously true is not the same as one the prover can
close.

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
| `proved` | an argument covering **every** input satisfying the guard |
| `property` | randomized cases, the count reported, shrinking on failure |
| `example` | concrete cases, and no coverage claim |

`proved` is a small, exactly-stated fragment. The rules a certificate may name
are, in full (`ply_prove::Rule`): ground evaluation of a closed Boolean term;
exhaustive enumeration of a finite domain up to `ENUMERATION_BOUND` = 4096
points; linear arithmetic over `Int` — `+`, `-`, unary `-`, multiplication *by a
literal*, and the six comparisons, with `x * y` at two symbolics, `/` and `%`
excluded; propositional reasoning by case split; a case split on a scrutinee's
outermost constructor; congruence closure; constructor injectivity; and
unfolding of *non-recursive* definitions only. Recursion over unbounded data
needs induction, which is not here, so `reverse(reverse(xs)) == xs` is
`property` and should be. An inconclusive proof attempt never reports `proved`.
It reports one of two weaker things, and the distinction matters: where the
obligation has binders the prover can *sample*, it falls back and reports
`property`; where it cannot even do that — an evaluation that raised, a body
whose row is not empty, a `law/host` under a hermetic run — it reports
**`unattempted`** (`W0604`), which is a *gap* rather than a weak tier. An
`unattempted` obligation is not green, is never cached, and is counted on its own
in the summary line, as the coverage example below shows. Reporting a gap as a
weak tier would be the same defect as reporting a sample as a proof.

> **Corrected: `proved` is not always a *static* argument.** This table said "a
> static argument" and the fragment above was listed as four rules rather than
> nine. There is a ninth rule, `ExhaustiveInterleaving`, and the source calls it
> "the one certificate rule that comes from execution rather than from a static
> argument": §6's footprint-guided search empties its frontier, so every
> interleaving *ran*. It is a proof over the schedule space, and it is reached by
> running the program. `examples/bank.ply`'s concurrency law is discharged this
> way and `ply prove` reports it `proved · exhaustive over 6 interleavings`. The
> coverage claims stay independent: that law has no binders, so its value domain
> is covered for free; add one `Int` binder and the honest tier drops to
> `property` however exhaustive the search was.

> **A tier label is a truth claim.** Every prior milestone could produce a wrong
> answer; only this one can produce a wrong answer wearing a certificate. When in
> doubt, report the weaker tier.

That is enforced structurally rather than by convention: a tier is computed from
the evidence a discharge carries, and the only evidence that computes to `proved`
is a certificate naming the inference rules it used — which only the prover can
construct. `ply_prove` carries no `tier` field at all; `Evidence::tier()` derives
it.

The exception, stated because "there is no `tier` field anywhere" is what this
paragraph used to say and a reader can falsify that with one grep:
`ply_store::obligations::CachedObligation` **does** have a `pub tier: String`, in
the on-disk cache. It is not an authority and cannot be used to assert one —
`ply_test::obligation::from_cached` recomputes the tier from the evidence and
returns an error rather than a discharge when the two disagree, so a file
labelled `proved` over a case report is refused. The field exists so that such a
disagreement is *detectable* rather than silently read past. The guarantee is
intact; the absolute phrasing was not.

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

**That row is why a signature is written rather than inferred.** A definition's
published type is part of what a reviewer holds fixed, so it cannot be a summary
derived from the body it describes — under full inference, editing a body
silently republishes the claim and the row becomes a coincidence. Every
parameter type and return type on a top-level `fn` is therefore mandatory
(`E0126`), and the diagnostic names the type inference would have given so the
fix is the text of the error. The **effect row is the deliberate exception**: it
is derived from what a body calls rather than chosen, so it stays inferred
unless written, and a written one is checked as an upper bound (§1). Infer what
is mechanical; write what is meant. `docs/GUIDE.md` §5.9 is the user-facing
statement of the split.

**What this is not**: a general-purpose theorem prover, an SMT integration, or a
termination checker. `requires` is a filter on the domain of the `ensures`
clauses beside it, not a contract checked at every call site.

`docs/adr/0007-specs.md` is the specification.

## What of this is built

What is true of the shipped language:

| §  | mechanism | state |
| --- | --- | --- |
| §1–§3 | effect rows, handlers, content addressing | built (M2, M3) |
| §2 | multi-shot continuations, a reified `resume` | **built** (M6) |
| §2 | region-scoped cells on a bump arena, `with_region`, `E0446`/`E0449` | **built** (ADR 0017) |
| §4 | the exact cache and conflict-coloured scheduling | built (M4) |
| §5 | machine-shaped failure, the suspect set | built (M5) |
| §6 | deterministic simulation | **built** (M7) |
| §7 | specs, tiers, `ply prove` / `ply review` | **built** (M8) |
| —  | VM-level snapshot/fork (the persistent `World`) | built in M6, then **removed** by ADR 0017 |
| —  | native codegen | **the one thing still deferred** (M9) |

The evaluator is still an interpreter: a control-stack machine. Native
codegen is deferred on a *measurement* rather than on effort — the interpreter is
about 35% of a served request, which caps any execution-strategy change at 1.55x,
and a Cranelift spike projects 1.48x against a 1.50x bar. `docs/adr/0011-the-web-track.md`
holds the numbers and states what would reopen it. See ROADMAP.md for the
milestone-by-milestone record.

The forkable world that ADR 0005 built *has* been replaced rather than deferred:
ADR 0017 moved cells onto a region-scoped bump arena, which is why §2 above talks
about regions and brands at all.
