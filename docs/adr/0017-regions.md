# ADR 0017 — Regions, and what replaces the forkable world

**Accepted, implemented.** Supersedes ADR 0005's persistent forkable state and
amends ADR 0008's isolation section. The reclamation half is implemented by a
**compiler pass** rather than at runtime: the lowering runs a liveness analysis,
so a last use moves and a dead binding releases without a runtime check.

## Context

Two decisions taken together forced this document — and **the second of them was
an unmeasured claim, which is the most important thing in this record.**

**Zero-cost was the goal.** A code generator measured large on the fragment it
can compile and almost nothing end to end, because that fragment is a few
percent of a request; a request meanwhile allocates far more times than it
writes bytes. So raising the ceiling means fixing representation first.

**Regions are the memory model.** Ply already had most of one and had not
noticed: a cell region is a lexically-scoped region whose atoms are discharged
at the boundary, effect rows already state what code touches, and footprints
already prove disjointness.

The consequence was stated as forced: *Perceus-style in-place update fires only
on a uniquely owned value, and a design that forks state keeps reference counts
high by construction, so the persistent forkable state and the zero-cost path
are mutually exclusive.*

> **That paragraph is the premise this was accepted on, it was never measured,
> and the work's own attribution contradicts it.** It claims the forkable state
> is what holds allocations up. Removing it put allocations per request **up**.
> Hoisting two compile-time analyses that were running at runtime brought the
> figure back to where it was before the lexical close — **still above where it
> started.** Not noise: an allocation count is exact and does not move with a
> machine.
>
> A two-window fit says where the allocations are, and neither analysis is on
> the request path in either sense any more. **The largest per-request site is
> the machine's own frame dispatch, which a region model does not touch.**
>
> **The premise was a chain of sound-sounding reasoning — unique ownership,
> reference counts, forking — with no measurement under any link of it, and the
> word used for its conclusion was *forced*.** The contribution rule *measure an
> ADR's motivating claim before accepting the ADR* is written from this
> document. What the region model *is* worth is measured below: the arena
> against the persistent map it replaced, and the escape discipline — **which is
> a safety property and was never an allocation claim.**

### Why this is available now and was not before

ADR 0005 merged the control stack and forkable state into one milestone for a
specific reason: capture a continuation inside a cell region, resume it outside,
and the cell escapes. Branding the region lost on two objections, and this
dissolves only the first.

1. **Branding looked heavy in the type system** — rank-2 polymorphism for one
   construct. Building regions for memory means building that branding anyway,
   **so this objection is genuinely gone: the mechanism is one this project is
   now committed to for an unrelated reason.**
2. **Branding "forbids the programs multi-shot exists for"** — and this is
   **not** dissolved. **It is paid**, and itemized below, **because it is a
   change of program meaning under a design whose governing property is that
   meaning does not change.**

## The property this must not break

**Program meaning does not change.** Everything here alters representation and
cost, not semantics. **Where a construct cannot preserve meaning it is refused
at compile time, never silently reinterpreted.**

## 1. A region is a lexical allocation scope with a brand

Values allocated in a region live in a bump arena freed at its close. The brand
appears in the types of values allocated there, so a value cannot outlive its
region — **the ST-monad discipline, applied to allocation rather than only to
cells.** A cell region becomes a special case: a cell is a value allocated
there, and the surface syntax is unchanged, so existing programs do not move.

## 2. Escape is a type error, not a runtime check

A value's brand is part of its type. Returning it, storing it in an outer
structure, capturing it in a closure that outlives the region, or sending it to
another task are all the same error, **reported at the point the value would
escape rather than where it is later used.**

This is what replaces the forkable state's guarantee. Under ADR 0005 a cell
could not meaningfully escape **because a cell was a *key* rather than a
pointer**, so an escaped cell read a live entry rather than dangling. Here it
cannot escape because the type says so.

**The check runs over the *resolved* type, including a function type's effect
row**, because a closure that captured the cell need not mention it in its
parameters or its result.

**That retires three shapes that used to compile**: a cell returned inside a
closure, inside a record of closures, and passed to an operation. **Refusing a
program that ran is a change of meaning, so it is recorded here rather than left
to an implementation: that is the trade, and it is taken.** The forkable state
made these safe; the arena does not, and a value that outlives a freed region is
the defect this exists to make impossible. Nothing outside audit fixtures was
written in those shapes.

**One exclusion, deliberate: a cell reaching a *task* is not refused for a bare
cell region.** "A cell reaching a task is how tasks share memory" is a landed
and tested shape, and it is safe under §3 as amended — a task operation anywhere
in a region makes the region shared, and a shared region's slots outlive its
close for exactly this reason. The new region syntax keeps the stricter rule,
because it is new syntax with no program depending on the loose one.

**What the brand still does not catch**: a *continuation* parked in an enclosing
region's cell, which is a *success* rather than an error. The continuation's row
carries the inner region's atom, **and the row is erased where the constructor's
field type is declared, so no type after the constructor mentions it.** Refusing
it would need the brand to survive a nominal declaration, which is the rank-2
machinery ADR 0005 rejected. **It is recorded as the one route that is open.**

The consequence for footprints: with every other route closed, **a written row
is the only way a cell atom reaches a published footprint.**

## 3. Resumption semantics — the sharp part

The first draft said "each resumption observes the region as it was at capture"
and asserted that this *is* ADR 0005's semantics. **That assertion was false,
and the two readings are distinguishable in one integer on the very example the
section works through.** ADR 0005 threads one state and pins the
two-resumption trace cell at **2** as a required test; snapshot-at-capture
answers **1**. Since the governing property is that meaning does not change, ADR
0005 wins.

**State is threaded, exactly as ADR 0005 says.** Resumption *n* observes
resumption *n−1*'s writes. A handler that wants per-branch state builds it in
four lines with the cell it already has, **and that direction is one-way:
threaded semantics can express snapshot semantics and not the reverse.**

**Why the snapshot reading cannot be taken, stated once so it is not
re-proposed:** the canonical state handler writes the cell and then resumes.
Restoring the region at the resumption discards the clause's own write before
the computation that asked for it runs. **That is not a backtracking corner; it
retypes one-shot resumption, which is the overwhelming majority of handlers.**

### What the two region kinds decide

Not what a resumption *observes* — that is fixed above — but **when the region's
memory may be reclaimed.**

- **`unique`** — the compiler proves no continuation is captured across this
  region, so nothing can reach its slots after its lexical close: allocation is
  a bump pointer and the close is a truncation. **This is the case that is
  free.**
- **`shared`** — a continuation may be captured across and resumed after the
  close, so the slots are reference counted and reclaimed when the last
  continuation that can reach them dies. **Cost is paid only in regions where a
  capture actually happens.**

Inference picks `unique` unless a capture is reachable. An annotation forces
either, **and forcing `unique` where a capture is reachable is a compile error
naming the capture site — because that annotation is a claim that the memory can
be freed at the close, and it is a use-after-free when it is wrong.**

In the two-resumption case **the cell is allocated before the capture, so one
cell serves both resumptions and the second observes what the first wrote.**
The region is shared, and **what that buys is that the cell is still there to be
read.** **An implementation in which the second resumption fails to observe the
first's writes is wrong, not a permitted optimization**: it would silently break
every cell-backed state handler and make a lost-update race unrepresentable — **a
green run on a program with a race in it.**

A save-and-restore primitive stays in the allocator and is **not on the capture
path; wiring it there would implement the retracted reading.** Where a capture
does need one it must cover *every region open at the capture* rather than only
the innermost, because a resumption may write any of them.

## 4. What escapes a region is reference-counted

**Whether a given update is in place is a *dynamic* test, and this section
originally stated it as a compile-time property — in the section a reader comes
to in order to ask whether Ply reuses memory. It answered yes unconditionally.**

An append rewrites in place only if the pointer is unshared **at that instant**;
otherwise it allocates and copies the whole array. The compiler's entire
contribution is an ownership hint on a variable node, and that hint is
documented as *an optimization hint and never a permission* — a wrong hint costs
a wasted walk and can never change an answer. **That is the property which keeps
the differential oracle meaningful, and it is exactly why nothing here is a
guarantee about cost.**

**What decides sharing in practice is position.** A pending frame is handed a
live clone of the scope whenever any sub-expression of the enclosing node
remains, **and it never asks what those remaining sub-expressions read.** So a
value built anywhere but the last sub-expression of its enclosing node is
aliased when the probe runs and is copied — once per element for a growing
container, which is quadratic — **and the trailing sub-expression that costs the
copy may be a literal constant.** The rule composes across call boundaries, **so
a correctly written callee is made quadratic by its caller.**

**This therefore establishes no complexity guarantee.** It establishes that
in-place update is *available*; whether a given program gets it is a fact about
where its sub-expressions sit, decided at run time and reported nowhere. **That
is the same shape as the Context's correction one level up — the region work is
a safety property and was never an allocation claim — and a cost claim read out
of this section is that same unmeasured inference one level down.**

Cycles are not collected. A cycle among escaped values leaks; say so in the
diagnostics where a cycle is constructible, and revisit if it proves to matter.

## 5. Tasks each hold a region stack

Values cannot cross tasks except through explicitly shared regions or effects,
because the brand prevents it. **Footprints are unaffected: they are static and
do not depend on how memory is represented.**

## 6. Test isolation becomes region isolation

A test's allocations live in a region closed when the test ends, so tests still
cannot observe each other's allocations.

**What is genuinely lost is the case where two tests share a resource label but
have disjoint state** — under forking they parallelized, and now they are
grouped by footprint conflict. **The number that matters is not the isolated
count**: a pure test has an empty footprint and conflicts with nothing
regardless, **so the question is how many currently-isolated tests would newly
serialize.** Measured at zero on this corpus — **and the reason is stated rather
than celebrated: no test carries a cell atom in its footprint at all, so the
exemption was exempting nothing.** A hypothetical mode prices the risk for a
corpus that would.

**The reporting must keep saying which tests are isolated and which contend**,
or the trivially-parallel count silently over-claims. ADR 0008 established that
trap when host-backed tests lost isolation, **and the same trap applies here.**

## What was measured

**Allocations per request — the number this exists to move, and it moved the
wrong way.** Against the right baseline (the constant memo had already landed
and taken the same request down) the region track is up a few percent in
allocations and in bytes, with the arena wired. **So the hypothesis that making
the arena the cell store would move this figure is falsified on this route**: the
route allocates no cells at all, and its allocations are on the request path —
framing, routing, encoding — **which a region model does not touch.** Unboxing
and monomorphization are what move them, and both are out of scope here.

**What the arena *is* worth is visible where cells exist.** Against the
persistent map it replaced, at ten thousand cells, the map costs tens of
thousands of allocations and a megabyte to build and ten thousand more to write
every cell; **the region costs nothing to build, nothing to write and nothing to
close.**

**Two cautions on those numbers, because this document has been burned by
both.** *A window share is not a request cost*: the lowering reads a third of a
short window while contributing **nothing** per request, and a single-window
attribution read as request-path work is exactly the mistake this shape
produces. And a *bytes* column is only comparable at the published window — it
rises with the window while the allocation count falls with it exactly as a
slope plus an intercept must, **and this milestone did not diagnose why.**

**The dynamic split is where the model is worth something, and it is not the
static one.** Statically, every region in the corpus is `shared` and none is
`unique`, every one because of a tail-resumptive clause — **which under a design
where `shared` meant "never reclaimed" would have said the zero-cost claim buys
nothing at all.** It does not mean that: the kinds are a claim about what a
close will *find* rather than a decision about whether one happens. Every
`shared` region reclaims at its close on every run, because no continuation
captured across one ever outlives it there.

**Region-scoped fixture cost**, measured the way forking's was **so "cheap"
stays a fact rather than becoming a slogan**: opening a large fixture and
writing one cell costs a fraction of a millisecond per test, against a fork's
nanosecond. **This is the price section 6 says is paid, and it is paid per test
rather than per group** — and it is a projection about a construct that is still
not writable in Ply.

**The oracle for "meaning did not move" is not the two-engine comparison**, and
the size of the hole is stated rather than implied. The tree-walker refuses
every clause that binds a continuation, **so that comparison audits nothing
about multi-shot resumption, which is precisely the construct this changes.** A
second and larger hole: **both engines hold the same state representation, so a
change to the memory model moves them together and the comparison stays green
whatever it did to meaning.** What actually audits it is a file of programs
whose answer *differs* between the two candidate readings, with the expected
integer written down.

## Where this could go wrong

In order of how hard it would be to see.

- **A resumption failing to observe a previous resumption's writes.** Silent,
  and it breaks every cell-backed state handler.
- **An escape the brand does not catch** — through a closure, a constructor
  field, a map key, a returned continuation, or a task. **An analogous hole was
  found reachable through a *type alias*, so the check must run on resolved
  types.**
- **`unique` inferred where a capture is reachable**, which frees memory a
  continuation can still reach. **Inference must be conservative: when in doubt,
  shared.** In particular a `handle` that lexically *encloses* a region does not
  make the operations it answers local to that region — it answers across the
  boundary, **which is the definition of a capture crossing it.**

  *That one happened, and it was found by a reader rather than by a test.* The
  analysis carried no local scope, so a parameter or binder shadowing a
  top-level definition's name was read as that definition — recording an edge to
  a definition that reaches no capture instead of an indirect call, **and
  inferring `unique` over a callee that is whatever the caller passed.** Latent
  rather than live only because the close never reads the kind, **but the
  annotation check did accept a hand-written `unique` it is required to refuse.**

## Consequences

The forkable state is removed. The code generator's ceiling was re-measured
after this landed **and it did not move**; the whole ladder was re-taken and the
verdict is unchanged. **The absolutes are all larger than the previous take's
and the reason is the rig rather than this change** — the Rust floor, which has
no Ply under it at all, moved too. **The portable readings are the ratios, and
they are flat.** The pre-region ladder is deliberately not overwritten: it is
the only record of the baseline this document's corrections are anchored to.

## Not in this ADR

Unboxed primitive representation, monomorphization, evidence passing and handler
specialization, and native codegen. Each is a separate milestone; **this one
establishes the memory model they all depend on** — a claim that should be read
against the Context's correction.
